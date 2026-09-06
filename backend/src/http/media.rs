use crate::db;
use crate::{
    auth::Auth,
    error::{Error, Result},
    models::Attachment,
    services,
    state::AppState,
};
use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Multipart, Path, State},
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use futures_util::TryStreamExt;
use serde_json::{Value, json};

use std::sync::atomic::Ordering;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

pub async fn task_upload(
    State(s): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    multipart: Multipart,
) -> Result<Json<Attachment>> {
    upload(s, auth, Some(id), None, multipart).await
}
pub async fn project_upload(
    State(s): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    multipart: Multipart,
) -> Result<Json<Attachment>> {
    upload(s, auth, None, Some(id), multipart).await
}
fn image_type(prefix: &[u8]) -> Option<&'static str> {
    if prefix.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if prefix.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if prefix.starts_with(b"GIF87a") || prefix.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if prefix.starts_with(b"RIFF") && prefix.get(8..12) == Some(b"WEBP".as_slice()) {
        Some("image/webp")
    } else {
        None
    }
}
async fn upload(
    s: AppState,
    auth: Auth,
    task_id: Option<i64>,
    project_id: Option<i64>,
    mut multipart: Multipart,
) -> Result<Json<Attachment>> {
    services::require_editor(&auth.user)?;
    {
        let conn = s.db.connect().await?;
        s.check_capacity(&conn).await?;
        if let Some(id) = task_id {
            services::editable(&services::task_on(&conn, id).await?)?;
        }
        if let Some(id) = project_id {
            services::active_project(&conn, id).await?;
        }
    }
    // Spool incoming bytes before taking the database writer lock.
    let temporary = tempfile::tempfile()?;
    let mut file = tokio::fs::File::from_std(temporary);
    let mut field = multipart
        .next_field()
        .await
        .map_err(|_| Error::bad("Invalid image upload."))?
        .ok_or_else(|| Error::bad("Select an image."))?;
    if field.name() != Some("file") {
        return Err(Error::bad("Use the file upload field."));
    }
    let filename = field
        .file_name()
        .unwrap_or("image")
        .chars()
        .filter(|c| !c.is_control())
        .take(255)
        .collect::<String>();
    let mut prefix = Vec::new();
    let mut size = 0u64;
    let starting_size = s.storage().await?.database_bytes;
    while let Some(bytes) = field
        .chunk()
        .await
        .map_err(|_| Error::bad("The upload was interrupted."))?
    {
        size = size
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| Error::bad("Image is too large for this system."))?;
        if size > s.config.max_image_bytes {
            return Err(Error::image_too_large(s.config.max_image_bytes));
        }
        let limit = s.max_db_bytes.load(Ordering::Relaxed);
        if limit > 0 && starting_size.saturating_add(size) > limit {
            return Err(Error::full());
        }
        if prefix.len() < 32 {
            prefix.extend_from_slice(&bytes[..bytes.len().min(32 - prefix.len())]);
        }
        file.write_all(&bytes).await?;
    }
    if size < 16 {
        return Err(Error::bad("This image is empty or invalid."));
    }
    let mime = image_type(&prefix).ok_or_else(|| {
        Error::bad("Upload a PNG, JPEG, GIF, or WebP image. SVG and HTML are not supported.")
    })?;
    drop(field);
    if multipart
        .next_field()
        .await
        .map_err(|_| Error::bad("Invalid upload."))?
        .is_some()
    {
        return Err(Error::bad("Upload one image at a time."));
    }
    file.flush().await?;
    file.rewind().await?;
    let _guard = s.writes.lock().await;
    let mut conn = s.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    let actor = services::current_actor(&tx, &auth.user).await?;
    services::require_editor(&actor)?;
    s.check_capacity(&tx).await?;
    let task = if let Some(id) = task_id {
        let t = services::task_on(&tx, id).await?;
        services::editable(&t)?;
        Some(t)
    } else {
        services::active_project(&tx, project_id.unwrap()).await?;
        None
    };
    let id = uuid::Uuid::new_v4().to_string();
    db::execute(&tx, "INSERT INTO attachments(id,task_id,project_id,uploader_id,filename,mime,size) VALUES(?,?,?,?,?,?,?)", turso::params![id.as_str(), task_id, project_id, actor.id, filename, mime, i64::try_from(size).map_err(|_|Error::bad("Image size is out of range."))?]).await?;
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut sequence = 0i64;
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        db::execute(
            &tx,
            "INSERT INTO attachment_chunks(attachment_id,sequence,data) VALUES(?,?,?)",
            turso::params![id.as_str(), sequence, &buffer[..read]],
        )
        .await?;
        sequence += 1;
        s.check_result_size(&tx).await?;
    }
    if let Some(task) = task {
        services::event(&tx, &actor, &task, "uploaded", "added an image", "web").await?;
    }
    s.check_result_size(&tx).await?;
    let attachment = db::one_as(
        &tx,
        "SELECT id,filename,mime,size,created_at FROM attachments WHERE id=?",
        turso::params![id.as_str()],
    )
    .await?;
    tx.commit().await?;
    Ok(Json(attachment))
}
pub async fn download(
    State(s): State<AppState>,
    _auth: Auth,
    Path(id): Path<String>,
) -> Result<Response> {
    let item = db::optional(
        &s.db.connect().await?,
        "SELECT mime,size FROM attachments WHERE id=?",
        turso::params![id.as_str()],
    )
    .await?
    .ok_or_else(Error::missing)?;
    let mime: String = item.get("mime");
    let size: i64 = item.get("size");
    let stream = async_stream::try_stream! {
        let conn = s.db.connect().await.map_err(std::io::Error::other)?;
        let mut chunks=conn.query("SELECT data FROM attachment_chunks WHERE attachment_id=? ORDER BY sequence", turso::params![id]).await.map_err(std::io::Error::other)?;
        while let Some(row)=chunks.next().await.map_err(std::io::Error::other)?{yield Bytes::from(row.get::<Vec<u8>>(0).map_err(std::io::Error::other)?);}
    };
    let mut response = Body::from_stream(stream.map_err(|e: std::io::Error| e)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&mime).map_err(|_| Error::bad("Invalid image type."))?,
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&size.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("inline"),
    );
    Ok(response)
}
pub async fn remove(
    State(s): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let _guard = s.writes.lock().await;
    let mut conn = s.db.connect().await?;
    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    let actor = services::current_actor(&tx, &auth.user).await?;
    services::require_editor(&actor)?;
    let item = db::optional(
        &tx,
        "SELECT task_id,project_id FROM attachments WHERE id=?",
        turso::params![id.as_str()],
    )
    .await?
    .ok_or_else(Error::missing)?;
    let task_id: Option<i64> = item.get("task_id");
    if let Some(task_id) = task_id {
        let task = services::task_on(&tx, task_id).await?;
        services::editable(&task)?;
        services::event(
            &tx,
            &actor,
            &task,
            "image_removed",
            "removed an image",
            "web",
        )
        .await?;
    } else {
        services::active_project(&tx, item.get("project_id")).await?;
    }
    db::execute(
        &tx,
        "DELETE FROM attachments WHERE id=?",
        turso::params![id],
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
