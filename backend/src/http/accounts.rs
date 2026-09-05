use crate::{auth::{self, Auth, now, stamp, token_hash, USER_COLUMNS}, error::{Error, Result}, models::User, services, state::AppState};
use axum::{extract::{State, Path, ConnectInfo}, Extension, Json, http::header, response::{IntoResponse, Response}};
use serde::Deserialize;
use serde_json::{Value,json};
use sqlx::Row;
use std::net::SocketAddr;

pub async fn setup(State(s): State<AppState>) -> Result<Json<Value>> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role='editor'").fetch_one(&s.pool).await?;
    Ok(Json(json!({"bootstrap_required":count==0})))
}
#[derive(Deserialize)]
pub struct Credentials { email: String, password: String }
pub async fn login(State(s): State<AppState>, peer: Option<Extension<ConnectInfo<SocketAddr>>>, Json(input): Json<Credentials>) -> Result<Response> {
    let email = auth::normalize_email(&input.email)?;
    s.rate_limit(format!("login-email:{}",token_hash(&email)),20).await?;
    let ip = peer.map(|p|p.0.0.ip().to_string()).unwrap_or_else(||"local".into());
    s.rate_limit(format!("login-ip:{ip}"),100).await?;
    let row = sqlx::query("SELECT id,password_hash,active FROM users WHERE email=?").bind(&email).fetch_optional(&s.pool).await?;
    let Some(row) = row else {
        // Perform the same expensive hash work for unknown accounts.
        let _ = auth::hash_password(&s,"not-a-real-account-password").await;
        return Err(Error(axum::http::StatusCode::UNAUTHORIZED,"invalid_credentials","Email or password is incorrect.".into()));
    };
    if !auth::verify_password(&s,input.password,row.get("password_hash")).await || !row.get::<bool,_>("active") { return Err(Error(axum::http::StatusCode::UNAUTHORIZED,"invalid_credentials","Email or password is incorrect.".into())); }
    let _guard = s.writes.lock().await;
    let user = auth::get_user(&s,row.get("id")).await?; if !user.active { return Err(Error::unauthorized()); }
    let (token,csrf) = auth::new_session(&s,user.id).await?;
    Ok(([(header::SET_COOKIE,auth::session_cookie(&s,&token,false))],Json(json!({"user":user,"csrf":csrf}))).into_response())
}
#[derive(Deserialize)]
pub struct Registration { email:String, name:String, password:String, token:String }
pub async fn register(State(s): State<AppState>, Json(input): Json<Registration>) -> Result<Json<Value>> {
    let email = auth::normalize_email(&input.email)?; let name = auth::valid_name(&input.name)?;
    s.rate_limit(format!("register:{}",token_hash(&email)),10).await?;
    let key = token_hash(&input.token);
    let valid: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM account_tokens WHERE token_hash=? AND email=? AND purpose='invite' AND used_at IS NULL AND expires_at>?").bind(&key).bind(&email).bind(now()).fetch_one(&s.pool).await?;
    if valid==0 { return Err(Error::bad("This invitation is invalid or expired.")); }
    let hash = auth::hash_password(&s,&input.password).await?;
    let _guard = s.writes.lock().await; let mut tx = s.pool.begin().await?; s.check_capacity(&mut tx).await?;
    let used = sqlx::query("UPDATE account_tokens SET used_at=? WHERE token_hash=? AND email=? AND purpose='invite' AND used_at IS NULL AND expires_at>?").bind(now()).bind(key).bind(&email).bind(now()).execute(&mut *tx).await?.rows_affected();
    if used!=1 { return Err(Error::bad("This invitation is invalid or expired.")); }
    sqlx::query("INSERT INTO users(email,password_hash,name,timezone) VALUES(?,?,?,'UTC')").bind(email).bind(hash).bind(name).execute(&mut *tx).await?;
    s.check_result_size(&mut tx).await?; tx.commit().await?; Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)] pub struct Reset { token:String, password:String }
