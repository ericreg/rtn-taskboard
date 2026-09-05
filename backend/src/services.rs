use crate::{auth::{self, now, stamp}, error::{Error, Result}, models::{Project, Task, TaskFilter, User}, state::AppState};
use chrono::{Days, NaiveDate, TimeZone};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};

pub const TASK_SELECT: &str = "SELECT t.*,p.name AS project_name,p.color AS project_color,p.archived_at AS project_archived_at,u.name AS assignee_name,CASE WHEN t.due_at<=CAST(strftime('%s','now') AS INTEGER) AND t.status NOT IN ('done','canceled') AND t.archived_at IS NULL AND p.archived_at IS NULL THEN 1 ELSE 0 END AS overdue FROM tasks t JOIN projects p ON p.id=t.project_id LEFT JOIN users u ON u.id=t.assignee_id";

pub async fn current_actor(conn: &mut SqliteConnection, actor: &User) -> Result<User> {
    let user = sqlx::query_as::<_,User>(&format!("SELECT {} FROM users WHERE id=? AND active=1", auth::USER_COLUMNS)).bind(actor.id).fetch_optional(conn).await?.ok_or_else(Error::unauthorized)?;
    Ok(user)
}
pub fn require_editor(actor: &User) -> Result<()> {
    if !actor.editor() { return Err(Error::forbidden()); }
    Ok(())
}
pub async fn task_on(conn: &mut SqliteConnection, id: i64) -> Result<Task> {
    sqlx::query_as::<_, Task>(&format!("{TASK_SELECT} WHERE t.id=?")).bind(id).fetch_optional(conn).await?.ok_or_else(Error::missing)
}
pub async fn get_task(state: &AppState, id: i64) -> Result<Task> { task_on(&mut *state.pool.acquire().await?, id).await }
pub fn editable(task: &Task) -> Result<()> {
    if task.archived_at.is_some() { return Err(Error::bad("Restore this task before editing it.")); }
    if task.project_archived_at.is_some() { return Err(Error::bad("Restore the project before editing its tasks.")); } Ok(())
}
pub async fn active_project(conn: &mut SqliteConnection, id: i64) -> Result<Project> {
    let p = sqlx::query_as::<_,Project>("SELECT * FROM projects WHERE id=?").bind(id).fetch_optional(conn).await?.ok_or_else(Error::missing)?;
    if p.archived_at.is_some() { return Err(Error::bad("Restore the project first.")); } Ok(p)
}
pub async fn list_tasks(state: &AppState, f: &TaskFilter) -> Result<Value> {
    let page = f.page.unwrap_or(1).clamp(1,1_000_000); let limit = f.limit.unwrap_or(50).clamp(1,100);
    let mut query = QueryBuilder::<Sqlite>::new(TASK_SELECT);
    query.push(if f.archived.unwrap_or(false) { " WHERE t.archived_at IS NOT NULL" } else { " WHERE t.archived_at IS NULL AND p.archived_at IS NULL" });
    if let Some(id) = f.project { query.push(" AND t.project_id=").push_bind(id); }
    if let Some(id) = f.assignee { query.push(" AND t.assignee_id=").push_bind(id); }
    if let Some(status) = &f.status {
        if status == "active" { query.push(" AND t.status NOT IN ('done','canceled')"); }
        else if status != "all" { valid_status(status)?; query.push(" AND t.status=").push_bind(status); }
    }
    if let Some(q) = f.q.as_ref().filter(|s| !s.trim().is_empty()) {
        if q.len() > 500 { return Err(Error::bad("Search is too long.")); }
        let q = format!("%{}%", q.trim().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        query.push(" AND (t.title LIKE ").push_bind(q.clone()).push(" ESCAPE '\\' OR t.description LIKE ").push_bind(q).push(" ESCAPE '\\')");
    }
    if let Some(due) = &f.due {
        query.push(" AND t.status NOT IN ('done','canceled')");
        match due.as_str() {
            "overdue" => { query.push(" AND t.due_at<=").push_bind(now()); }
            "soon" => { query.push(" AND t.due_at>").push_bind(now()).push(" AND t.due_at<=").push_bind(now()+7*86400); }
            "today" => { query.push(" AND t.due_start<=").push_bind(now()).push(" AND t.due_at>").push_bind(now()); }
            _ => return Err(Error::bad("Invalid due-date filter.")),
        }
    }
    query.push(" ORDER BY t.created_at DESC,t.id DESC LIMIT ").push_bind(limit+1).push(" OFFSET ").push_bind((page-1)*limit);
    let mut tasks: Vec<Task> = query.build_query_as().fetch_all(&state.pool).await?;
    let has_more = tasks.len() as i64 > limit; tasks.truncate(limit as usize);
    Ok(json!({"items":tasks,"page":page,"has_more":has_more}))
}

#[derive(Clone, Deserialize)]
pub struct TaskInput {
    pub title: String,
    #[serde(default)] pub description: String,
    #[serde(default="default_status")] pub status: String,
    pub assignee_id: Option<i64>, pub due_date: Option<String>, pub version: Option<i64>,
}
fn default_status() -> String { "todo".into() }
impl From<&Task> for TaskInput {
    fn from(t: &Task) -> Self { Self { title:t.title.clone(), description:t.description.clone(), status:t.status.clone(), assignee_id:t.assignee_id, due_date:t.due_date.clone(), version:Some(t.version) } }
}
pub fn valid_status(status: &str) -> Result<()> { if !["todo","in_progress","blocked","done","canceled"].contains(&status) { return Err(Error::bad("Unknown task status.")); } Ok(()) }
pub fn text(value: &str, name: &str, max: usize, required: bool) -> Result<String> {
    let value = value.trim(); if (required && value.is_empty()) || value.chars().count()>max { return Err(Error::bad(format!("{name} must be {}–{max} characters.", if required {1} else {0}))); } Ok(value.into())
}
pub fn deadline(date: Option<&str>, zone: &str) -> Result<(Option<i64>,Option<i64>)> {
    let Some(date) = date else { return Ok((None,None)); };
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| Error::bad("Use a valid due date (YYYY-MM-DD)."))?;
    let tz: chrono_tz::Tz = zone.parse().map_err(|_| Error::bad("Invalid time zone."))?;
    let next = date.checked_add_days(Days::new(1)).ok_or_else(|| Error::bad("Invalid due date."))?;
    let midnight = |d: NaiveDate| tz.from_local_datetime(&d.and_hms_opt(0,0,0).unwrap()).earliest().or_else(|| tz.from_local_datetime(&d.and_hms_opt(1,0,0).unwrap()).earliest()).map(|v|v.timestamp()).ok_or_else(|| Error::bad("This date does not exist in the selected time zone."));
    Ok((Some(midnight(date)?),Some(midnight(next)?)))
}
async fn validate_task(conn: &mut SqliteConnection, input: &TaskInput) -> Result<()> {
    text(&input.title,"Title",200,true)?; text(&input.description,"Description",500_000,false)?; valid_status(&input.status)?;
    if let Some(id) = input.assignee_id {
        if sqlx::query_scalar::<_,i64>("SELECT id FROM users WHERE id=? AND active=1").bind(id).fetch_optional(conn).await?.is_none() { return Err(Error::bad("Choose an active assignee.")); }
    } Ok(())
}
pub async fn event(conn: &mut SqliteConnection, actor: &User, task: &Task, kind: &str, detail: &str, source: &str) -> Result<()> {
    let event_id = sqlx::query("INSERT INTO activity_events(task_id,project_id,actor_id,kind,detail,source) VALUES(?,?,?,?,?,?)").bind(task.id).bind(task.project_id).bind(actor.id).bind(kind).bind(detail).bind(source).execute(&mut *conn).await?.last_insert_rowid();
    let watchers = sqlx::query("SELECT u.id,u.activity_in_app,u.activity_discord FROM task_watchers w JOIN users u ON u.id=w.user_id WHERE w.task_id=? AND u.id!=? AND u.active=1").bind(task.id).bind(actor.id).fetch_all(&mut *conn).await?;
    for watcher in watchers {
        let user: i64 = watcher.get("id"); let in_app: bool = watcher.get("activity_in_app"); let discord: bool = watcher.get("activity_discord");
        if !in_app && !discord { continue; }
        let message = format!("{} {} · TB-{} {}",actor.name,detail,task.id,task.title);
        let notification = sqlx::query("INSERT INTO notifications(user_id,task_id,event_id,kind,message,dedupe_key,in_app) VALUES(?,?,?,'activity',?,?,?)").bind(user).bind(task.id).bind(event_id).bind(message).bind(format!("event:{event_id}:{user}")).bind(in_app).execute(&mut *conn).await?.last_insert_rowid();
        if discord { sqlx::query("INSERT INTO notification_deliveries(notification_id) VALUES(?)").bind(notification).execute(&mut *conn).await?; }
    } Ok(())
}
pub async fn create_task(state: &AppState, actor: &User, project: i64, input: TaskInput) -> Result<Task> {
    let _guard = state.writes.lock().await; let mut tx = state.pool.begin().await?;
    let actor = current_actor(&mut tx,actor).await?; require_editor(&actor)?; state.check_capacity(&mut tx).await?; active_project(&mut tx,project).await?;
    validate_task(&mut tx,&input).await?; let due = deadline(input.due_date.as_deref(),&actor.timezone)?;
    let id = sqlx::query("INSERT INTO tasks(project_id,title,description,status,assignee_id,creator_id,due_date,due_timezone,due_start,due_at) VALUES(?,?,?,?,?,?,?,?,?,?)")
        .bind(project).bind(input.title.trim()).bind(&input.description).bind(input.status).bind(input.assignee_id).bind(actor.id).bind(input.due_date).bind(&actor.timezone).bind(due.0).bind(due.1).execute(&mut *tx).await?.last_insert_rowid();
    sqlx::query("INSERT INTO task_watchers(task_id,user_id) VALUES(?,?)").bind(id).bind(actor.id).execute(&mut *tx).await?;
    let task = task_on(&mut tx,id).await?; event(&mut tx,&actor,&task,"created","created the task","web").await?;
    state.check_result_size(&mut tx).await?; tx.commit().await?; Ok(task)
}
async fn receipt(conn: &mut SqliteConnection, actor: &User, task: i64, action: &str, interaction: Option<&str>) -> Result<bool> {
    if let Some(id) = interaction {
        let inserted = sqlx::query("INSERT OR IGNORE INTO discord_interactions(interaction_id,user_id,task_id,action,created_at) VALUES(?,?,?,?,?)").bind(id).bind(actor.id).bind(task).bind(action).bind(now()).execute(conn).await?.rows_affected();
        return Ok(inserted == 0);
    } Ok(false)
}
pub async fn save_task(state: &AppState, actor: &User, id: i64, input: TaskInput, source: &str, interaction: Option<&str>) -> Result<Task> {
    let _guard = state.writes.lock().await; let mut tx = state.pool.begin().await?;
    let actor = current_actor(&mut tx,actor).await?; require_editor(&actor)?;
    let old = task_on(&mut tx,id).await?;
    if receipt(&mut tx,&actor,id,"update",interaction).await? { return Ok(old); }
    editable(&old)?; if input.version != Some(old.version) { return Err(Error::conflict()); }
    validate_task(&mut tx,&input).await?;
    let grows = input.title.len()+input.description.len() > old.title.len()+old.description.len();
    if grows { state.check_capacity(&mut tx).await?; }
    let mut changes = Vec::new();
    if input.title.trim()!=old.title { changes.push("title"); } if input.description!=old.description { changes.push("description"); }
    if input.status!=old.status { changes.push("status"); } if input.assignee_id!=old.assignee_id { changes.push("assignee"); }
    if input.due_date!=old.due_date { changes.push("due date"); }
    if changes.is_empty() { tx.commit().await?; return Ok(old); }
    let zone = if input.due_date != old.due_date { &actor.timezone } else { &old.due_timezone };
    let due = deadline(input.due_date.as_deref(),zone)?;
    let revision = input.due_date != old.due_date || input.assignee_id != old.assignee_id || input.status != old.status;
    sqlx::query("UPDATE tasks SET title=?,description=?,status=?,assignee_id=?,due_date=?,due_timezone=?,due_start=?,due_at=?,version=version+1,reminder_revision=reminder_revision+?,updated_at=? WHERE id=?")
        .bind(input.title.trim()).bind(input.description).bind(input.status).bind(input.assignee_id).bind(input.due_date).bind(zone).bind(due.0).bind(due.1).bind(i64::from(revision)).bind(stamp()).bind(id).execute(&mut *tx).await?;
    if revision { cancel_due(&mut tx,id).await?; }
    let task = task_on(&mut tx,id).await?; event(&mut tx,&actor,&task,"updated",&format!("updated {}",changes.join(", ")),source).await?;
    if grows { state.check_result_size(&mut tx).await?; } tx.commit().await?; Ok(task)
}
pub async fn cancel_due(conn: &mut SqliteConnection, task: i64) -> Result<()> {
    sqlx::query("UPDATE notification_deliveries SET state='canceled' WHERE state IN ('pending','retry','leased') AND notification_id IN (SELECT id FROM notifications WHERE task_id=? AND kind='due')").bind(task).execute(conn).await?; Ok(())
}
pub async fn archive_task(state: &AppState, actor: &User, id: i64, version: i64, action: &str, source: &str, interaction: Option<&str>) -> Result<Option<Task>> {
    let _guard = state.writes.lock().await; let mut tx = state.pool.begin().await?; let actor = current_actor(&mut tx,actor).await?; require_editor(&actor)?;
    let task = task_on(&mut tx,id).await?;
    if receipt(&mut tx,&actor,id,action,interaction).await? { return Ok(Some(task)); }
    if task.version!=version { return Err(Error::conflict()); }
    match action {
        "archive" => {
            if task.archived_at.is_some() { tx.commit().await?; return Ok(Some(task)); } editable(&task)?;
            sqlx::query("UPDATE tasks SET archived_at=?,archived_by=?,version=version+1,reminder_revision=reminder_revision+1,updated_at=? WHERE id=?").bind(stamp()).bind(actor.id).bind(stamp()).bind(id).execute(&mut *tx).await?;
        }
        "restore" => {
            if task.project_archived_at.is_some() { return Err(Error::bad("Restore the project first.")); }
            if task.archived_at.is_none() { tx.commit().await?; return Ok(Some(task)); }
            sqlx::query("UPDATE tasks SET archived_at=NULL,archived_by=NULL,version=version+1,reminder_revision=reminder_revision+1,updated_at=? WHERE id=?").bind(stamp()).bind(id).execute(&mut *tx).await?;
        }
        "purge" => {
            if task.archived_at.is_none() { return Err(Error::bad("Archive the task before permanently deleting it.")); }
            sqlx::query("INSERT INTO admin_audit_log(actor_id,action,target) VALUES(?,'purge_task',?)").bind(actor.id).bind(format!("TB-{id}")).execute(&mut *tx).await?;
            sqlx::query("DELETE FROM tasks WHERE id=?").bind(id).execute(&mut *tx).await?; tx.commit().await?; return Ok(None);
        }
        _ => return Err(Error::bad("Unknown action.")),
    }
    cancel_due(&mut tx,id).await?;
    event(&mut tx,&actor,&task,action,if action=="archive" {"moved the task to Archive"} else {"restored the task"},source).await?;
    let updated = task_on(&mut tx,id).await?; tx.commit().await?; Ok(Some(updated))
}
pub async fn watch(state: &AppState, actor: &User, id: i64, watching: bool) -> Result<()> {
    let _guard = state.writes.lock().await; let mut tx = state.pool.begin().await?; current_actor(&mut tx,actor).await?;
    let task = task_on(&mut tx,id).await?;
    if watching { editable(&task)?; state.check_capacity(&mut tx).await?; sqlx::query("INSERT OR IGNORE INTO task_watchers(task_id,user_id) VALUES(?,?)").bind(id).bind(actor.id).execute(&mut *tx).await?; state.check_result_size(&mut tx).await?; }
    else { sqlx::query("DELETE FROM task_watchers WHERE task_id=? AND user_id=?").bind(id).bind(actor.id).execute(&mut *tx).await?; }
    tx.commit().await?; Ok(())
}
pub async fn add_comment(state: &AppState, actor: &User, id: i64, body: &str) -> Result<()> {
    let body = text(body,"Comment",100_000,true)?;
    let _guard = state.writes.lock().await; let mut tx = state.pool.begin().await?; let actor = current_actor(&mut tx,actor).await?; require_editor(&actor)?;
    state.check_capacity(&mut tx).await?; let task = task_on(&mut tx,id).await?; editable(&task)?;
    sqlx::query("INSERT INTO comments(task_id,author_id,body) VALUES(?,?,?)").bind(id).bind(actor.id).bind(body).execute(&mut *tx).await?;
    event(&mut tx,&actor,&task,"commented","commented on the task","web").await?;
    state.check_result_size(&mut tx).await?; tx.commit().await?; Ok(())
}
pub async fn link(state: &AppState, actor: &User, id: i64, link: &str, label: &str) -> Result<()> {
    let url = url::Url::parse(link).map_err(|_| Error::bad("Enter a complete https:// link."))?;
    if !matches!(url.scheme(),"https"|"http") || !url.username().is_empty() || url.password().is_some() || link.len()>4000 { return Err(Error::bad("Use an HTTP or HTTPS link without credentials.")); }
    let label = text(label,"Link label",200,false)?;
    let _guard = state.writes.lock().await; let mut tx = state.pool.begin().await?; let actor = current_actor(&mut tx,actor).await?; require_editor(&actor)?; state.check_capacity(&mut tx).await?;
    let task = task_on(&mut tx,id).await?; editable(&task)?;
    sqlx::query("INSERT INTO task_links(task_id,url,label) VALUES(?,?,?)").bind(id).bind(url.as_str()).bind(label).execute(&mut *tx).await?;
    event(&mut tx,&actor,&task,"linked","added a link","web").await?; state.check_result_size(&mut tx).await?; tx.commit().await?; Ok(())
}
