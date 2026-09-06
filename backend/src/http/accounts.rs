use crate::db;
use crate::{
    auth::{self, Auth, USER_COLUMNS, now, stamp, token_hash},
    error::{Error, Result},
    models::User,
    services,
    state::AppState,
};
use axum::{
    Extension, Json,
    extract::{ConnectInfo, Path, State},
    http::header,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};

use std::net::SocketAddr;

pub async fn setup(State(s): State<AppState>) -> Result<Json<Value>> {
    let count: i64 = db::scalar(
        &s.db.connect().await?,
        "SELECT COUNT(*) FROM users WHERE role='editor'",
        (),
    )
    .await?;
    Ok(Json(json!({"bootstrap_required":count==0})))
}
#[derive(Deserialize)]
pub struct Credentials {
    email: String,
    password: String,
}
pub async fn login(
    State(s): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    Json(input): Json<Credentials>,
) -> Result<Response> {
    let email = auth::normalize_email(&input.email)?;
    s.rate_limit(format!("login-email:{}", token_hash(&email)), 20)
        .await?;
    let ip = peer
        .map(|p| p.0.0.ip().to_string())
        .unwrap_or_else(|| "local".into());
    s.rate_limit(format!("login-ip:{ip}"), 100).await?;
    let row = db::optional(
        &s.db.connect().await?,
        "SELECT id,password_hash,active FROM users WHERE email=?",
        turso::params![email.as_str()],
    )
    .await?;
    let Some(row) = row else {
        // Perform the same expensive hash work for unknown accounts.
        let _ = auth::hash_password(&s, "not-a-real-account-password").await;
        return Err(Error(
            axum::http::StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "Email or password is incorrect.".into(),
        ));
    };
    if !auth::verify_password(&s, input.password, row.get("password_hash")).await
        || !row.get::<bool>("active")
    {
        return Err(Error(
            axum::http::StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "Email or password is incorrect.".into(),
        ));
    }
    let _guard = s.writes.lock().await;
    let user = auth::get_user(&s, row.get("id")).await?;
    if !user.active {
        return Err(Error::unauthorized());
    }
    let (token, csrf) = auth::new_session(&s, user.id).await?;
    Ok((
        [(header::SET_COOKIE, auth::session_cookie(&s, &token, false))],
        Json(json!({"user":user,"csrf":csrf})),
    )
        .into_response())
}
#[derive(Deserialize)]
pub struct Registration {
    email: String,
    name: String,
    password: String,
    token: String,
}
pub async fn register(
    State(s): State<AppState>,
    Json(input): Json<Registration>,
) -> Result<Json<Value>> {
    let email = auth::normalize_email(&input.email)?;
    let name = auth::valid_name(&input.name)?;
    s.rate_limit(format!("register:{}", token_hash(&email)), 10)
        .await?;
    let key = token_hash(&input.token);
    let valid: i64 = db::scalar(&s.db.connect().await?, "SELECT COUNT(*) FROM account_tokens WHERE token_hash=? AND email=? AND purpose='invite' AND used_at IS NULL AND expires_at>?", turso::params![key.as_str(), email.as_str(), now()]).await?;
    if valid == 0 {
        return Err(Error::bad("This invitation is invalid or expired."));
    }
    let hash = auth::hash_password(&s, &input.password).await?;
    let _guard = s.writes.lock().await;
    let mut conn = s.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    s.check_capacity(&tx).await?;
    let used = db::execute(&tx, "UPDATE account_tokens SET used_at=? WHERE token_hash=? AND email=? AND purpose='invite' AND used_at IS NULL AND expires_at>?", turso::params![now(), key, email.as_str(), now()]).await?.rows_affected();
    if used != 1 {
        return Err(Error::bad("This invitation is invalid or expired."));
    }
    db::execute(
        &tx,
        "INSERT INTO users(email,password_hash,name,timezone) VALUES(?,?,?,'UTC')",
        turso::params![email, hash, name],
    )
    .await?;
    s.check_result_size(&tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
pub struct Reset {
    token: String,
    password: String,
}
pub async fn reset(State(s): State<AppState>, Json(input): Json<Reset>) -> Result<Json<Value>> {
    s.rate_limit(format!("reset:{}", token_hash(&input.token)), 10)
        .await?;
    let key = token_hash(&input.token);
    let email: Option<String> = db::optional_scalar(&s.db.connect().await?, "SELECT email FROM account_tokens WHERE token_hash=? AND purpose='reset' AND used_at IS NULL AND expires_at>?", turso::params![key.as_str(), now()]).await?;
    let email = email.ok_or_else(|| Error::bad("This reset link is invalid or expired."))?;
    let hash = auth::hash_password(&s, &input.password).await?;
    let _guard = s.writes.lock().await;
    let mut conn = s.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    let used = db::execute(&tx, "UPDATE account_tokens SET used_at=? WHERE token_hash=? AND used_at IS NULL AND expires_at>?", turso::params![now(), key, now()]).await?.rows_affected();
    if used != 1 {
        return Err(Error::bad("This reset link is invalid or expired."));
    }
    db::execute(
        &tx,
        "UPDATE users SET password_hash=? WHERE email=? AND active=1",
        turso::params![hash, email.as_str()],
    )
    .await?;
    db::execute(
        &tx,
        "DELETE FROM sessions WHERE user_id IN (SELECT id FROM users WHERE email=?)",
        turso::params![email.as_str()],
    )
    .await?;
    db::execute(
        &tx,
        "UPDATE account_tokens SET used_at=? WHERE email=? AND purpose='reset'",
        turso::params![now(), email],
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
pub async fn me(State(s): State<AppState>, auth: Auth) -> Result<Json<Value>> {
    let discord = db::optional(
        &s.db.connect().await?,
        "SELECT discord_id,discord_name FROM discord_accounts WHERE user_id=?",
        turso::params![auth.user.id],
    )
    .await?
    .map(|r| json!({"id":r.get::<String>("discord_id"),"name":r.get::<String>("discord_name")}));
    let pending = db::optional(&s.db.connect().await?, "SELECT discord_id,discord_name FROM discord_link_tokens WHERE user_id=? AND expires_at>? AND discord_id IS NOT NULL", turso::params![auth.user.id, now()]).await?.map(|r| json!({"id":r.get::<String>("discord_id"),"name":r.get::<String>("discord_name")}));
    Ok(Json(
        json!({"user":auth.user,"csrf":auth.csrf,"discord":discord,"pending_discord":pending}),
    ))
}
pub async fn logout(State(s): State<AppState>, auth: Auth) -> Result<Response> {
    db::execute(
        &s.db.connect().await?,
        "DELETE FROM sessions WHERE token_hash=?",
        turso::params![auth.token_hash],
    )
    .await?;
    Ok((
        [(header::SET_COOKIE, auth::session_cookie(&s, "", true))],
        Json(json!({"ok":true})),
    )
        .into_response())
}
pub async fn users(State(s): State<AppState>, _auth: Auth) -> Result<Json<Vec<User>>> {
    Ok(Json(
        db::all_as(
            &s.db.connect().await?,
            &format!("SELECT {USER_COLUMNS} FROM users ORDER BY active DESC,name COLLATE NOCASE"),
            (),
        )
        .await?,
    ))
}
#[derive(Deserialize)]
pub struct Profile {
    name: String,
    theme: String,
    timezone: String,
    activity_in_app: bool,
    activity_discord: bool,
    due_in_app: bool,
    due_discord: bool,
    watched_due: bool,
}
pub async fn profile(
    State(s): State<AppState>,
    auth: Auth,
    Json(input): Json<Profile>,
) -> Result<Json<User>> {
    let name = auth::valid_name(&input.name)?;
    if !["light", "dark", "system"].contains(&input.theme.as_str())
        || input.timezone.parse::<chrono_tz::Tz>().is_err()
    {
        return Err(Error::bad("Invalid appearance or time zone."));
    }
    let _guard = s.writes.lock().await;
    db::execute(&s.db.connect().await?, "UPDATE users SET name=?,theme=?,timezone=?,activity_in_app=?,activity_discord=?,due_in_app=?,due_discord=?,watched_due=? WHERE id=? AND active=1", turso::params![name, input.theme, input.timezone, input.activity_in_app, input.activity_discord, input.due_in_app, input.due_discord, input.watched_due, auth.user.id]).await?;
    Ok(Json(auth::get_user(&s, auth.user.id).await?))
}
#[derive(Deserialize)]
pub struct ChangePassword {
    current_password: String,
    password: String,
}
pub async fn change_password(
    State(s): State<AppState>,
    auth: Auth,
    Json(input): Json<ChangePassword>,
) -> Result<Json<Value>> {
    s.rate_limit(format!("password:{}", auth.user.id), 10)
        .await?;
    let old: String = db::scalar(
        &s.db.connect().await?,
        "SELECT password_hash FROM users WHERE id=?",
        turso::params![auth.user.id],
    )
    .await?;
    if !auth::verify_password(&s, input.current_password, old.clone()).await {
        return Err(Error::bad("Current password is incorrect."));
    }
    let hash = auth::hash_password(&s, &input.password).await?;
    let _guard = s.writes.lock().await;
    let mut conn = s.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    let changed = db::execute(
        &tx,
        "UPDATE users SET password_hash=? WHERE id=? AND password_hash=? AND active=1",
        turso::params![hash, auth.user.id, old],
    )
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(Error::conflict());
    }
    db::execute(
        &tx,
        "DELETE FROM sessions WHERE user_id=?",
        turso::params![auth.user.id],
    )
    .await?;
    db::execute(
        &tx,
        "UPDATE account_tokens SET used_at=? WHERE email=? AND purpose='reset'",
        turso::params![now(), auth.user.email.as_str()],
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
pub struct TokenInput {
    email: String,
    purpose: String,
}
pub async fn issue_token(
    State(s): State<AppState>,
    auth: Auth,
    Json(input): Json<TokenInput>,
) -> Result<Json<Value>> {
    let email = auth::normalize_email(&input.email)?;
    if !["invite", "reset"].contains(&input.purpose.as_str()) {
        return Err(Error::bad("Invalid token purpose."));
    }
    let _guard = s.writes.lock().await;
    let mut conn = s.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    let actor = services::current_actor(&tx, &auth.user).await?;
    services::require_editor(&actor)?;
    if input.purpose == "invite" {
        s.check_capacity(&tx).await?;
    }
    let existing: Option<i64> = db::optional_scalar(
        &tx,
        "SELECT id FROM users WHERE email=? AND active=1",
        turso::params![email.as_str()],
    )
    .await?;
    if (input.purpose == "reset" && existing.is_none())
        || (input.purpose == "invite" && existing.is_some())
    {
        return Err(Error::bad(
            "Use Invite for new employees and Reset for existing active accounts.",
        ));
    }
    let token = auth::random_token();
    let expires = now()
        + if input.purpose == "invite" {
            7 * 86400
        } else {
            1800
        };
    db::execute(
        &tx,
        "DELETE FROM account_tokens WHERE email=? AND purpose=?",
        turso::params![email.as_str(), input.purpose.as_str()],
    )
    .await?;
    db::execute(&tx, "INSERT INTO account_tokens(token_hash,email,purpose,expires_at,created_by) VALUES(?,?,?,?,?)", turso::params![token_hash(token.as_str()), email.as_str(), input.purpose.as_str(), expires, actor.id]).await?;
    if input.purpose == "invite" {
        s.check_result_size(&tx).await?;
    }
    tx.commit().await?;
    let route = if input.purpose == "invite" {
        "register"
    } else {
        "reset"
    };
    // Tokens live in the URL fragment so proxies and referrers do not receive them.
    let link = format!(
        "{}/{route}#token={}&email={}",
        s.config.base_url,
        token,
        url::form_urlencoded::byte_serialize(email.as_bytes()).collect::<String>()
    );
    Ok(Json(json!({"url":link,"expires_at":expires})))
}
#[derive(Deserialize)]
pub struct UserUpdate {
    role: String,
    active: bool,
}
pub async fn update_user(
    State(s): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(input): Json<UserUpdate>,
) -> Result<Json<Value>> {
    if !["viewer", "editor"].contains(&input.role.as_str()) {
        return Err(Error::bad("Invalid role."));
    }
    let _guard = s.writes.lock().await;
    let mut conn = s.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    let actor = services::current_actor(&tx, &auth.user).await?;
    services::require_editor(&actor)?;
    let old = db::optional_as::<User>(
        &tx,
        &format!("SELECT {USER_COLUMNS} FROM users WHERE id=?"),
        turso::params![id],
    )
    .await?
    .ok_or_else(Error::missing)?;
    if old.editor() && old.active && (!input.active || input.role != "editor") {
        let editors: i64 = db::scalar(
            &tx,
            "SELECT COUNT(*) FROM users WHERE active=1 AND role='editor'",
            (),
        )
        .await?;
        if editors <= 1 {
            return Err(Error::bad("Keep at least one active editor."));
        }
    }
    db::execute(
        &tx,
        "UPDATE users SET role=?,active=? WHERE id=?",
        turso::params![input.role.as_str(), input.active, id],
    )
    .await?;
    if !input.active {
        db::execute(
            &tx,
            "DELETE FROM sessions WHERE user_id=?",
            turso::params![id],
        )
        .await?;
        db::execute(
            &tx,
            "DELETE FROM discord_link_tokens WHERE user_id=?",
            turso::params![id],
        )
        .await?;
        db::execute(
            &tx,
            "UPDATE account_tokens SET used_at=? WHERE email=?",
            turso::params![now(), old.email],
        )
        .await?;
    }
    db::execute(&tx, "INSERT INTO admin_audit_log(actor_id,action,target,created_at) VALUES(?,'update_user',?,?)", turso::params![actor.id, format!("{id}:{}:{}",input.role,input.active), stamp()]).await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