pub async fn reset(State(s): State<AppState>, Json(input): Json<Reset>) -> Result<Json<Value>> {
    s.rate_limit(format!("reset:{}",token_hash(&input.token)),10).await?;
    let key = token_hash(&input.token);
    let email: Option<String> = sqlx::query_scalar("SELECT email FROM account_tokens WHERE token_hash=? AND purpose='reset' AND used_at IS NULL AND expires_at>?").bind(&key).bind(now()).fetch_optional(&s.pool).await?;
    let email = email.ok_or_else(||Error::bad("This reset link is invalid or expired."))?;
    let hash = auth::hash_password(&s,&input.password).await?;
    let _guard = s.writes.lock().await; let mut tx = s.pool.begin().await?;
    let used = sqlx::query("UPDATE account_tokens SET used_at=? WHERE token_hash=? AND used_at IS NULL AND expires_at>?").bind(now()).bind(key).bind(now()).execute(&mut *tx).await?.rows_affected();
    if used!=1 { return Err(Error::bad("This reset link is invalid or expired.")); }
    sqlx::query("UPDATE users SET password_hash=? WHERE email=? AND active=1").bind(hash).bind(&email).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM sessions WHERE user_id IN (SELECT id FROM users WHERE email=?)").bind(&email).execute(&mut *tx).await?;
    sqlx::query("UPDATE account_tokens SET used_at=? WHERE email=? AND purpose='reset'").bind(now()).bind(email).execute(&mut *tx).await?;
    tx.commit().await?; Ok(Json(json!({"ok":true})))
}
pub async fn me(State(s): State<AppState>, auth: Auth) -> Result<Json<Value>> {
    let discord = sqlx::query("SELECT discord_id,discord_name FROM discord_accounts WHERE user_id=?").bind(auth.user.id).fetch_optional(&s.pool).await?.map(|r| json!({"id":r.get::<String,_>("discord_id"),"name":r.get::<String,_>("discord_name")}));
    let pending = sqlx::query("SELECT discord_id,discord_name FROM discord_link_tokens WHERE user_id=? AND expires_at>? AND discord_id IS NOT NULL").bind(auth.user.id).bind(now()).fetch_optional(&s.pool).await?.map(|r| json!({"id":r.get::<String,_>("discord_id"),"name":r.get::<String,_>("discord_name")}));
    Ok(Json(json!({"user":auth.user,"csrf":auth.csrf,"discord":discord,"pending_discord":pending})))
}
pub async fn logout(State(s): State<AppState>, auth: Auth) -> Result<Response> {
    sqlx::query("DELETE FROM sessions WHERE token_hash=?").bind(auth.token_hash).execute(&s.pool).await?;
    Ok(([(header::SET_COOKIE,auth::session_cookie(&s,"",true))],Json(json!({"ok":true}))).into_response())
}
pub async fn users(State(s): State<AppState>, _auth: Auth) -> Result<Json<Vec<User>>> {
    Ok(Json(sqlx::query_as(&format!("SELECT {USER_COLUMNS} FROM users ORDER BY active DESC,name COLLATE NOCASE")).fetch_all(&s.pool).await?))
}
#[derive(Deserialize)] pub struct Profile { name:String, theme:String, timezone:String, activity_in_app:bool, activity_discord:bool, due_in_app:bool, due_discord:bool, watched_due:bool }
pub async fn profile(State(s): State<AppState>, auth: Auth, Json(input): Json<Profile>) -> Result<Json<User>> {
    let name = auth::valid_name(&input.name)?; if !["light","dark","system"].contains(&input.theme.as_str()) || input.timezone.parse::<chrono_tz::Tz>().is_err() { return Err(Error::bad("Invalid appearance or time zone.")); }
    let _guard = s.writes.lock().await;
    sqlx::query("UPDATE users SET name=?,theme=?,timezone=?,activity_in_app=?,activity_discord=?,due_in_app=?,due_discord=?,watched_due=? WHERE id=? AND active=1").bind(name).bind(input.theme).bind(input.timezone).bind(input.activity_in_app).bind(input.activity_discord).bind(input.due_in_app).bind(input.due_discord).bind(input.watched_due).bind(auth.user.id).execute(&s.pool).await?;
    Ok(Json(auth::get_user(&s,auth.user.id).await?))
}
#[derive(Deserialize)] pub struct ChangePassword { current_password:String, password:String }
pub async fn change_password(State(s): State<AppState>, auth: Auth, Json(input): Json<ChangePassword>) -> Result<Json<Value>> {
    s.rate_limit(format!("password:{}",auth.user.id),10).await?;
    let old: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id=?").bind(auth.user.id).fetch_one(&s.pool).await?;
    if !auth::verify_password(&s,input.current_password,old.clone()).await { return Err(Error::bad("Current password is incorrect.")); }
    let hash = auth::hash_password(&s,&input.password).await?;
    let _guard = s.writes.lock().await; let mut tx = s.pool.begin().await?;
    let changed = sqlx::query("UPDATE users SET password_hash=? WHERE id=? AND password_hash=? AND active=1").bind(hash).bind(auth.user.id).bind(old).execute(&mut *tx).await?.rows_affected(); if changed!=1 { return Err(Error::conflict()); }
    sqlx::query("DELETE FROM sessions WHERE user_id=?").bind(auth.user.id).execute(&mut *tx).await?;
    sqlx::query("UPDATE account_tokens SET used_at=? WHERE email=? AND purpose='reset'").bind(now()).bind(&auth.user.email).execute(&mut *tx).await?;
    tx.commit().await?; Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)] pub struct TokenInput { email:String, purpose:String }
