use crate::{auth::Auth,error::{Error,Result},models::{Attachment,Task,TaskFilter},services::{self,TaskInput},state::AppState};
use axum::{extract::{State,Path,Query},Json};
use serde::Deserialize;
use serde_json::{Value,json};
use sqlx::Row;
use super::projects::Version;

pub async fn list(State(s):State<AppState>,_auth:Auth,Query(filter):Query<TaskFilter>)->Result<Json<Value>>{Ok(Json(services::list_tasks(&s,&filter).await?))}
pub async fn create(State(s):State<AppState>,auth:Auth,Path(id):Path<i64>,Json(input):Json<TaskInput>)->Result<Json<Task>>{Ok(Json(services::create_task(&s,&auth.user,id,input).await?))}
pub async fn update(State(s):State<AppState>,auth:Auth,Path(id):Path<i64>,Json(input):Json<TaskInput>)->Result<Json<Task>>{Ok(Json(services::save_task(&s,&auth.user,id,input,"web",None).await?))}
pub async fn detail(State(s):State<AppState>,_auth:Auth,Path(id):Path<i64>)->Result<Json<Value>>{
    let task=services::get_task(&s,id).await?;
    let comments=sqlx::query("SELECT c.*,u.name AS author_name FROM comments c JOIN users u ON u.id=c.author_id WHERE c.task_id=? ORDER BY c.id DESC LIMIT 200").bind(id).fetch_all(&s.pool).await?.into_iter().rev().map(|r|json!({"id":r.get::<i64,_>("id"),"author_id":r.get::<i64,_>("author_id"),"author_name":r.get::<String,_>("author_name"),"body":r.get::<String,_>("body"),"created_at":r.get::<String,_>("created_at")})).collect::<Vec<_>>();
    let activity=sqlx::query("SELECT e.*,u.name AS actor_name FROM activity_events e JOIN users u ON u.id=e.actor_id WHERE e.task_id=? ORDER BY e.id DESC LIMIT 100").bind(id).fetch_all(&s.pool).await?.into_iter().map(|r|json!({"id":r.get::<i64,_>("id"),"actor_name":r.get::<String,_>("actor_name"),"detail":r.get::<String,_>("detail"),"source":r.get::<String,_>("source"),"created_at":r.get::<String,_>("created_at")})).collect::<Vec<_>>();
    let links=sqlx::query("SELECT * FROM task_links WHERE task_id=? ORDER BY id").bind(id).fetch_all(&s.pool).await?.into_iter().map(|r|json!({"id":r.get::<i64,_>("id"),"url":r.get::<String,_>("url"),"label":r.get::<String,_>("label")})).collect::<Vec<_>>();
    let watchers=sqlx::query("SELECT u.id,u.name FROM task_watchers w JOIN users u ON u.id=w.user_id WHERE w.task_id=? ORDER BY u.name").bind(id).fetch_all(&s.pool).await?.into_iter().map(|r|json!({"id":r.get::<i64,_>("id"),"name":r.get::<String,_>("name")})).collect::<Vec<_>>();
    let attachments:Vec<Attachment>=sqlx::query_as("SELECT id,filename,mime,size,created_at FROM attachments WHERE task_id=? ORDER BY created_at").bind(id).fetch_all(&s.pool).await?;
    Ok(Json(json!({"task":task,"comments":comments,"activity":activity,"links":links,"watchers":watchers,"attachments":attachments})))
}
pub async fn archive(State(s):State<AppState>,auth:Auth,Path(id):Path<i64>,Json(v):Json<Version>)->Result<Json<Value>>{Ok(Json(json!(services::archive_task(&s,&auth.user,id,v.version,"archive","web",None).await?)))}
pub async fn restore(State(s):State<AppState>,auth:Auth,Path(id):Path<i64>,Json(v):Json<Version>)->Result<Json<Value>>{Ok(Json(json!(services::archive_task(&s,&auth.user,id,v.version,"restore","web",None).await?)))}
pub async fn purge(State(s):State<AppState>,auth:Auth,Path(id):Path<i64>,Json(v):Json<Version>)->Result<Json<Value>>{services::archive_task(&s,&auth.user,id,v.version,"purge","web",None).await?;Ok(Json(json!({"ok":true})))}
#[derive(Deserialize)]pub struct Comment{body:String}
pub async fn comment(State(s):State<AppState>,auth:Auth,Path(id):Path<i64>,Json(input):Json<Comment>)->Result<Json<Value>>{services::add_comment(&s,&auth.user,id,&input.body).await?;Ok(Json(json!({"ok":true})))}
pub async fn watch(State(s):State<AppState>,auth:Auth,Path(id):Path<i64>)->Result<Json<Value>>{services::watch(&s,&auth.user,id,true).await?;Ok(Json(json!({"ok":true})))}
pub async fn unwatch(State(s):State<AppState>,auth:Auth,Path(id):Path<i64>)->Result<Json<Value>>{services::watch(&s,&auth.user,id,false).await?;Ok(Json(json!({"ok":true})))}
#[derive(Deserialize)]pub struct Link{url:String,#[serde(default)]label:String}
pub async fn link(State(s):State<AppState>,auth:Auth,Path(id):Path<i64>,Json(input):Json<Link>)->Result<Json<Value>>{services::link(&s,&auth.user,id,&input.url,&input.label).await?;Ok(Json(json!({"ok":true})))}
pub async fn unlink(State(s):State<AppState>,auth:Auth,Path((id,link)):Path<(i64,i64)>)->Result<Json<Value>>{
    let _guard=s.writes.lock().await;let mut tx=s.pool.begin().await?;let actor=services::current_actor(&mut tx,&auth.user).await?;services::require_editor(&actor)?;let task=services::task_on(&mut tx,id).await?;services::editable(&task)?;
    if sqlx::query("DELETE FROM task_links WHERE id=? AND task_id=?").bind(link).bind(id).execute(&mut *tx).await?.rows_affected()==0{return Err(Error::missing());}
    services::event(&mut tx,&actor,&task,"unlinked","removed a link","web").await?;tx.commit().await?;Ok(Json(json!({"ok":true})))
}
