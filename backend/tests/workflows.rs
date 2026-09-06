use axum::{
    Json, Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use taskboard::db;
use taskboard::{
    auth,
    config::Config,
    http, jobs,
    services::{self, TaskInput},
    state::AppState,
};
use tower::ServiceExt;

struct Fixture {
    state: AppState,
    admin: Session,
    _dir: tempfile::TempDir,
}

#[tokio::test]
async fn turso_task_filters_bind_literals_and_paginate_together() {
    use taskboard::models::TaskFilter;
    let f = Fixture::new().await;
    let project = f.project().await;
    let actor = auth::get_user(&f.state, f.admin.id).await.unwrap();
    let due = (chrono::Utc::now().date_naive() + chrono::Days::new(2)).to_string();
    for title in [
        r"literal %_\ first",
        r"literal %_\ second",
        "literal wildcard distractor",
    ] {
        services::create_task(
            &f.state,
            &actor,
            project,
            TaskInput {
                title: title.into(),
                description: String::new(),
                status: "todo".into(),
                assignee_id: Some(actor.id),
                due_date: Some(due.clone()),
                version: None,
            },
        )
        .await
        .unwrap();
    }
    let mut filter = TaskFilter {
        project: Some(project),
        assignee: Some(actor.id),
        status: Some("todo".into()),
        q: Some(r"%_\".into()),
        due: Some("soon".into()),
        limit: Some(1),
        ..Default::default()
    };
    let first = services::list_tasks(&f.state, &filter).await.unwrap();
    assert_eq!(first["items"][0]["title"], r"literal %_\ second");
    assert_eq!(first["has_more"], true);
    filter.page = Some(2);
    let second = services::list_tasks(&f.state, &filter).await.unwrap();
    assert_eq!(second["items"][0]["title"], r"literal %_\ first");
    assert_eq!(second["has_more"], false);
    filter.q = Some("' OR 1=1 --".into());
    assert_eq!(
        services::list_tasks(&f.state, &filter).await.unwrap()["items"],
        json!([])
    );
}
#[derive(Clone)]
struct Session {
    cookie: String,
    csrf: String,
    id: i64,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(Config {
            database: dir.path().join("taskboard.db"),
            base_url: "http://localhost:8080".into(),
            max_db_bytes: 0,
            max_image_bytes: 10 * 1024 * 1024,
            secure_cookies: false,
            discord_token: None,
            discord_guild: None,
            rtn_relay_only: false,
        })
        .await
        .unwrap();
        auth::seed(
            &state,
            "admin@example.test",
            "Alex",
            "a long test-only passphrase",
        )
        .await
        .unwrap();
        let (token, csrf) = auth::new_session(&state, 1).await.unwrap();
        Self {
            state,
            admin: Session {
                cookie: format!("taskboard_session={token}"),
                csrf,
                id: 1,
            },
            _dir: dir,
        }
    }
    async fn project(&self) -> i64 {
        let (status, p, _) = send(
            &self.state,
            "POST",
            "/projects",
            Some(&self.admin),
            Some(json!({"name":"Launch","description":"Project context"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{p}");
        p["id"].as_i64().unwrap()
    }
    async fn task(&self, project: i64) -> Value {
        let (status,t,_)=send(&self.state,"POST",&format!("/projects/{project}/tasks"),Some(&self.admin),Some(json!({"title":"Ship a useful thing","description":"## Context\nA task with a project.","assignee_id":1}))).await;
        assert_eq!(status, StatusCode::OK, "{t}");
        t
    }
    async fn member(&self) -> Session {
        let (_, invite, _) = send(
            &self.state,
            "POST",
            "/admin/tokens",
            Some(&self.admin),
            Some(json!({"email":"member@example.test","purpose":"invite"})),
        )
        .await;
        let url = url::Url::parse(invite["url"].as_str().unwrap()).unwrap();
        let token = url::form_urlencoded::parse(url.fragment().unwrap().as_bytes())
            .find(|(k, _)| k == "token")
            .unwrap()
            .1
            .into_owned();
        let (status,body,_)=send(&self.state,"POST","/auth/register",None,Some(json!({"email":"member@example.test","name":"Jamie","password":"another long test passphrase","token":token}))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let id: i64 = db::scalar(
            &self.state.db.connect().await.unwrap(),
            "SELECT id FROM users WHERE email='member@example.test'",
            (),
        )
        .await
        .unwrap();
        let (token, csrf) = auth::new_session(&self.state, id).await.unwrap();
        Session {
            cookie: format!("taskboard_session={token}"),
            csrf,
            id,
        }
    }
}
async fn send(
    state: &AppState,
    method: &str,
    path: &str,
    session: Option<&Session>,
    body: Option<Value>,
) -> (StatusCode, Value, HeaderMap) {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("/api/v1{path}"))
        .header("Origin", "http://localhost:8080");
    if let Some(s) = session {
        request = request
            .header("Cookie", &s.cookie)
            .header("X-CSRF-Token", &s.csrf);
    }
    if body.is_some() {
        request = request.header("Content-Type", "application/json");
    }
    let response = http::router(state.clone())
        .oneshot(
            request
                .body(
                    body.map(|v| Body::from(v.to_string()))
                        .unwrap_or_else(Body::empty),
                )
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({"raw":String::from_utf8_lossy(&bytes)}));
    (status, value, headers)
}
async fn image(
    state: &AppState,
    session: &Session,
    task: i64,
    bytes: &[u8],
) -> (StatusCode, Value) {
    let mut body=b"--image-boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"example.png\"\r\nContent-Type: image/png\r\n\r\n".to_vec();
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n--image-boundary--\r\n");
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/tasks/{task}/attachments"))
        .header(
            "Content-Type",
            "multipart/form-data; boundary=image-boundary",
        )
        .header("Origin", "http://localhost:8080")
        .header("Cookie", &session.cookie)
        .header("X-CSRF-Token", &session.csrf)
        .body(Body::from(body))
        .unwrap();
    let response = http::router(state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap())
}
fn png(size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size.max(32)];
    bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    bytes
}

#[tokio::test]
async fn authentication_csrf_roles_and_revocation() {
    let f = Fixture::new().await;
    assert_eq!(
        send(&f.state, "GET", "/projects", None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (_, user, headers) = send(
        &f.state,
        "POST",
        "/auth/login",
        None,
        Some(json!({"email":"ADMIN@example.test","password":"a long test-only passphrase"})),
    )
    .await;
    assert_eq!(user["user"]["id"], 1);
    assert!(headers["set-cookie"].to_str().unwrap().contains("HttpOnly"));
    assert!(user["user"].get("password_hash").is_none());
    let hash: String = db::scalar(
        &f.state.db.connect().await.unwrap(),
        "SELECT password_hash FROM users WHERE id=1",
        (),
    )
    .await
    .unwrap();
    assert!(hash.starts_with("$argon2id$"));
    let mut bad = f.admin.clone();
    bad.csrf = "wrong".into();
    assert_eq!(
        send(
            &f.state,
            "POST",
            "/projects",
            Some(&bad),
            Some(json!({"name":"Wrong"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let member = f.member().await;
    let project = f.project().await;
    assert_eq!(
        send(
            &f.state,
            "GET",
            &format!("/projects/{project}"),
            Some(&member),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(
            &f.state,
            "POST",
            "/admin/tokens",
            Some(&member),
            Some(json!({"email":"x@example.test","purpose":"invite"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    // An enrolled gateway owns browser-origin policy; backend link URLs do not
    // restrict authenticated, CSRF-protected requests from that gateway.
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/projects")
        .header("Origin", "https://gateway.example")
        .header("Cookie", &f.admin.cookie)
        .header("X-CSRF-Token", &f.admin.csrf)
        .header("Content-Type", "application/json")
        .body(Body::from(r#"{"name":"different gateway origin"}"#))
        .unwrap();
    assert_eq!(
        http::router(f.state.clone())
            .oneshot(request)
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(
            &f.state,
            "POST",
            "/projects",
            Some(&member),
            Some(json!({"name":"Viewers cannot create"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &f.state,
            "PATCH",
            "/admin/users/1",
            Some(&f.admin),
            Some(json!({"role":"viewer","active":false}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(
            &f.state,
            "PATCH",
            &format!("/admin/users/{}", member.id),
            Some(&f.admin),
            Some(json!({"role":"viewer","active":false}))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(&f.state, "GET", "/projects", Some(&member), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&f.state, "POST", "/auth/logout", Some(&f.admin), None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        send(&f.state, "GET", "/projects", Some(&f.admin), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn tasks_keep_projects_archive_restore_and_purge_owned_content() {
    let f = Fixture::new().await;
    let member = f.member().await;
    let project = f.project().await;
    let task = f.task(project).await;
    let id = task["id"].as_i64().unwrap();
    assert_eq!(
        send(
            &f.state,
            "POST",
            &format!("/tasks/{id}/comments"),
            Some(&member),
            Some(json!({"body":"Viewers cannot comment"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    db::execute(
        &f.state.db.connect().await.unwrap(),
        "UPDATE users SET role='editor' WHERE id=?",
        turso::params![member.id],
    )
    .await
    .unwrap();
    assert!(
        db::execute(
            &f.state.db.connect().await.unwrap(),
            "INSERT INTO tasks(project_id,title,creator_id) VALUES(999999,'Orphan',1)",
            ()
        )
        .await
        .is_err()
    );
    assert!(
        db::execute(
            &f.state.db.connect().await.unwrap(),
            "DELETE FROM projects WHERE id=?",
            turso::params![project]
        )
        .await
        .is_err()
    );
    send(
        &f.state,
        "POST",
        &format!("/tasks/{id}/comments"),
        Some(&member),
        Some(json!({"body":"Please keep this context."})),
    )
    .await;
    send(
        &f.state,
        "POST",
        &format!("/tasks/{id}/links"),
        Some(&member),
        Some(json!({"url":"https://github.com/example/repo/issues/1"})),
    )
    .await;
    let (status, attachment) = image(&f.state, &member, id, &png(100)).await;
    assert_eq!(status, StatusCode::OK, "{attachment}");
    let archived = send(
        &f.state,
        "DELETE",
        &format!("/tasks/{id}"),
        Some(&member),
        Some(json!({"version":1})),
    )
    .await;
    assert_eq!(archived.0, StatusCode::OK);
    assert_eq!(archived.1["project_id"], project);
    assert_eq!(
        send(&f.state, "GET", "/tasks", Some(&member), None).await.1["items"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        send(&f.state, "GET", "/tasks?archived=true", Some(&member), None)
            .await
            .1["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        send(
            &f.state,
            "POST",
            &format!("/tasks/{id}/comments"),
            Some(&member),
            Some(json!({"body":"Cannot edit archived"}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let restored = send(
        &f.state,
        "POST",
        &format!("/tasks/{id}/restore"),
        Some(&member),
        Some(json!({"version":2})),
    )
    .await;
    assert_eq!(restored.0, StatusCode::OK);
    assert_eq!(restored.1["status"], "todo");
    let (_, detail, _) = send(
        &f.state,
        "GET",
        &format!("/tasks/{id}"),
        Some(&member),
        None,
    )
    .await;
    assert_eq!(detail["comments"].as_array().unwrap().len(), 1);
    assert_eq!(detail["attachments"].as_array().unwrap().len(), 1);
    assert_eq!(
        send(
            &f.state,
            "DELETE",
            &format!("/tasks/{id}/permanent"),
            Some(&f.admin),
            Some(json!({"version":3}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    send(
        &f.state,
        "DELETE",
        &format!("/tasks/{id}"),
        Some(&f.admin),
        Some(json!({"version":3})),
    )
    .await;
    assert_eq!(
        send(
            &f.state,
            "DELETE",
            &format!("/tasks/{id}/permanent"),
            Some(&f.admin),
            Some(json!({"version":4}))
        )
        .await
        .0,
        StatusCode::OK
    );
    for table in [
        "tasks",
        "comments",
        "task_links",
        "attachments",
        "attachment_chunks",
        "task_watchers",
        "activity_events",
        "notifications",
    ] {
        let count: i64 = db::scalar(
            &f.state.db.connect().await.unwrap(),
            &format!("SELECT COUNT(*) FROM {table}"),
            (),
        )
        .await
        .unwrap();
        assert_eq!(count, 0, "{table} was not purged");
    }
    let next = f.task(project).await;
    assert!(next["id"].as_i64().unwrap() > id);
}

#[tokio::test]
async fn quota_covers_images_and_allows_recovery() {
    let f = Fixture::new().await;
    let project = f.project().await;
    let task = f.task(project).await;
    let id = task["id"].as_i64().unwrap();
    let content = png(10 * 1024 * 1024);
    let (status, image) = image(&f.state, &f.admin, id, &content).await;
    assert_eq!(status, StatusCode::OK, "{image}");
    assert_eq!(image["size"], content.len());
    let attachment = image["id"].as_str().unwrap();
    let request = Request::builder()
        .uri(format!("/api/v1/attachments/{attachment}"))
        .header("Cookie", &f.admin.cookie)
        .body(Body::empty())
        .unwrap();
    let response = http::router(f.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .as_ref(),
        content.as_slice()
    );
    let (status, error) = self::image(&f.state, &f.admin, id, &png(10 * 1024 * 1024 + 1)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{error}");
    assert_eq!(error["error"]["code"], "image_too_large");
    let count: i64 = db::scalar(
        &f.state.db.connect().await.unwrap(),
        "SELECT COUNT(*) FROM attachments",
        (),
    )
    .await
    .unwrap();
    assert_eq!(count, 1, "Oversized upload must not persist");
    assert_eq!(
        send(
            &f.state,
            "GET",
            &format!("/attachments/{attachment}"),
            None,
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let size = f.state.storage().await.unwrap().database_bytes;
    assert!(size > content.len() as u64);
    send(
        &f.state,
        "PATCH",
        "/admin/settings",
        Some(&f.admin),
        Some(json!({"max_db_bytes":size-1})),
    )
    .await;
    assert!(f.state.storage().await.unwrap().content_blocked);
    for (path, body) in [
        ("/projects".into(), json!({"name":"No more"})),
        (
            format!("/projects/{project}/tasks"),
            json!({"title":"No more"}),
        ),
        (format!("/tasks/{id}/comments"), json!({"body":"No more"})),
        (
            format!("/tasks/{id}/links"),
            json!({"url":"https://example.com"}),
        ),
    ] {
        assert_eq!(
            send(&f.state, "POST", &path, Some(&f.admin), Some(body))
                .await
                .0,
            StatusCode::INSUFFICIENT_STORAGE,
            "{path}"
        );
    }
    assert_eq!(
        self::image(&f.state, &f.admin, id, &png(100)).await.0,
        StatusCode::INSUFFICIENT_STORAGE
    );
    let mut update = task.clone();
    update["status"] = json!("done");
    assert_eq!(
        send(
            &f.state,
            "PATCH",
            &format!("/tasks/{id}"),
            Some(&f.admin),
            Some(update)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(
            &f.state,
            "POST",
            "/auth/login",
            None,
            Some(json!({"email":"admin@example.test","password":"a long test-only passphrase"}))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(
            &f.state,
            "DELETE",
            &format!("/tasks/{id}"),
            Some(&f.admin),
            Some(json!({"version":2}))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(
            &f.state,
            "DELETE",
            &format!("/tasks/{id}/permanent"),
            Some(&f.admin),
            Some(json!({"version":3}))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(
        !f.state.storage().await.unwrap().content_blocked,
        "Auto-vacuum must reclaim purged image pages"
    );
    assert_eq!(
        send(
            &f.state,
            "POST",
            &format!("/projects/{project}/tasks"),
            Some(&f.admin),
            Some(json!({"title":"Space reclaimed"}))
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn content_crossing_threshold_rolls_back_atomically() {
    let f = Fixture::new().await;
    let project = f.project().await;
    let size = f.state.storage().await.unwrap().database_bytes;
    f.state.max_db_bytes.store(size + 8192, Ordering::Relaxed);
    let (status, body, _) = send(
        &f.state,
        "POST",
        &format!("/projects/{project}/tasks"),
        Some(&f.admin),
        Some(json!({"title":"Must not persist","description":"x".repeat(200_000)})),
    )
    .await;
    assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE, "{body}");
    for table in ["tasks", "task_watchers", "activity_events", "notifications"] {
        let count: i64 = db::scalar(
            &f.state.db.connect().await.unwrap(),
            &format!("SELECT COUNT(*) FROM {table}"),
            (),
        )
        .await
        .unwrap();
        assert_eq!(count, 0, "Partially committed {table}");
    }
    f.state.max_db_bytes.store(0, Ordering::Relaxed);
    let task = f.task(project).await;
    let id = task["id"].as_i64().unwrap();
    let size = f.state.storage().await.unwrap().database_bytes;
    f.state.max_db_bytes.store(size + 4096, Ordering::Relaxed);
    assert_eq!(
        image(&f.state, &f.admin, id, &png(200_000)).await.0,
        StatusCode::INSUFFICIENT_STORAGE
    );
    let count: i64 = db::scalar(
        &f.state.db.connect().await.unwrap(),
        "SELECT COUNT(*) FROM attachments",
        (),
    )
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn stale_edits_and_replayed_discord_mutations_do_not_overwrite_or_duplicate() {
    let f = Fixture::new().await;
    let project = f.project().await;
    let task = f.task(project).await;
    let id = task["id"].as_i64().unwrap();
    let actor = auth::get_user(&f.state, 1).await.unwrap();
    let current = services::get_task(&f.state, id).await.unwrap();
    let mut input = TaskInput::from(&current);
    input.status = "done".into();
    let updated = services::save_task(
        &f.state,
        &actor,
        id,
        input.clone(),
        "discord",
        Some("discord-123"),
    )
    .await
    .unwrap();
    assert_eq!(updated.version, 2);
    let replay = services::save_task(
        &f.state,
        &actor,
        id,
        input.clone(),
        "discord",
        Some("discord-123"),
    )
    .await
    .unwrap();
    assert_eq!(replay.version, 2);
    assert_eq!(
        services::save_task(&f.state, &actor, id, input, "web", None)
            .await
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    let count: i64 = db::scalar(
        &f.state.db.connect().await.unwrap(),
        "SELECT COUNT(*) FROM activity_events WHERE task_id=?",
        turso::params![id],
    )
    .await
    .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn project_archival_pauses_tasks_and_keeps_membership() {
    let f = Fixture::new().await;
    let member = f.member().await;
    let project = f.project().await;
    let task = f.task(project).await;
    let id = task["id"].as_i64().unwrap();
    assert_eq!(
        send(
            &f.state,
            "POST",
            &format!("/projects/{project}/archive"),
            Some(&member),
            Some(json!({"version":1}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    send(
        &f.state,
        "POST",
        &format!("/projects/{project}/archive"),
        Some(&f.admin),
        Some(json!({"version":1})),
    )
    .await;
    let mut input = task;
    input["status"] = json!("done");
    assert_eq!(
        send(
            &f.state,
            "PATCH",
            &format!("/tasks/{id}"),
            Some(&f.admin),
            Some(input)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        services::get_task(&f.state, id).await.unwrap().project_id,
        project
    );
    send(
        &f.state,
        "POST",
        &format!("/projects/{project}/restore"),
        Some(&f.admin),
        Some(json!({"version":2})),
    )
    .await;
    assert_eq!(
        send(&f.state, "GET", "/tasks", Some(&member), None).await.1["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn due_reminders_deduplicate_catch_up_and_cancel() {
    let f = Fixture::new().await;
    let member = f.member().await;
    let project = f.project().await;
    let mut task = f.task(project).await;
    let id = task["id"].as_i64().unwrap();
    send(
        &f.state,
        "PUT",
        &format!("/tasks/{id}/watchers/me"),
        Some(&member),
        None,
    )
    .await;
    task["due_date"] = json!("2026-03-08");
    task["assignee_id"] = json!(member.id);
    send(
        &f.state,
        "PATCH",
        &format!("/tasks/{id}"),
        Some(&f.admin),
        Some(task),
    )
    .await;
    db::execute(
        &f.state.db.connect().await.unwrap(),
        "UPDATE users SET watched_due=1 WHERE id=?",
        turso::params![member.id],
    )
    .await
    .unwrap();
    let at = chrono::DateTime::parse_from_rfc3339("2026-03-08T16:01:00Z")
        .unwrap()
        .timestamp();
    jobs::schedule_due(&f.state, at).await.unwrap();
    jobs::schedule_due(&f.state, at).await.unwrap();
    let count: i64 = db::scalar(
        &f.state.db.connect().await.unwrap(),
        "SELECT COUNT(*) FROM notifications WHERE user_id=? AND kind='due'",
        turso::params![member.id],
    )
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "Assignee + watcher paths or restarts duplicated a reminder"
    );
    let overdue = chrono::DateTime::parse_from_rfc3339("2026-04-20T18:00:00Z")
        .unwrap()
        .timestamp();
    jobs::schedule_due(&f.state, overdue).await.unwrap();
    jobs::schedule_due(&f.state, overdue).await.unwrap();
    let count: i64 = db::scalar(
        &f.state.db.connect().await.unwrap(),
        "SELECT COUNT(*) FROM notifications WHERE user_id=? AND kind='due'",
        turso::params![member.id],
    )
    .await
    .unwrap();
    assert_eq!(
        count, 2,
        "Catch-up should add only one current overdue reminder"
    );
    let actor = auth::get_user(&f.state, 1).await.unwrap();
    let task = services::get_task(&f.state, id).await.unwrap();
    let mut input = TaskInput::from(&task);
    input.status = "done".into();
    services::save_task(&f.state, &actor, id, input, "web", None)
        .await
        .unwrap();
    jobs::schedule_due(&f.state, overdue).await.unwrap();
    let pending:i64=db::scalar(&f.state.db.connect().await.unwrap(), "SELECT COUNT(*) FROM notification_deliveries d JOIN notifications n ON n.id=d.notification_id WHERE n.kind='due' AND d.state='pending'", ()).await.unwrap();
    assert_eq!(pending, 0);
    let self_notifications: i64 = db::scalar(
        &f.state.db.connect().await.unwrap(),
        "SELECT COUNT(*) FROM notifications WHERE user_id=1 AND kind='activity'",
        (),
    )
    .await
    .unwrap();
    assert_eq!(self_notifications, 0);
}

#[test]
fn deadlines_observe_daylight_saving_and_command_contract() {
    let (start, end) = services::deadline(Some("2026-03-08"), "America/Los_Angeles").unwrap();
    assert_eq!(end.unwrap() - start.unwrap(), 23 * 3600);
    let (start, end) = services::deadline(Some("2026-11-01"), "America/Los_Angeles").unwrap();
    assert_eq!(end.unwrap() - start.unwrap(), 25 * 3600);
    assert!(services::deadline(Some("2026-02-31"), "UTC").is_err());
    let at = chrono::DateTime::parse_from_rfc3339("2026-03-08T16:00:00Z")
        .unwrap()
        .timestamp();
    assert_eq!(
        jobs::reminder_slot("2026-03-08", "America/Los_Angeles", at),
        Some(("due", "Due today"))
    );
    let commands = serde_json::to_value(taskboard::discord::commands()).unwrap();
    assert!(
        commands
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "status")
    );
    assert_eq!(taskboard::discord::task_id("TB-123").unwrap(), 123);
    assert!(taskboard::discord::task_id("-4").is_err());
}

#[tokio::test]
async fn user_timezone_controls_new_deadlines_while_instants_are_utc() {
    let f = Fixture::new().await;
    let project = f.project().await;
    let initial = auth::get_user(&f.state, 1).await.unwrap();
    assert_eq!(initial.timezone, "UTC");
    db::execute(
        &f.state.db.connect().await.unwrap(),
        "UPDATE users SET timezone='America/Los_Angeles' WHERE id=1",
        (),
    )
    .await
    .unwrap();
    let task = send(
        &f.state,
        "POST",
        &format!("/projects/{project}/tasks"),
        Some(&f.admin),
        Some(json!({"title":"Local deadline","due_date":"2026-03-08"})),
    )
    .await
    .1;
    assert_eq!(task["due_timezone"], "America/Los_Angeles");
    let (start, end): (i64, i64) = db::one_as(
        &f.state.db.connect().await.unwrap(),
        "SELECT due_start,due_at FROM tasks WHERE id=?",
        turso::params![task["id"].as_i64().unwrap()],
    )
    .await
    .unwrap();
    assert_eq!(
        start,
        chrono::DateTime::parse_from_rfc3339("2026-03-08T08:00:00Z")
            .unwrap()
            .timestamp()
    );
    assert_eq!(
        end,
        chrono::DateTime::parse_from_rfc3339("2026-03-09T07:00:00Z")
            .unwrap()
            .timestamp()
    );
}

#[tokio::test]
async fn delivery_failure_is_durable_and_never_calls_live_discord() {
    let f = Fixture::new().await;
    let member = f.member().await;
    let project = f.project().await;
    let task = f.task(project).await;
    let id = task["id"].as_i64().unwrap();
    send(
        &f.state,
        "PUT",
        &format!("/tasks/{id}/watchers/me"),
        Some(&member),
        None,
    )
    .await;
    db::execute(&f.state.db.connect().await.unwrap(), "INSERT INTO discord_accounts(user_id,discord_id,discord_name) VALUES(?,'123456789','Jamie')", turso::params![member.id]).await.unwrap();
    send(
        &f.state,
        "POST",
        &format!("/tasks/{id}/comments"),
        Some(&f.admin),
        Some(json!({"body":"A watched update"})),
    )
    .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mock = Router::new().fallback(|| async {
        (
            StatusCode::FORBIDDEN,
            Json(json!({"code":50007,"message":"Cannot send messages to this user"})),
        )
    });
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let http = serenity::http::HttpBuilder::new("test-only-token")
        .proxy(format!("http://{address}"))
        .ratelimiter_disabled(true)
        .build();
    assert!(jobs::deliver_one(&f.state, &http).await.unwrap());
    let state: String = db::scalar(
        &f.state.db.connect().await.unwrap(),
        "SELECT state FROM notification_deliveries",
        (),
    )
    .await
    .unwrap();
    assert_eq!(state, "failed");
    let unread: i64 = db::scalar(
        &f.state.db.connect().await.unwrap(),
        "SELECT COUNT(*) FROM notifications WHERE user_id=? AND read_at IS NULL",
        turso::params![member.id],
    )
    .await
    .unwrap();
    assert_eq!(
        unread, 1,
        "Blocked DMs must preserve the in-app notification"
    );
    server.abort();
}
