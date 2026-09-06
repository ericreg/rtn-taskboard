mod accounts;
mod projects;
mod tasks;
mod media;
mod settings;

use crate::{error::Error, state::AppState};
use axum::{Router, extract::{DefaultBodyLimit, Request}, http::{HeaderValue, header}, middleware::{self, Next}, response::Response, routing::{get,post,patch,put,delete}};
use tower_http::trace::TraceLayer;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/auth/setup",get(accounts::setup))
        .route("/auth/login",post(accounts::login))
        .route("/auth/register",post(accounts::register))
        .route("/auth/reset-password",post(accounts::reset))
        .route("/auth/logout",post(accounts::logout))
        .route("/auth/me",get(accounts::me))
        .route("/users",get(accounts::users))
        .route("/me",patch(accounts::profile))
        .route("/me/change-password",post(accounts::change_password))
        .route("/admin/tokens",post(accounts::issue_token))
        .route("/admin/users/{id}",patch(accounts::update_user))
        .route("/projects",get(projects::list).post(projects::create))
        .route("/projects/{id}",get(projects::detail).patch(projects::update))
        .route("/projects/{id}/archive",post(projects::archive))
        .route("/projects/{id}/restore",post(projects::restore))
        .route("/projects/{id}/tasks",post(tasks::create))
        .route("/tasks",get(tasks::list))
        .route("/tasks/{id}",get(tasks::detail).patch(tasks::update).delete(tasks::archive))
        .route("/tasks/{id}/restore",post(tasks::restore))
        .route("/tasks/{id}/permanent",delete(tasks::purge))
        .route("/tasks/{id}/comments",post(tasks::comment))
        .route("/tasks/{id}/watchers/me",put(tasks::watch).delete(tasks::unwatch))
        .route("/tasks/{id}/links",post(tasks::link))
        .route("/tasks/{id}/links/{link}",delete(tasks::unlink))
        .route("/tasks/{id}/attachments",post(media::task_upload).layer(DefaultBodyLimit::disable()))
        .route("/projects/{id}/attachments",post(media::project_upload).layer(DefaultBodyLimit::disable()))
        .route("/attachments/{id}",get(media::download).delete(media::remove))
        .route("/notifications",get(settings::notifications))
        .route("/notifications/read",post(settings::mark_read))
        .route("/status",get(settings::status))
        .route("/admin/settings",patch(settings::update))
        .route("/admin/deliveries",get(settings::deliveries))
        .route("/admin/deliveries/retry",post(settings::retry))
        .route("/me/discord/link-token",post(settings::link_token))
        .route("/me/discord/confirm",post(settings::confirm_link))
        .route("/me/discord",delete(settings::unlink_discord))
        .fallback(|| async { Error::missing() });
    Router::new().nest("/api/v1",api)
        .layer(DefaultBodyLimit::max(1024*1024))
        .layer(middleware::from_fn(security))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
async fn security(request: Request, next: Next) -> Response {
    // Browser-origin checks belong to the enrolled gateway. User authentication,
    // session-bound CSRF tokens, and permissions are still enforced here.
    let api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("x-content-type-options",HeaderValue::from_static("nosniff"));
    headers.insert("referrer-policy",HeaderValue::from_static("same-origin"));
    headers.insert("x-frame-options",HeaderValue::from_static("DENY"));
    headers.insert("content-security-policy",HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' blob:; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'; object-src 'none'"));
    if api { headers.insert(header::CACHE_CONTROL,HeaderValue::from_static("no-store")); }
    response
}
