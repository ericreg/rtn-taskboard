use crate::{auth::Auth,error::{Error,Result},models::Attachment,services,state::AppState};
use axum::{extract::{State,Path,Multipart},body::{Body,Bytes},http::{header,HeaderValue},response::{Response,IntoResponse},Json};
use futures_util::TryStreamExt;
use serde_json::{Value,json};
use sqlx::Row;
use tokio::io::{AsyncReadExt,AsyncSeekExt,AsyncWriteExt};
use std::sync::atomic::Ordering;

pub async fn task_upload(State(s):State<AppState>,auth:Auth,Path(id):Path<i64>,multipart:Multipart)->Result<Json<Attachment>>{upload(s,auth,Some(id),None,multipart).await}
pub async fn project_upload(State(s):State<AppState>,auth:Auth,Path(id):Path<i64>,multipart:Multipart)->Result<Json<Attachment>>{upload(s,auth,None,Some(id),multipart).await}
fn image_type(prefix:&[u8])->Option<&'static str>{
    if prefix.starts_with(b"\x89PNG\r\n\x1a\n"){Some("image/png")}
    else if prefix.starts_with(b"\xff\xd8\xff"){Some("image/jpeg")}
    else if prefix.starts_with(b"GIF87a")||prefix.starts_with(b"GIF89a"){Some("image/gif")}
    else if prefix.starts_with(b"RIFF")&&prefix.get(8..12)==Some(b"WEBP".as_slice()){Some("image/webp")}
    else{None}
}
async fn upload(s:AppState,auth:Auth,task_id:Option<i64>,project_id:Option<i64>,mut multipart:Multipart)->Result<Json<Attachment>>{
    services::require_editor(&auth.user)?;
    {
        let mut conn=s.pool.acquire().await?;s.check_capacity(&mut conn).await?;
        if let Some(id)=task_id{services::editable(&services::task_on(&mut conn,id).await?)?;}
        if let Some(id)=project_id{services::active_project(&mut conn,id).await?;}
    }
    // Spool incoming bytes before taking the database writer lock. No per-image limit.
    let temporary=tempfile::tempfile()?;let mut file=tokio::fs::File::from_std(temporary);
    let mut field=multipart.next_field().await.map_err(|_|Error::bad("Invalid image upload."))?.ok_or_else(||Error::bad("Select an image."))?;
    if field.name()!=Some("file"){return Err(Error::bad("Use the file upload field."));}
    let filename=field.file_name().unwrap_or("image").chars().filter(|c|!c.is_control()).take(255).collect::<String>();
    let mut prefix=Vec::new();let mut size=0u64;let starting_size=s.storage().await?.database_bytes;
    while let Some(bytes)=field.chunk().await.map_err(|_|Error::bad("The upload was interrupted."))?{
        size=size.checked_add(bytes.len() as u64).ok_or_else(||Error::bad("Image is too large for this system."))?;
        let limit=s.max_db_bytes.load(Ordering::Relaxed);
        if limit>0&&starting_size.saturating_add(size)>limit{return Err(Error::full());}
        if prefix.len()<32{prefix.extend_from_slice(&bytes[..bytes.len().min(32-prefix.len())]);}
        file.write_all(&bytes).await?;
    }
    if size<16{return Err(Error::bad("This image is empty or invalid."));}
    let mime=image_type(&prefix).ok_or_else(||Error::bad("Upload a PNG, JPEG, GIF, or WebP image. SVG and HTML are not supported."))?;
    drop(field);
    if multipart.next_field().await.map_err(|_|Error::bad("Invalid upload."))?.is_some(){return Err(Error::bad("Upload one image at a time."));}
    file.flush().await?;file.rewind().await?;
    let _guard=s.writes.lock().await;let mut tx=s.pool.begin().await?;let actor=services::current_actor(&mut tx,&auth.user).await?;services::require_editor(&actor)?;s.check_capacity(&mut tx).await?;
    let task=if let Some(id)=task_id{let t=services::task_on(&mut tx,id).await?;services::editable(&t)?;Some(t)}else{services::active_project(&mut tx,project_id.unwrap()).await?;None};
    let id=uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO attachments(id,task_id,project_id,uploader_id,filename,mime,size) VALUES(?,?,?,?,?,?,?)").bind(&id).bind(task_id).bind(project_id).bind(actor.id).bind(filename).bind(mime).bind(i64::try_from(size).map_err(|_|Error::bad("Image size is out of range."))?).execute(&mut *tx).await?;
    let mut buffer=vec![0u8;1024*1024];let mut sequence=0i64;
    loop{let read=file.read(&mut buffer).await?;if read==0{break;}
        sqlx::query("INSERT INTO attachment_chunks(attachment_id,sequence,data) VALUES(?,?,?)").bind(&id).bind(sequence).bind(&buffer[..read]).execute(&mut *tx).await?;sequence+=1;s.check_result_size(&mut tx).await?;
    }
    if let Some(task)=task{services::event(&mut tx,&actor,&task,"uploaded","added an image","web").await?;}
    s.check_result_size(&mut tx).await?;let attachment=sqlx::query_as("SELECT id,filename,mime,size,created_at FROM attachments WHERE id=?").bind(&id).fetch_one(&mut *tx).await?;
    tx.commit().await?;Ok(Json(attachment))
}
pub async fn download(State(s):State<AppState>,_auth:Auth,Path(id):Path<String>)->Result<Response>{
    let item=sqlx::query("SELECT mime,size FROM attachments WHERE id=?").bind(&id).fetch_optional(&s.pool).await?.ok_or_else(Error::missing)?;
    let mime:String=item.get("mime");let size:i64=item.get("size");
    let stream=async_stream::try_stream!{
        let mut chunks=sqlx::query("SELECT data FROM attachment_chunks WHERE attachment_id=? ORDER BY sequence").bind(id).fetch(&s.pool);
        while let Some(row)=chunks.try_next().await.map_err(std::io::Error::other)?{yield Bytes::from(row.get::<Vec<u8>,_>("data"));}
    };
    let mut response=Body::from_stream(stream.map_err(|e:std::io::Error|e)).into_response();
    response.headers_mut().insert(header::CONTENT_TYPE,HeaderValue::from_str(&mime).map_err(|_|Error::bad("Invalid image type."))?);
    response.headers_mut().insert(header::CONTENT_LENGTH,HeaderValue::from_str(&size.to_string()).unwrap());
    response.headers_mut().insert(header::CONTENT_DISPOSITION,HeaderValue::from_static("inline"));Ok(response)
}
pub async fn remove(State(s):State<AppState>,auth:Auth,Path(id):Path<String>)->Result<Json<Value>>{
    let _guard=s.writes.lock().await;let mut tx=s.pool.begin().await?;let actor=services::current_actor(&mut tx,&auth.user).await?;services::require_editor(&actor)?;
    let item=sqlx::query("SELECT task_id,project_id FROM attachments WHERE id=?").bind(&id).fetch_optional(&mut *tx).await?.ok_or_else(Error::missing)?;
    let task_id:Option<i64>=item.get("task_id");
    if let Some(task_id)=task_id{let task=services::task_on(&mut tx,task_id).await?;services::editable(&task)?;services::event(&mut tx,&actor,&task,"image_removed","removed an image","web").await?;}
    else{services::active_project(&mut tx,item.get("project_id")).await?;}
    sqlx::query("DELETE FROM attachments WHERE id=?").bind(id).execute(&mut *tx).await?;tx.commit().await?;Ok(Json(json!({"ok":true})))
}
