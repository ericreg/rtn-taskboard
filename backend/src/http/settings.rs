use crate::db;
use crate::{
    auth::{self, Auth, now, stamp},
    error::{Error, Result},
    services,
    state::AppState,
};
use axum::{
    Json,
    extract::{Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};

use std::sync::atomic::Ordering;

pub async fn status(State(s): State<AppState>, _auth: Auth) -> Result<Json<Value>> {
    Ok(Json(json!(s.storage().await?)))
}
#[derive(Deserialize)]
pub struct Settings {
    max_db_bytes: u64,
}
pub async fn update(
    State(s): State<AppState>,
    auth: Auth,
    Json(input): Json<Settings>,
) -> Result<Json<Value>> {
    if input.max_db_bytes > 9_007_199_254_740_991 {
        return Err(Error::bad("The threshold is out of range."));
    }
    let _guard = s.writes.lock().await;
    let mut conn = s.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    let actor = services::current_actor(&tx, &auth.user).await?;
    services::require_editor(&actor)?;
    db::execute(
        &tx,
        "UPDATE app_settings SET value=? WHERE key='max_db_bytes'",
        turso::params![input.max_db_bytes.to_string()],
    )
    .await?;
    db::execute(
        &tx,
        "INSERT INTO admin_audit_log(actor_id,action,target) VALUES(?,'storage_limit',?)",
        turso::params![actor.id, input.max_db_bytes.to_string()],
    )
    .await?;
    tx.commit().await?;
    s.max_db_bytes.store(input.max_db_bytes, Ordering::Relaxed);
    Ok(Json(json!(s.storage().await?)))
}
#[derive(Deserialize)]
pub struct Page {
    page: Option<i64>,
}
pub async fn notifications(
    State(s): State<AppState>,
    auth: Auth,
    Query(p): Query<Page>,
) -> Result<Json<Value>> {
    let page = p.page.unwrap_or(1).clamp(1, 1_000_000);
    let unread: i64 = db::scalar(
        &s.db.connect().await?,
        "SELECT COUNT(*) FROM notifications WHERE user_id=? AND in_app=1 AND read_at IS NULL",
        turso::params![auth.user.id],
    )
    .await?;
    let mut items=db::all(&s.db.connect().await?, "SELECT * FROM notifications WHERE user_id=? AND in_app=1 ORDER BY id DESC LIMIT 51 OFFSET ?", turso::params![auth.user.id, (page-1)*50]).await?.into_iter().map(|r|json!({"id":r.get::<i64>("id"),"task_id":r.get::<i64>("task_id"),"kind":r.get::<String>("kind"),"message":r.get::<String>("message"),"read_at":r.get::<Option<String>>("read_at"),"created_at":r.get::<String>("created_at")})).collect::<Vec<_>>();
    let has_more = items.len() > 50;
    items.truncate(50);
    Ok(Json(
        json!({"items":items,"unread":unread,"has_more":has_more,"page":page}),
    ))
}
#[derive(Deserialize)]
pub struct Read {
    id: Option<i64>,
}
pub async fn mark_read(
    State(s): State<AppState>,
    auth: Auth,
    Json(input): Json<Read>,
) -> Result<Json<Value>> {
    db::execute(&s.db.connect().await?, "UPDATE notifications SET read_at=? WHERE user_id=? AND read_at IS NULL AND (? IS NULL OR id=?)", turso::params![stamp(), auth.user.id, input.id, input.id]).await?;
    Ok(Json(json!({"ok":true})))
}
pub async fn deliveries(State(s): State<AppState>, auth: Auth) -> Result<Json<Value>> {
    services::require_editor(&auth.user)?;
    let items=db::all(&s.db.connect().await?, "SELECT d.id,d.state,d.attempts,d.last_error,n.message,u.name FROM notification_deliveries d JOIN notifications n ON n.id=d.notification_id JOIN users u ON u.id=n.user_id WHERE d.state IN ('failed','retry') ORDER BY d.id DESC LIMIT 100", ()).await?.into_iter().map(|r|json!({"id":r.get::<i64>("id"),"state":r.get::<String>("state"),"attempts":r.get::<i64>("attempts"),"error":r.get::<Option<String>>("last_error"),"message":r.get::<String>("message"),"recipient":r.get::<String>("name")})).collect::<Vec<_>>();
    Ok(Json(json!(items)))
}
pub async fn retry(State(s): State<AppState>, auth: Auth) -> Result<Json<Value>> {
    services::require_editor(&auth.user)?;
    db::execute(&s.db.connect().await?, "UPDATE notification_deliveries SET state='pending',attempts=0,next_attempt=0,last_error=NULL WHERE state='failed'", ()).await?;
    Ok(Json(json!({"ok":true})))
}
pub async fn link_token(State(s): State<AppState>, auth: Auth) -> Result<Json<Value>> {
    if s.config.discord_token.is_none() {
        return Err(Error::bad("The Discord bot has not been configured yet."));
    }
    s.rate_limit(format!("link:{}", auth.user.id), 10).await?;
    let code = auth::random_token();
    let expires = now() + 600;
    db::execute(&s.db.connect().await?, "INSERT INTO discord_link_tokens(token_hash,user_id,expires_at) VALUES(?,?,?) ON CONFLICT(user_id) DO UPDATE SET token_hash=excluded.token_hash,expires_at=excluded.expires_at,discord_id=NULL,discord_name=NULL", turso::params![auth::token_hash(code.as_str()), auth.user.id, expires]).await?;
    Ok(Json(json!({"code":code,"expires_at":expires})))
}
#[derive(Deserialize)]
pub struct Confirm {
    discord_id: String,
}
pub async fn confirm_link(
    State(s): State<AppState>,
    auth: Auth,
    Json(input): Json<Confirm>,
) -> Result<Json<Value>> {
    let _guard = s.writes.lock().await;
    let mut conn = s.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    services::current_actor(&tx, &auth.user).await?;
    let row=db::optional(&tx, "SELECT discord_id,discord_name FROM discord_link_tokens WHERE user_id=? AND expires_at>? AND discord_id=?", turso::params![auth.user.id, now(), input.discord_id.as_str()]).await?.ok_or_else(||Error::bad("This link request expired. Start again."))?;
    db::execute(&tx, "INSERT INTO discord_accounts(user_id,discord_id,discord_name) VALUES(?,?,?) ON CONFLICT(user_id) DO UPDATE SET discord_id=excluded.discord_id,discord_name=excluded.discord_name,linked_at=?", turso::params![auth.user.id, row.get::<String>("discord_id"), row.get::<String>("discord_name"), stamp()]).await?;
    db::execute(
        &tx,
        "DELETE FROM discord_link_tokens WHERE user_id=?",
        turso::params![auth.user.id],
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
pub async fn unlink_discord(State(s): State<AppState>, auth: Auth) -> Result<Json<Value>> {
    let _guard = s.writes.lock().await;
    let mut conn = s.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    db::execute(
        &tx,
        "DELETE FROM discord_accounts WHERE user_id=?",
        turso::params![auth.user.id],
    )
    .await?;
    db::execute(
        &tx,
        "DELETE FROM discord_link_tokens WHERE user_id=?",
        turso::params![auth.user.id],
    )
    .await?;
    db::execute(&tx, "UPDATE notification_deliveries SET state='canceled' WHERE state IN ('pending','retry','leased') AND notification_id IN (SELECT id FROM notifications WHERE user_id=?)", turso::params![auth.user.id]).await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
