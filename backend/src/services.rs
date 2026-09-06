use crate::db;
use crate::{
    auth::{self, now, stamp},
    error::{Error, Result},
    models::{Project, Task, TaskFilter, User},
    state::AppState,
};
use chrono::{Days, NaiveDate, TimeZone};
use serde::Deserialize;
use serde_json::{Value, json};
use turso::Connection;

pub const TASK_SELECT: &str = "SELECT t.*,p.name AS project_name,p.color AS project_color,p.archived_at AS project_archived_at,u.name AS assignee_name,CASE WHEN t.due_at<=CAST(strftime('%s','now') AS INTEGER) AND t.status NOT IN ('done','canceled') AND t.archived_at IS NULL AND p.archived_at IS NULL THEN 1 ELSE 0 END AS overdue FROM tasks t JOIN projects p ON p.id=t.project_id LEFT JOIN users u ON u.id=t.assignee_id";

pub async fn current_actor(conn: &Connection, actor: &User) -> Result<User> {
    let user = db::optional_as::<User>(
        conn,
        &format!(
            "SELECT {} FROM users WHERE id=? AND active=1",
            auth::USER_COLUMNS
        ),
        turso::params![actor.id],
    )
    .await?
    .ok_or_else(Error::unauthorized)?;
    Ok(user)
}
pub fn require_editor(actor: &User) -> Result<()> {
    if !actor.editor() {
        return Err(Error::forbidden());
    }
    Ok(())
}
pub async fn task_on(conn: &Connection, id: i64) -> Result<Task> {
    db::optional_as::<Task>(
        conn,
        &format!("{TASK_SELECT} WHERE t.id=?"),
        turso::params![id],
    )
    .await?
    .ok_or_else(Error::missing)
}
pub async fn get_task(state: &AppState, id: i64) -> Result<Task> {
    task_on(&state.db.connect().await?, id).await
}
pub fn editable(task: &Task) -> Result<()> {
    if task.archived_at.is_some() {
        return Err(Error::bad("Restore this task before editing it."));
    }
    if task.project_archived_at.is_some() {
        return Err(Error::bad("Restore the project before editing its tasks."));
    }
    Ok(())
}
pub async fn active_project(conn: &Connection, id: i64) -> Result<Project> {
    let p = db::optional_as::<Project>(
        conn,
        "SELECT * FROM projects WHERE id=?",
        turso::params![id],
    )
    .await?
    .ok_or_else(Error::missing)?;
    if p.archived_at.is_some() {
        return Err(Error::bad("Restore the project first."));
    }
    Ok(p)
}
pub async fn list_tasks(state: &AppState, f: &TaskFilter) -> Result<Value> {
    let page = f.page.unwrap_or(1).clamp(1, 1_000_000);
    let limit = f.limit.unwrap_or(50).clamp(1, 100);
    let mut query = TASK_SELECT.to_owned();
    let mut params: Vec<turso::Value> = Vec::new();
    query.push_str(if f.archived.unwrap_or(false) {
        " WHERE t.archived_at IS NOT NULL"
    } else {
        " WHERE t.archived_at IS NULL AND p.archived_at IS NULL"
    });
    if let Some(id) = f.project {
        query.push_str(" AND t.project_id=?");
        params.push(id.into());
    }
    if let Some(id) = f.assignee {
        query.push_str(" AND t.assignee_id=?");
        params.push(id.into());
    }
    if let Some(status) = &f.status {
        if status == "active" {
            query.push_str(" AND t.status NOT IN ('done','canceled')");
        } else if status != "all" {
            valid_status(status)?;
            query.push_str(" AND t.status=?");
            params.push(status.as_str().into());
        }
    }
    if let Some(q) = f.q.as_ref().filter(|s| !s.trim().is_empty()) {
        if q.len() > 500 {
            return Err(Error::bad("Search is too long."));
        }
        let q = format!(
            "%{}%",
            q.trim()
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        query.push_str(" AND (t.title LIKE ?");
        params.push(q.clone().into());
        query.push_str(" ESCAPE '\\' OR t.description LIKE ?");
        params.push(q.into());
        query.push_str(" ESCAPE '\\')");
    }
    if let Some(due) = &f.due {
        query.push_str(" AND t.status NOT IN ('done','canceled')");
        match due.as_str() {
            "overdue" => {
                query.push_str(" AND t.due_at<=?");
                params.push(now().into());
            }
            "soon" => {
                query.push_str(" AND t.due_at>?");
                params.push(now().into());
                query.push_str(" AND t.due_at<=?");
                params.push((now() + 7 * 86400).into());
            }
            "today" => {
                query.push_str(" AND t.due_start<=?");
                params.push(now().into());
                query.push_str(" AND t.due_at>?");
                params.push(now().into());
            }
            _ => return Err(Error::bad("Invalid due-date filter.")),
        }
    }
    query.push_str(" ORDER BY t.created_at DESC,t.id DESC LIMIT ?");
    params.push((limit + 1).into());
    query.push_str(" OFFSET ?");
    params.push(((page - 1) * limit).into());
    let mut tasks: Vec<Task> = db::all_as(&state.db.connect().await?, &query, params).await?;
    let has_more = tasks.len() as i64 > limit;
    tasks.truncate(limit as usize);
    Ok(json!({"items":tasks,"page":page,"has_more":has_more}))
}

#[derive(Clone, Deserialize)]
pub struct TaskInput {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_status")]
    pub status: String,
    pub assignee_id: Option<i64>,
    pub due_date: Option<String>,
    pub version: Option<i64>,
}
fn default_status() -> String {
    "todo".into()
}
impl From<&Task> for TaskInput {
    fn from(t: &Task) -> Self {
        Self {
            title: t.title.clone(),
            description: t.description.clone(),
            status: t.status.clone(),
            assignee_id: t.assignee_id,
            due_date: t.due_date.clone(),
            version: Some(t.version),
        }
    }
}
pub fn valid_status(status: &str) -> Result<()> {
    if !["todo", "in_progress", "blocked", "done", "canceled"].contains(&status) {
        return Err(Error::bad("Unknown task status."));
    }
    Ok(())
}
pub fn text(value: &str, name: &str, max: usize, required: bool) -> Result<String> {
    let value = value.trim();
    if (required && value.is_empty()) || value.chars().count() > max {
        return Err(Error::bad(format!(
            "{name} must be {}–{max} characters.",
            if required { 1 } else { 0 }
        )));
    }
    Ok(value.into())
}
pub fn deadline(date: Option<&str>, zone: &str) -> Result<(Option<i64>, Option<i64>)> {
    let Some(date) = date else {
        return Ok((None, None));
    };
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map_err(|_| Error::bad("Use a valid due date (YYYY-MM-DD)."))?;
    let tz: chrono_tz::Tz = zone.parse().map_err(|_| Error::bad("Invalid time zone."))?;
    let next = date
        .checked_add_days(Days::new(1))
        .ok_or_else(|| Error::bad("Invalid due date."))?;
    let midnight = |d: NaiveDate| {
        tz.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
            .earliest()
            .or_else(|| {
                tz.from_local_datetime(&d.and_hms_opt(1, 0, 0).unwrap())
                    .earliest()
            })
            .map(|v| v.timestamp())
            .ok_or_else(|| Error::bad("This date does not exist in the selected time zone."))
    };
    Ok((Some(midnight(date)?), Some(midnight(next)?)))
}
async fn validate_task(conn: &Connection, input: &TaskInput) -> Result<()> {
    text(&input.title, "Title", 200, true)?;
    text(&input.description, "Description", 500_000, false)?;
    valid_status(&input.status)?;
    if let Some(id) = input.assignee_id
        && db::optional_scalar::<i64>(
            conn,
            "SELECT id FROM users WHERE id=? AND active=1",
            turso::params![id],
        )
        .await?
        .is_none()
    {
        return Err(Error::bad("Choose an active assignee."));
    }
    Ok(())
}
pub async fn event(
    conn: &Connection,
    actor: &User,
    task: &Task,
    kind: &str,
    detail: &str,
    source: &str,
) -> Result<()> {
    let event_id = db::execute(conn, "INSERT INTO activity_events(task_id,project_id,actor_id,kind,detail,source) VALUES(?,?,?,?,?,?)", turso::params![task.id, task.project_id, actor.id, kind, detail, source]).await?.last_insert_rowid();
    let watchers = db::all(conn, "SELECT u.id,u.activity_in_app,u.activity_discord FROM task_watchers w JOIN users u ON u.id=w.user_id WHERE w.task_id=? AND u.id!=? AND u.active=1", turso::params![task.id, actor.id]).await?;
    for watcher in watchers {
        let user: i64 = watcher.get("id");
        let in_app: bool = watcher.get("activity_in_app");
        let discord: bool = watcher.get("activity_discord");
        if !in_app && !discord {
            continue;
        }
        let message = format!("{} {} · TB-{} {}", actor.name, detail, task.id, task.title);
        let notification = db::execute(conn, "INSERT INTO notifications(user_id,task_id,event_id,kind,message,dedupe_key,in_app) VALUES(?,?,?,'activity',?,?,?)", turso::params![user, task.id, event_id, message, format!("event:{event_id}:{user}"), in_app]).await?.last_insert_rowid();
        if discord {
            db::execute(
                conn,
                "INSERT INTO notification_deliveries(notification_id) VALUES(?)",
                turso::params![notification],
            )
            .await?;
        }
    }
    Ok(())
}
pub async fn create_task(
    state: &AppState,
    actor: &User,
    project: i64,
    input: TaskInput,
) -> Result<Task> {
    let _guard = state.writes.lock().await;
    let mut conn = state.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    let actor = current_actor(&tx, actor).await?;
    require_editor(&actor)?;
    state.check_capacity(&tx).await?;
    active_project(&tx, project).await?;
    validate_task(&tx, &input).await?;
    let due = deadline(input.due_date.as_deref(), &actor.timezone)?;
    let id = db::execute(&tx, "INSERT INTO tasks(project_id,title,description,status,assignee_id,creator_id,due_date,due_timezone,due_start,due_at) VALUES(?,?,?,?,?,?,?,?,?,?)", turso::params![project, input.title.trim(), input.description.as_str(), input.status, input.assignee_id, actor.id, input.due_date, actor.timezone.as_str(), due.0, due.1]).await?.last_insert_rowid();
    db::execute(
        &tx,
        "INSERT INTO task_watchers(task_id,user_id) VALUES(?,?)",
        turso::params![id, actor.id],
    )
    .await?;
    let task = task_on(&tx, id).await?;
    event(&tx, &actor, &task, "created", "created the task", "web").await?;
    state.check_result_size(&tx).await?;
    tx.commit().await?;
    Ok(task)
}
async fn receipt(
    conn: &Connection,
    actor: &User,
    task: i64,
    action: &str,
    interaction: Option<&str>,
) -> Result<bool> {
    if let Some(id) = interaction {
        let inserted = db::execute(conn, "INSERT OR IGNORE INTO discord_interactions(interaction_id,user_id,task_id,action,created_at) VALUES(?,?,?,?,?)", turso::params![id, actor.id, task, action, now()]).await?.rows_affected();
        return Ok(inserted == 0);
    }
    Ok(false)
}
pub async fn save_task(
    state: &AppState,
    actor: &User,
    id: i64,
    input: TaskInput,
    source: &str,
    interaction: Option<&str>,
) -> Result<Task> {
    let _guard = state.writes.lock().await;
    let mut conn = state.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    let actor = current_actor(&tx, actor).await?;
    require_editor(&actor)?;
    let old = task_on(&tx, id).await?;
    if receipt(&tx, &actor, id, "update", interaction).await? {
        return Ok(old);
    }
    editable(&old)?;
    if input.version != Some(old.version) {
        return Err(Error::conflict());
    }
    validate_task(&tx, &input).await?;
    let grows =
        input.title.len() + input.description.len() > old.title.len() + old.description.len();
    if grows {
        state.check_capacity(&tx).await?;
    }
    let mut changes = Vec::new();
    if input.title.trim() != old.title {
        changes.push("title");
    }
    if input.description != old.description {
        changes.push("description");
    }
    if input.status != old.status {
        changes.push("status");
    }
    if input.assignee_id != old.assignee_id {
        changes.push("assignee");
    }
    if input.due_date != old.due_date {
        changes.push("due date");
    }
    if changes.is_empty() {
        tx.commit().await?;
        return Ok(old);
    }
    let zone = if input.due_date != old.due_date {
        &actor.timezone
    } else {
        &old.due_timezone
    };
    let due = deadline(input.due_date.as_deref(), zone)?;
    let revision = input.due_date != old.due_date
        || input.assignee_id != old.assignee_id
        || input.status != old.status;
    db::execute(&tx, "UPDATE tasks SET title=?,description=?,status=?,assignee_id=?,due_date=?,due_timezone=?,due_start=?,due_at=?,version=version+1,reminder_revision=reminder_revision+?,updated_at=? WHERE id=?", turso::params![input.title.trim(), input.description, input.status, input.assignee_id, input.due_date, zone.as_str(), due.0, due.1, i64::from(revision), stamp(), id]).await?;
    if revision {
        cancel_due(&tx, id).await?;
    }
    let task = task_on(&tx, id).await?;
    event(
        &tx,
        &actor,
        &task,
        "updated",
        &format!("updated {}", changes.join(", ")),
        source,
    )
    .await?;
    if grows {
        state.check_result_size(&tx).await?;
    }
    tx.commit().await?;
    Ok(task)
}
pub async fn cancel_due(conn: &Connection, task: i64) -> Result<()> {
    db::execute(conn, "UPDATE notification_deliveries SET state='canceled' WHERE state IN ('pending','retry','leased') AND notification_id IN (SELECT id FROM notifications WHERE task_id=? AND kind='due')", turso::params![task]).await?;
    Ok(())
}
pub async fn archive_task(
    state: &AppState,
    actor: &User,
    id: i64,
    version: i64,
    action: &str,
    source: &str,
    interaction: Option<&str>,
) -> Result<Option<Task>> {
    let _guard = state.writes.lock().await;
    let mut conn = state.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    let actor = current_actor(&tx, actor).await?;
    require_editor(&actor)?;
    let task = task_on(&tx, id).await?;
    if receipt(&tx, &actor, id, action, interaction).await? {
        return Ok(Some(task));
    }
    if task.version != version {
        return Err(Error::conflict());
    }
    match action {
        "archive" => {
            if task.archived_at.is_some() {
                tx.commit().await?;
                return Ok(Some(task));
            }
            editable(&task)?;
            db::execute(&tx, "UPDATE tasks SET archived_at=?,archived_by=?,version=version+1,reminder_revision=reminder_revision+1,updated_at=? WHERE id=?", turso::params![stamp(), actor.id, stamp(), id]).await?;
        }
        "restore" => {
            if task.project_archived_at.is_some() {
                return Err(Error::bad("Restore the project first."));
            }
            if task.archived_at.is_none() {
                tx.commit().await?;
                return Ok(Some(task));
            }
            db::execute(&tx, "UPDATE tasks SET archived_at=NULL,archived_by=NULL,version=version+1,reminder_revision=reminder_revision+1,updated_at=? WHERE id=?", turso::params![stamp(), id]).await?;
        }
        "purge" => {
            if task.archived_at.is_none() {
                return Err(Error::bad(
                    "Archive the task before permanently deleting it.",
                ));
            }
            db::execute(
                &tx,
                "INSERT INTO admin_audit_log(actor_id,action,target) VALUES(?,'purge_task',?)",
                turso::params![actor.id, format!("TB-{id}")],
            )
            .await?;
            db::execute(&tx, "DELETE FROM tasks WHERE id=?", turso::params![id]).await?;
            tx.commit().await?;
            return Ok(None);
        }
        _ => return Err(Error::bad("Unknown action.")),
    }
    cancel_due(&tx, id).await?;
    event(
        &tx,
        &actor,
        &task,
        action,
        if action == "archive" {
            "moved the task to Archive"
        } else {
            "restored the task"
        },
        source,
    )
    .await?;
    let updated = task_on(&tx, id).await?;
    tx.commit().await?;
    Ok(Some(updated))
}
pub async fn watch(state: &AppState, actor: &User, id: i64, watching: bool) -> Result<()> {
    let _guard = state.writes.lock().await;
    let mut conn = state.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    current_actor(&tx, actor).await?;
    let task = task_on(&tx, id).await?;
    if watching {
        editable(&task)?;
        state.check_capacity(&tx).await?;
        db::execute(
            &tx,
            "INSERT OR IGNORE INTO task_watchers(task_id,user_id) VALUES(?,?)",
            turso::params![id, actor.id],
        )
        .await?;
        state.check_result_size(&tx).await?;
    } else {
        db::execute(
            &tx,
            "DELETE FROM task_watchers WHERE task_id=? AND user_id=?",
            turso::params![id, actor.id],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
pub async fn add_comment(state: &AppState, actor: &User, id: i64, body: &str) -> Result<()> {
    let body = text(body, "Comment", 100_000, true)?;
    let _guard = state.writes.lock().await;
    let mut conn = state.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    let actor = current_actor(&tx, actor).await?;
    require_editor(&actor)?;
    state.check_capacity(&tx).await?;
    let task = task_on(&tx, id).await?;
    editable(&task)?;
    db::execute(
        &tx,
        "INSERT INTO comments(task_id,author_id,body) VALUES(?,?,?)",
        turso::params![id, actor.id, body],
    )
    .await?;
    event(
        &tx,
        &actor,
        &task,
        "commented",
        "commented on the task",
        "web",
    )
    .await?;
    state.check_result_size(&tx).await?;
    tx.commit().await?;
    Ok(())
}
pub async fn link(state: &AppState, actor: &User, id: i64, link: &str, label: &str) -> Result<()> {
    let url = url::Url::parse(link).map_err(|_| Error::bad("Enter a complete https:// link."))?;
    if !matches!(url.scheme(), "https" | "http")
        || !url.username().is_empty()
        || url.password().is_some()
        || link.len() > 4000
    {
        return Err(Error::bad("Use an HTTP or HTTPS link without credentials."));
    }
    let label = text(label, "Link label", 200, false)?;
    let _guard = state.writes.lock().await;
    let mut conn = state.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    let actor = current_actor(&tx, actor).await?;
    require_editor(&actor)?;
    state.check_capacity(&tx).await?;
    let task = task_on(&tx, id).await?;
    editable(&task)?;
    db::execute(
        &tx,
        "INSERT INTO task_links(task_id,url,label) VALUES(?,?,?)",
        turso::params![id, url.as_str(), label],
    )
    .await?;
    event(&tx, &actor, &task, "linked", "added a link", "web").await?;
    state.check_result_size(&tx).await?;
    tx.commit().await?;
    Ok(())
}
