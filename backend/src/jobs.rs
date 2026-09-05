use crate::{auth::now,error::{Error,Result},services,state::AppState};
use chrono::{Days,NaiveDate,TimeZone};
use serenity::{http::Http,all::{UserId,CreateMessage,CreateAllowedMentions}};
use sqlx::Row;
use std::time::Duration;

pub fn reminder_slot(date:&str,zone:&str,at:i64)->Option<(&'static str,&'static str)>{
    let date=NaiveDate::parse_from_str(date,"%Y-%m-%d").ok()?;let zone:chrono_tz::Tz=zone.parse().ok()?;
    let slots=[(date.checked_add_days(Days::new(1))?,"overdue","Overdue"),(date,"due","Due today"),(date.checked_sub_days(Days::new(1))?,"before","Due tomorrow")];
    for (date,key,label) in slots{if let Some(instant)=zone.from_local_datetime(&date.and_hms_opt(9,0,0)?).earliest(){if instant.timestamp()<=at{return Some((key,label));}}}None
}
pub async fn schedule_due(s:&AppState,at:i64)->Result<()> {
    let ids:Vec<i64>=sqlx::query_scalar("SELECT t.id FROM tasks t JOIN projects p ON p.id=t.project_id WHERE t.due_date IS NOT NULL AND t.archived_at IS NULL AND p.archived_at IS NULL AND t.status NOT IN ('done','canceled')").fetch_all(&s.pool).await?;
    for id in ids{
        let _guard=s.writes.lock().await;let mut tx=s.pool.begin().await?;
        let task=match services::task_on(&mut tx,id).await{Ok(t)=>t,Err(e) if e.0==axum::http::StatusCode::NOT_FOUND=>continue,Err(e)=>return Err(e)};
        if services::editable(&task).is_err()||["done","canceled"].contains(&task.status.as_str()){continue;}
        let Some(date)=&task.due_date else{continue;};let Some((slot,label))=reminder_slot(date,&task.due_timezone,at) else{continue;};
        let users=sqlx::query("SELECT u.id,u.due_in_app,u.due_discord FROM users u WHERE u.active=1 AND (u.id=? OR (u.watched_due=1 AND EXISTS(SELECT 1 FROM task_watchers w WHERE w.task_id=? AND w.user_id=u.id)))").bind(task.assignee_id).bind(id).fetch_all(&mut *tx).await?;
        for user in users{
            let uid:i64=user.get("id");let in_app:bool=user.get("due_in_app");let discord:bool=user.get("due_discord");if !in_app&&!discord{continue;}
            let key=format!("due:{id}:{}:{uid}:{slot}",task.reminder_revision);
            let inserted=sqlx::query("INSERT OR IGNORE INTO notifications(user_id,task_id,kind,message,dedupe_key,in_app,reminder_revision) VALUES(?,?,'due',?,?,?,?)").bind(uid).bind(id).bind(format!("{label} · TB-{id} {} · {date} ({})",task.title,task.due_timezone)).bind(&key).bind(in_app).bind(task.reminder_revision).execute(&mut *tx).await?;
            if inserted.rows_affected()>0&&discord{sqlx::query("INSERT INTO notification_deliveries(notification_id) VALUES(?)").bind(inserted.last_insert_rowid()).execute(&mut *tx).await?;}
        }
        tx.commit().await?;
    }Ok(())
}

pub fn safe_discord_text(text:&str)->String{
    text.chars().flat_map(|c|if ['*','_','~','`','>','[',']','\\'].contains(&c){vec!['\\',c]}else if c=='@'{vec!['@','\u{200b}']}else if c=='\n'||c=='\r'{vec![' ']}else{vec![c]}).take(1400).collect()
}

