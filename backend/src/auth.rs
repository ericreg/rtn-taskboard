use crate::{error::{Error, Result}, models::User, state::AppState};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use axum::{extract::FromRequestParts, http::{request::Parts, header, Method}};
use rand::{RngCore, rngs::OsRng};
use sha2::{Digest, Sha256};
use sqlx::Row;

pub const USER_COLUMNS: &str = "id,email,name,role,active,theme,timezone,activity_in_app,activity_discord,due_in_app,due_discord,watched_due";
#[derive(Clone)]
pub struct Auth { pub user: User, pub csrf: String, pub token_hash: String }
impl FromRequestParts<AppState> for Auth {
    type Rejection = Error;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self> {
        let raw = parts.headers.get(header::COOKIE).and_then(|h| h.to_str().ok()).and_then(|cookie| cookie.split(';').find_map(|p| p.trim().strip_prefix("taskboard_session="))).ok_or_else(Error::unauthorized)?;
        if raw.len() != 64 { return Err(Error::unauthorized()); }
        let token_hash = token_hash(raw);
        let row = sqlx::query("SELECT user_id,csrf FROM sessions WHERE token_hash=? AND expires_at>?").bind(&token_hash).bind(now()).fetch_optional(&state.pool).await?.ok_or_else(Error::unauthorized)?;
        let user = get_user(state, row.get("user_id")).await?;
        if !user.active { return Err(Error::unauthorized()); }
        let csrf: String = row.get("csrf");
        if !matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS) {
            let supplied = parts.headers.get("x-csrf-token").and_then(|h| h.to_str().ok()).unwrap_or("");
            if supplied != csrf { return Err(Error::forbidden()); }
        }
        Ok(Auth { user, csrf, token_hash })
    }
}
pub fn now() -> i64 { chrono::Utc::now().timestamp() }
pub fn stamp() -> String { chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true) }
pub fn random_token() -> String { let mut bytes = [0u8;32]; OsRng.fill_bytes(&mut bytes); bytes.iter().map(|b| format!("{b:02x}")).collect() }
pub fn token_hash(token: &str) -> String { format!("{:x}", Sha256::digest(token.as_bytes())) }
pub fn normalize_email(email: &str) -> Result<String> {
    let email = email.trim().to_ascii_lowercase();
    if email.len() > 254 || email.contains(char::is_whitespace) || !email.split_once('@').is_some_and(|(a,b)| !a.is_empty() && b.contains('.') && !b.contains('@')) { return Err(Error::bad("Enter a valid email address.")); }
    Ok(email)
}
pub fn valid_name(name: &str) -> Result<String> {
    let name = name.trim(); if name.is_empty() || name.chars().count() > 100 { return Err(Error::bad("Name must be between 1 and 100 characters.")); } Ok(name.into())
}
pub async fn hash_password(state: &AppState, password: &str) -> Result<String> {
    if !(15..=1024).contains(&password.chars().count()) { return Err(Error::bad("Use a password with 15–1024 characters.")); }
    let password = password.to_owned();
    let permit = state.hashes.clone().acquire_owned().await.map_err(|_| Error::bad("Please try again."))?;
    tokio::task::spawn_blocking(move || { let _permit = permit; let salt = SaltString::generate(&mut OsRng); Argon2::default().hash_password(password.as_bytes(), &salt).map(|h| h.to_string()).map_err(|_| Error::bad("Could not protect the password.")) }).await.map_err(|_| Error::bad("Please try again."))?
}
pub async fn verify_password(state: &AppState, password: String, hash: String) -> bool {
    if password.chars().count() > 1024 { return false; }
    let Ok(permit) = state.hashes.clone().acquire_owned().await else { return false; };
    tokio::task::spawn_blocking(move || { let _permit = permit; PasswordHash::new(&hash).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok()) }).await.unwrap_or(false)
}
pub async fn get_user(state: &AppState, id: i64) -> Result<User> {
    sqlx::query_as::<_,User>(&format!("SELECT {USER_COLUMNS} FROM users WHERE id=?")).bind(id).fetch_optional(&state.pool).await?.ok_or_else(Error::missing)
}
pub async fn bootstrap(state: &AppState, email: &str, name: &str, password: &str) -> Result<()> {
    let email = normalize_email(email)?; let name = valid_name(name)?;
    let hash = hash_password(state, password).await?;
    let _guard = state.writes.lock().await; let mut tx = state.pool.begin().await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role='editor'").fetch_one(&mut *tx).await?;
    if count > 0 { return Err(Error::bad("An editor already exists. Use Workspace management to invite more people.")); }
    sqlx::query("INSERT INTO users(email,password_hash,name,role,timezone) VALUES(?,?,?,'editor','UTC')").bind(email).bind(hash).bind(name).execute(&mut *tx).await?;
    tx.commit().await?; Ok(())
}
pub fn session_cookie(state: &AppState, token: &str, expired: bool) -> String {
    format!("taskboard_session={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}", if expired { 0 } else { 604800 }, if state.config.secure_cookies { "; Secure" } else { "" })
}
pub async fn new_session(state: &AppState, user_id: i64) -> Result<(String,String)> {
    let token = random_token(); let csrf = random_token();
    sqlx::query("INSERT INTO sessions(token_hash,user_id,csrf,expires_at) VALUES(?,?,?,?)").bind(token_hash(&token)).bind(user_id).bind(&csrf).bind(now()+604800).execute(&state.pool).await?;
    Ok((token,csrf))
}
