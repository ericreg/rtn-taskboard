use crate::db;
use crate::{
    auth::{Auth, stamp},
    error::{Error, Result},
    models::{Attachment, Project},
    services,
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;
use serde_json::{Value, json};

pub async fn list(State(s): State<AppState>, _auth: Auth) -> Result<Json<Vec<Project>>> {
    Ok(Json(db::all_as(&s.db.connect().await?, "SELECT p.*,(SELECT COUNT(*) FROM tasks WHERE project_id=p.id AND archived_at IS NULL) AS task_count,(SELECT COUNT(*) FROM tasks WHERE project_id=p.id AND archived_at IS NULL AND status='done') AS done_count FROM projects p ORDER BY p.archived_at IS NOT NULL,p.name COLLATE NOCASE", ()).await?))
}
#[derive(Deserialize)]
pub struct Input {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default = "color")]
    color: String,
    version: Option<i64>,
}
fn color() -> String {
    "#5e6ad2".into()
}
fn validate(input: &Input) -> Result<()> {
    services::text(&input.name, "Project name", 100, true)?;
    services::text(&input.description, "Description", 500_000, false)?;
    if input.color.len() != 7
        || !input.color.starts_with('#')
        || !input.color[1..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(Error::bad("Choose a valid project color."));
    }
    Ok(())
}
pub async fn create(
    State(s): State<AppState>,
    auth: Auth,
    Json(input): Json<Input>,
) -> Result<Json<Project>> {
    validate(&input)?;
    let _guard = s.writes.lock().await;
    let mut conn = s.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    let actor = services::current_actor(&tx, &auth.user).await?;
    services::require_editor(&actor)?;
    s.check_capacity(&tx).await?;
    let slug_base: String = input
        .name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = format!(
        "{}-{}",
        slug_base.trim_matches('-'),
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let id = db::execute(
        &tx,
        "INSERT INTO projects(name,slug,description,color,creator_id) VALUES(?,?,?,?,?)",
        turso::params![
            input.name.trim(),
            slug,
            input.description,
            input.color,
            actor.id
        ],
    )
    .await?
    .last_insert_rowid();
    s.check_result_size(&tx).await?;
    let project = db::one_as(&tx, "SELECT * FROM projects WHERE id=?", turso::params![id]).await?;
    tx.commit().await?;
    Ok(Json(project))
}
pub async fn detail(
    State(s): State<AppState>,
    _auth: Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>> {
    let project = db::optional_as::<Project>(
        &s.db.connect().await?,
        "SELECT * FROM projects WHERE id=?",
        turso::params![id],
    )
    .await?
    .ok_or_else(Error::missing)?;
    let attachments:Vec<Attachment>=db::all_as(&s.db.connect().await?, "SELECT id,filename,mime,size,created_at FROM attachments WHERE project_id=? ORDER BY created_at", turso::params![id]).await?;
    let events=db::all(&s.db.connect().await?, "SELECT e.*,u.name AS actor_name FROM activity_events e JOIN users u ON u.id=e.actor_id WHERE e.project_id=? ORDER BY e.id DESC LIMIT 100", turso::params![id]).await?.into_iter().map(|r|json!({"id":r.get::<i64>("id"),"task_id":r.get::<Option<i64>>("task_id"),"actor_name":r.get::<String>("actor_name"),"detail":r.get::<String>("detail"),"created_at":r.get::<String>("created_at")})).collect::<Vec<_>>();
    Ok(Json(
        json!({"project":project,"attachments":attachments,"activity":events}),
    ))
}
pub async fn update(
    State(s): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(input): Json<Input>,
) -> Result<Json<Project>> {
    validate(&input)?;
    let _guard = s.writes.lock().await;
    let mut conn = s.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    let actor = services::current_actor(&tx, &auth.user).await?;
    services::require_editor(&actor)?;
    let old = services::active_project(&tx, id).await?;
    if input.version != Some(old.version) {
        return Err(Error::conflict());
    }
    let grows = input.name.len() + input.description.len() > old.name.len() + old.description.len();
    if grows {
        s.check_capacity(&tx).await?;
    }
    db::execute(&tx, "UPDATE projects SET name=?,description=?,color=?,version=version+1,updated_at=? WHERE id=?", turso::params![input.name.trim(), input.description, input.color, stamp(), id]).await?;
    if grows {
        s.check_result_size(&tx).await?;
    }
    let p = db::one_as(&tx, "SELECT * FROM projects WHERE id=?", turso::params![id]).await?;
    tx.commit().await?;
    Ok(Json(p))
}
#[derive(Deserialize)]
pub struct Version {
    pub version: i64,
}
pub async fn archive(
    State(s): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(v): Json<Version>,
) -> Result<Json<Value>> {
    change(&s, &auth, id, v.version, true).await
}
pub async fn restore(
    State(s): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(v): Json<Version>,
) -> Result<Json<Value>> {
    change(&s, &auth, id, v.version, false).await
}
async fn change(
    s: &AppState,
    auth: &Auth,
    id: i64,
    version: i64,
    archive: bool,
) -> Result<Json<Value>> {
    let _guard = s.writes.lock().await;
    let mut conn = s.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    let actor = services::current_actor(&tx, &auth.user).await?;
    let p =
        db::optional_as::<Project>(&tx, "SELECT * FROM projects WHERE id=?", turso::params![id])
            .await?
            .ok_or_else(Error::missing)?;
    services::require_editor(&actor)?;
    if p.version != version {
        return Err(Error::conflict());
    }
    if p.archived_at.is_some() != archive {
        db::execute(
            &tx,
            "UPDATE projects SET archived_at=?,version=version+1,updated_at=? WHERE id=?",
            turso::params![if archive { Some(stamp()) } else { None }, stamp(), id],
        )
        .await?;
        db::execute(
            &tx,
            "UPDATE tasks SET reminder_revision=reminder_revision+1 WHERE project_id=?",
            turso::params![id],
        )
        .await?;
        db::execute(&tx, "UPDATE notification_deliveries SET state='canceled' WHERE state IN ('pending','retry','leased') AND notification_id IN (SELECT n.id FROM notifications n JOIN tasks t ON t.id=n.task_id WHERE t.project_id=? AND n.kind='due')", turso::params![id]).await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