pub async fn deliver_one(s:&AppState,http:&Http)->Result<bool>{
    let _guard=s.writes.lock().await;let mut tx=s.pool.begin().await?;
    let next=sqlx::query("SELECT id,notification_id,attempts FROM notification_deliveries WHERE (state IN ('pending','retry') AND next_attempt<=?) OR (state='leased' AND lease_until<?) ORDER BY id LIMIT 1").bind(now()).bind(now()).fetch_optional(&mut *tx).await?;
    let Some(next)=next else{return Ok(false);};let delivery_id:i64=next.get("id");let notification_id:i64=next.get("notification_id");let attempts:i64=next.get("attempts");
    let row=sqlx::query("SELECT n.*,u.active,u.activity_discord,u.due_discord,u.watched_due,d.discord_id,t.assignee_id,t.status,t.archived_at,t.reminder_revision AS current_revision,p.archived_at AS project_archived_at,EXISTS(SELECT 1 FROM task_watchers w WHERE w.task_id=t.id AND w.user_id=u.id) AS watching FROM notifications n JOIN users u ON u.id=n.user_id JOIN tasks t ON t.id=n.task_id JOIN projects p ON p.id=t.project_id LEFT JOIN discord_accounts d ON d.user_id=u.id WHERE n.id=?").bind(notification_id).fetch_optional(&mut *tx).await?;
    let Some(row)=row else{return Ok(true);};let kind:String=row.get("kind");let user:i64=row.get("user_id");let task:i64=row.get("task_id");let watching:bool=row.get("watching");
    let discord:Option<String>=row.get("discord_id");
    let eligible=row.get::<bool,_>("active")&&discord.is_some()&&if kind=="activity"{row.get::<bool,_>("activity_discord")&&watching}else{
        row.get::<bool,_>("due_discord")&&row.get::<Option<i64>,_>("reminder_revision")==Some(row.get("current_revision"))&&row.get::<Option<String>,_>("archived_at").is_none()&&row.get::<Option<String>,_>("project_archived_at").is_none()&&! ["done","canceled"].contains(&row.get::<String,_>("status").as_str())&&(row.get::<Option<i64>,_>("assignee_id")==Some(user)||(watching&&row.get::<bool,_>("watched_due")))
    };
    if !eligible{
        sqlx::query("UPDATE notification_deliveries SET state='canceled',lease_until=NULL WHERE id=?").bind(delivery_id).execute(&mut *tx).await?;tx.commit().await?;return Ok(true);
    }
    // Only the latest relevant due reminder may leave the queue after downtime.
    if kind=="due"{
        let later:i64=sqlx::query_scalar("SELECT COUNT(*) FROM notifications WHERE user_id=? AND task_id=? AND kind='due' AND reminder_revision=? AND id>?").bind(user).bind(task).bind(row.get::<i64,_>("current_revision")).bind(notification_id).fetch_one(&mut *tx).await?;
        if later>0{sqlx::query("UPDATE notification_deliveries SET state='canceled' WHERE id=?").bind(delivery_id).execute(&mut *tx).await?;tx.commit().await?;return Ok(true);}
    }
    sqlx::query("UPDATE notification_deliveries SET state='leased',lease_until=?,attempts=attempts+1 WHERE id=?").bind(now()+120).bind(delivery_id).execute(&mut *tx).await?;
    let content=format!("{}\n{}/tasks/TB-{task}",safe_discord_text(&row.get::<String,_>("message")),s.config.base_url);
    let discord_id:u64=discord.unwrap().parse().map_err(|_|Error::bad("Invalid linked Discord ID."))?;
    tx.commit().await?;drop(_guard);
    let sent=tokio::time::timeout(Duration::from_secs(45),async{
        let channel=UserId::new(discord_id).create_dm_channel(http).await?;
        channel.send_message(http,CreateMessage::new().content(content).allowed_mentions(CreateAllowedMentions::new())).await
    }).await;
    let (state,message_id,error)=match sent{
        Ok(Ok(message))=>("sent",Some(message.id.to_string()),None),
        Ok(Err(err))=>{
            let code=if let serenity::Error::Http(ref http)=err{http.status_code().map(|c|c.as_u16())}else{None};
            let permanent=matches!(code,Some(400|401|403|404))||attempts>=7;
            (if permanent{"failed"}else{"retry"},None,Some(match code{Some(c)=>format!("Discord returned HTTP {c}. Check bot access and the recipient's DM settings."),None=>"Discord delivery failed. The worker will retry temporary failures.".into()}))
        },
        Err(_)=>(if attempts>=7{"failed"}else{"retry"},None,Some("Discord delivery timed out.".into())),
    };
    sqlx::query("UPDATE notification_deliveries SET state=?,provider_message_id=?,last_error=?,lease_until=NULL,next_attempt=? WHERE id=? AND state='leased'").bind(state).bind(message_id).bind(error).bind(now()+(30*2i64.pow(attempts.min(7) as u32)).min(3600)).bind(delivery_id).execute(&s.pool).await?;
    Ok(true)
}
pub async fn run(s:AppState){
    let http=s.config.discord_token.as_ref().map(|token|Http::new(token));
    let mut last_schedule=0i64;let mut last_cleanup=0i64;
    loop{
        if now()-last_schedule>=60{if let Err(e)=schedule_due(&s,now()).await{tracing::error!(code=e.1,"Reminder scheduling failed");}last_schedule=now();}
        if now()-last_cleanup>=3600{
            let result=async{
                sqlx::query("DELETE FROM sessions WHERE expires_at<?").bind(now()).execute(&s.pool).await?;
                sqlx::query("DELETE FROM account_tokens WHERE expires_at<? OR used_at IS NOT NULL").bind(now()).execute(&s.pool).await?;
                sqlx::query("DELETE FROM discord_link_tokens WHERE expires_at<?").bind(now()).execute(&s.pool).await?;
                sqlx::query("DELETE FROM discord_confirmations WHERE expires_at<?").bind(now()).execute(&s.pool).await?;
                sqlx::query("DELETE FROM discord_interactions WHERE created_at<?").bind(now()-86400).execute(&s.pool).await?;
                Ok::<_,sqlx::Error>(())
            }.await;if result.is_err(){tracing::warn!("Expired-token cleanup failed");}last_cleanup=now();
        }
        if let Some(http)=&http{for _ in 0..10{match deliver_one(&s,http).await{Ok(true)=>{},Ok(false)=>break,Err(e)=>{tracing::warn!(code=e.1,"Notification worker failed");break;}}}}
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}