pub async fn issue_token(State(s): State<AppState>, auth: Auth, Json(input): Json<TokenInput>) -> Result<Json<Value>> {
    let email = auth::normalize_email(&input.email)?; if !["invite","reset"].contains(&input.purpose.as_str()) { return Err(Error::bad("Invalid token purpose.")); }
    let _guard = s.writes.lock().await; let mut tx = s.pool.begin().await?; let actor = services::current_actor(&mut tx,&auth.user).await?; services::require_editor(&actor)?;
    if input.purpose=="invite" { s.check_capacity(&mut tx).await?; }
    let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE email=? AND active=1").bind(&email).fetch_optional(&mut *tx).await?;
    if (input.purpose=="reset" && existing.is_none()) || (input.purpose=="invite" && existing.is_some()) { return Err(Error::bad("Use Invite for new employees and Reset for existing active accounts.")); }
    let token = auth::random_token(); let expires = now()+if input.purpose=="invite" { 7*86400 } else { 1800 };
    sqlx::query("DELETE FROM account_tokens WHERE email=? AND purpose=?").bind(&email).bind(&input.purpose).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO account_tokens(token_hash,email,purpose,expires_at,created_by) VALUES(?,?,?,?,?)").bind(token_hash(&token)).bind(&email).bind(&input.purpose).bind(expires).bind(actor.id).execute(&mut *tx).await?;
    if input.purpose=="invite" { s.check_result_size(&mut tx).await?; } tx.commit().await?;
    let route = if input.purpose=="invite" {"register"} else {"reset"};
    // Tokens live in the URL fragment so proxies and referrers do not receive them.
    let link = format!("{}/{route}#token={}&email={}",s.config.base_url,token,url::form_urlencoded::byte_serialize(email.as_bytes()).collect::<String>());
    Ok(Json(json!({"url":link,"expires_at":expires})))
}
#[derive(Deserialize)] pub struct UserUpdate { role:String, active:bool }
pub async fn update_user(State(s): State<AppState>, auth: Auth, Path(id): Path<i64>, Json(input): Json<UserUpdate>) -> Result<Json<Value>> {
    if !["viewer","editor"].contains(&input.role.as_str()) { return Err(Error::bad("Invalid role.")); }
    let _guard = s.writes.lock().await; let mut tx = s.pool.begin().await?; let actor = services::current_actor(&mut tx,&auth.user).await?; services::require_editor(&actor)?;
    let old = sqlx::query_as::<_,User>(&format!("SELECT {USER_COLUMNS} FROM users WHERE id=?")).bind(id).fetch_optional(&mut *tx).await?.ok_or_else(Error::missing)?;
    if old.editor() && old.active && (!input.active || input.role!="editor") {
        let editors: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE active=1 AND role='editor'").fetch_one(&mut *tx).await?;
        if editors<=1 { return Err(Error::bad("Keep at least one active editor.")); }
    }
    sqlx::query("UPDATE users SET role=?,active=? WHERE id=?").bind(&input.role).bind(input.active).bind(id).execute(&mut *tx).await?;
    if !input.active {
        sqlx::query("DELETE FROM sessions WHERE user_id=?").bind(id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM discord_link_tokens WHERE user_id=?").bind(id).execute(&mut *tx).await?;
        sqlx::query("UPDATE account_tokens SET used_at=? WHERE email=?").bind(now()).bind(old.email).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO admin_audit_log(actor_id,action,target,created_at) VALUES(?,'update_user',?,?)").bind(actor.id).bind(format!("{id}:{}:{}",input.role,input.active)).bind(stamp()).execute(&mut *tx).await?;
    tx.commit().await?; Ok(Json(json!({"ok":true})))
}
