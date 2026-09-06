use crate::db;
use crate::{
    auth::{self, now},
    error::{Error, Result},
    jobs::safe_discord_text,
    models::{TaskFilter, User},
    services::{self, TaskInput},
    state::AppState,
};
use serenity::{all::*, async_trait};

use std::{sync::atomic::Ordering, time::Duration};

pub fn commands() -> Vec<CreateCommand> {
    let task_option = || {
        CreateCommandOption::new(
            CommandOptionType::String,
            "task",
            "Task reference, such as TB-123",
        )
        .required(true)
    };
    let mut root = CreateCommand::new("taskboard").description("Manage company tasks");
    root = root.add_option(
        CreateCommandOption::new(CommandOptionType::SubCommand, "list", "List tasks")
            .add_sub_option(CreateCommandOption::new(
                CommandOptionType::Integer,
                "project",
                "Project ID",
            ))
            .add_sub_option(CreateCommandOption::new(
                CommandOptionType::User,
                "assignee",
                "Linked Discord assignee",
            ))
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::String,
                    "scope",
                    "Whose tasks? Defaults to everyone, including unassigned tasks.",
                )
                .add_string_choice("Everyone", "all")
                .add_string_choice("My tasks", "mine"),
            )
            .add_sub_option(
                CreateCommandOption::new(CommandOptionType::String, "status", "Task status")
                    .add_string_choice("Active", "active")
                    .add_string_choice("To do", "todo")
                    .add_string_choice("In progress", "in_progress")
                    .add_string_choice("Blocked", "blocked")
                    .add_string_choice("Done", "done")
                    .add_string_choice("Canceled", "canceled")
                    .add_string_choice("All", "all"),
            )
            .add_sub_option(
                CreateCommandOption::new(CommandOptionType::String, "due", "Due-date filter")
                    .add_string_choice("Overdue", "overdue")
                    .add_string_choice("Due today", "today")
                    .add_string_choice("Next seven days", "soon"),
            )
            .add_sub_option(
                CreateCommandOption::new(CommandOptionType::Integer, "page", "Page number")
                    .min_int_value(1),
            )
            .add_sub_option(CreateCommandOption::new(
                CommandOptionType::Boolean,
                "public",
                "Show this task list to everyone in this channel (default: false)",
            )),
    );
    for (name, description) in [
        ("view", "View a task"),
        ("finish", "Mark a task Done"),
        ("cancel", "Mark a task Canceled"),
        ("delete", "Move a task to Archive"),
        ("watch", "Watch task activity"),
        ("unwatch", "Stop watching task activity"),
    ] {
        root = root.add_option(
            CreateCommandOption::new(CommandOptionType::SubCommand, name, description)
                .add_sub_option(task_option()),
        );
    }
    root = root
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "link",
                "Link your Taskboard account",
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::String,
                    "code",
                    "Code from Taskboard settings",
                )
                .required(true),
            ),
        )
        .add_option(CreateCommandOption::new(
            CommandOptionType::SubCommand,
            "help",
            "Show available commands",
        ))
        .add_option(CreateCommandOption::new(
            CommandOptionType::SubCommand,
            "status",
            "Show database storage status",
        ));
    vec![
        root,
        CreateCommand::new("status").description("Show Taskboard database size and content limit"),
    ]
}
struct Handler {
    state: AppState,
}

fn public_task_list(name: &str, options: &[CommandDataOption]) -> bool {
    if name != "taskboard" {
        return false;
    }
    let Some(sub) = options.first().filter(|sub| sub.name == "list") else {
        return false;
    };
    let CommandDataOptionValue::SubCommand(options) = &sub.value else {
        return false;
    };
    matches!(
        option(options, "public"),
        Some(CommandDataOptionValue::Boolean(true))
    )
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, _ready: Ready) {
        self.state.discord_connected.store(true, Ordering::Relaxed);
        if let Some(guild) = self.state.config.discord_guild
            && GuildId::new(guild)
                .set_commands(&ctx.http, commands())
                .await
                .is_err()
        {
            tracing::error!(
                "Could not register Discord commands. Check the bot installation and permissions."
            );
        }
        tracing::info!("Discord bot connected");
    }
    async fn shard_stage_update(&self, _ctx: Context, event: ShardStageUpdateEvent) {
        self.state
            .discord_connected
            .store(event.new == ConnectionStage::Connected, Ordering::Relaxed);
    }
    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        match interaction {
            Interaction::Command(command) => {
                let public = public_task_list(&command.data.name, &command.data.options);
                if command
                    .create_response(
                        &ctx.http,
                        CreateInteractionResponse::Defer(
                            CreateInteractionResponseMessage::new().ephemeral(!public),
                        ),
                    )
                    .await
                    .is_err()
                {
                    return;
                }
                let (text, buttons) = match self.command(&command).await {
                    Ok(v) => v,
                    Err(e) => (e.2, Vec::new()),
                };
                if command
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new()
                            .content(text)
                            .components(buttons)
                            .allowed_mentions(CreateAllowedMentions::new()),
                    )
                    .await
                    .is_err()
                {
                    tracing::warn!("Could not complete Discord interaction response");
                }
            }
            Interaction::Component(component) => {
                if component
                    .create_response(
                        &ctx.http,
                        CreateInteractionResponse::Defer(
                            CreateInteractionResponseMessage::new().ephemeral(true),
                        ),
                    )
                    .await
                    .is_err()
                {
                    return;
                }
                let text = match self.confirm(&component).await {
                    Ok(v) => v,
                    Err(e) => e.2,
                };
                let _ = component
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new()
                            .content(text)
                            .allowed_mentions(CreateAllowedMentions::new()),
                    )
                    .await;
            }
            _ => {}
        }
    }
}
pub async fn linked_user(s: &AppState, id: u64) -> Result<User> {
    let user:Option<i64>=db::optional_scalar(&s.db.connect().await?, "SELECT d.user_id FROM discord_accounts d JOIN users u ON u.id=d.user_id WHERE d.discord_id=? AND u.active=1", turso::params![id.to_string()]).await?;
    let user = user.ok_or_else(|| {
        Error::bad(
            "Link your active Taskboard account first: open Settings → Discord in Taskboard.",
        )
    })?;
    auth::get_user(s, user).await
}
fn option<'a>(options: &'a [CommandDataOption], name: &str) -> Option<&'a CommandDataOptionValue> {
    options.iter().find(|o| o.name == name).map(|o| &o.value)
}
fn string<'a>(options: &'a [CommandDataOption], name: &str) -> Option<&'a str> {
    option(options, name).and_then(|v| v.as_str())
}
pub fn task_id(value: &str) -> Result<i64> {
    value
        .trim()
        .strip_prefix("TB-")
        .unwrap_or(value.trim())
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| Error::bad("Use a task reference such as TB-123."))
}
impl Handler {
    fn guild(&self, id: Option<GuildId>) -> Result<()> {
        if id.map(|g| g.get()) != self.state.config.discord_guild {
            return Err(Error::forbidden());
        }
        Ok(())
    }
    async fn command(
        &self,
        command: &CommandInteraction,
    ) -> Result<(String, Vec<CreateActionRow>)> {
        self.guild(command.guild_id)?;
        self.state
            .rate_limit(format!("discord:{}", command.user.id), 300)
            .await?;
        let (name, options) = if command.data.name == "status" {
            ("status", &[][..])
        } else {
            let sub = command
                .data
                .options
                .first()
                .ok_or_else(|| Error::bad("Choose a Taskboard command."))?;
            let CommandDataOptionValue::SubCommand(options) = &sub.value else {
                return Err(Error::bad("Unknown command."));
            };
            (sub.name.as_str(), options.as_slice())
        };
        if name == "link" {
            self.state
                .rate_limit(format!("discord-link:{}", command.user.id), 10)
                .await?;
            let code =
                string(options, "code").ok_or_else(|| Error::bad("Enter your linking code."))?;
            let changed=db::execute(&self.state.db.connect().await?, "UPDATE discord_link_tokens SET discord_id=?,discord_name=? WHERE token_hash=? AND expires_at>? AND discord_id IS NULL AND user_id IN (SELECT id FROM users WHERE active=1)", turso::params![command.user.id.to_string(), command.user.name.as_str(), auth::token_hash(code), now()]).await?.rows_affected();
            if changed != 1 {
                return Err(Error::bad(
                    "This code is invalid, expired, or already used. Generate another in Taskboard.",
                ));
            }
            return Ok(("Now return to Taskboard Settings and confirm your Discord identity to finish linking.".into(),vec![]));
        }
        let user = linked_user(&self.state, command.user.id.get()).await?;
        if name == "status" {
            let status = self.state.storage().await?;
            let limit = if status.limit_bytes == 0 {
                "Unlimited".into()
            } else {
                format!(
                    "{} bytes ({:.2} MiB)",
                    status.limit_bytes,
                    status.limit_bytes as f64 / 1_048_576.0
                )
            };
            return Ok((
                format!(
                    "**Taskboard storage**\nDatabase: {} bytes ({:.2} MiB)\nImage content: {} bytes\nMaximum per image: {} bytes ({:.2} MiB)\nContent threshold: {limit}\nNew content: {}\nMain file: {} bytes · WAL: {} bytes\nThe threshold counts used database pages, including images; deleted pages are reusable and WAL is reported separately.",
                    status.database_bytes,
                    status.database_bytes as f64 / 1_048_576.0,
                    status.image_bytes,
                    status.max_image_bytes,
                    status.max_image_bytes as f64 / 1_048_576.0,
                    if status.content_blocked {
                        "blocked"
                    } else {
                        "allowed"
                    },
                    status.database_file_bytes,
                    status.wal_bytes
                ),
                vec![],
            ));
        }
        if name == "help" {
            return Ok(("Use `/taskboard list`, `view`, `finish`, `cancel`, `delete`, `watch`, or `unwatch`. Finish marks Done; cancel keeps a Canceled task; delete moves it to Archive after confirmation. Restore and permanent deletion are available in the website. `/status` shows database size and the configured threshold.".into(),vec![]));
        }
        if name == "list" {
            let mut assignee = if string(options, "scope") == Some("mine") {
                Some(user.id)
            } else {
                None
            };
            if let Some(CommandDataOptionValue::User(id)) = option(options, "assignee") {
                assignee = Some(linked_user(&self.state, id.get()).await?.id);
            }
            let filter = TaskFilter {
                project: option(options, "project").and_then(|v| v.as_i64()),
                assignee,
                status: Some(string(options, "status").unwrap_or("active").into()),
                due: string(options, "due").map(String::from),
                page: option(options, "page").and_then(|v| v.as_i64()),
                limit: Some(8),
                ..Default::default()
            };
            let result = services::list_tasks(&self.state, &filter).await?;
            let items = result["items"].as_array().unwrap();
            let mut content = format!("**Taskboard · page {}**\n", result["page"]);
            for item in items {
                let title: String = item["title"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(70)
                    .collect();
                let id = item["id"].as_i64().unwrap();
                let assignee = item["assignee_name"].as_str().unwrap_or("Unassigned");
                content.push_str(&format!(
                    "[TB-{id}]({}/tasks/TB-{id}) · {} · Assignee: {}\n",
                    self.state.config.base_url,
                    safe_discord_text(&title),
                    safe_discord_text(assignee)
                ));
            }
            if items.is_empty() {
                content.push_str("No matching tasks.");
            }
            if result["has_more"] == true {
                content.push_str("Use the page option to see more tasks.");
            }
            return Ok((content, vec![]));
        }
        let id = task_id(string(options, "task").ok_or_else(|| Error::bad("Choose a task."))?)?;
        let task = services::get_task(&self.state, id).await?;
        match name {
            "view" => Ok((
                format!(
                    "**TB-{id} · {}**\nProject: {}\nStatus: {}{}\n{}/tasks/TB-{id}",
                    safe_discord_text(&task.title),
                    safe_discord_text(&task.project_name),
                    task.status,
                    task.due_date
                        .map(|d| format!("\nDue: {d} ({})", task.due_timezone))
                        .unwrap_or_default(),
                    self.state.config.base_url
                ),
                vec![],
            )),
            "finish" | "cancel" => {
                let mut input = TaskInput::from(&task);
                input.status = if name == "finish" { "done" } else { "canceled" }.into();
                services::save_task(
                    &self.state,
                    &user,
                    id,
                    input,
                    "discord",
                    Some(&command.id.to_string()),
                )
                .await?;
                Ok((
                    format!(
                        "TB-{id} marked {}. {}/tasks/TB-{id}",
                        if name == "finish" { "Done" } else { "Canceled" },
                        self.state.config.base_url
                    ),
                    vec![],
                ))
            }
            "watch" | "unwatch" => {
                services::watch(&self.state, &user, id, name == "watch").await?;
                Ok((
                    format!(
                        "{} TB-{id}.",
                        if name == "watch" {
                            "Watching"
                        } else {
                            "Stopped watching"
                        }
                    ),
                    vec![],
                ))
            }
            "delete" => {
                services::editable(&task)?;
                let token = auth::random_token();
                db::execute(&self.state.db.connect().await?, "INSERT INTO discord_confirmations(token,discord_id,user_id,task_id,version,expires_at) VALUES(?,?,?,?,?,?)", turso::params![token.as_str(), command.user.id.to_string(), user.id, id, task.version, now()+120]).await?;
                Ok((
                    format!(
                        "Move TB-{id} · {} to Archive? You can restore it from Taskboard. This confirmation expires in two minutes.",
                        safe_discord_text(&task.title)
                    ),
                    vec![CreateActionRow::Buttons(vec![
                        CreateButton::new(format!("archive:{token}"))
                            .label("Move to Archive")
                            .style(ButtonStyle::Danger),
                        CreateButton::new(format!("cancel:{token}"))
                            .label("Keep task")
                            .style(ButtonStyle::Secondary),
                    ])],
                ))
            }
            _ => Err(Error::bad("Unknown Taskboard command.")),
        }
    }
    async fn confirm(&self, component: &ComponentInteraction) -> Result<String> {
        self.guild(component.guild_id)?;
        let user = linked_user(&self.state, component.user.id.get()).await?;
        let (action, token) = component
            .data
            .custom_id
            .split_once(':')
            .ok_or_else(|| Error::bad("Invalid confirmation."))?;
        if !["archive", "cancel"].contains(&action) {
            return Err(Error::bad("Invalid confirmation."));
        }
        let row=db::optional(&self.state.db.connect().await?, "SELECT task_id,version FROM discord_confirmations WHERE token=? AND discord_id=? AND user_id=? AND expires_at>?", turso::params![token, component.user.id.to_string(), user.id, now()]).await?.ok_or_else(||Error::bad("This confirmation expired or belongs to another user."))?;
        let task: i64 = row.get("task_id");
        if action == "archive" {
            services::archive_task(
                &self.state,
                &user,
                task,
                row.get("version"),
                "archive",
                "discord",
                Some(&component.id.to_string()),
            )
            .await?;
        }
        db::execute(
            &self.state.db.connect().await?,
            "DELETE FROM discord_confirmations WHERE token=?",
            turso::params![token],
        )
        .await?;
        Ok(if action == "archive" {
            format!("TB-{task} moved to Archive. Restore it from Taskboard if needed.")
        } else {
            "The task was kept.".into()
        })
    }
}
pub async fn run(state: AppState) {
    let Some(token) = state.config.discord_token.clone() else {
        return;
    };
    loop {
        match Client::builder(&token, GatewayIntents::GUILDS)
            .event_handler(Handler {
                state: state.clone(),
            })
            .await
        {
            Ok(mut client) => {
                if client.start().await.is_err() {
                    tracing::warn!("Discord connection stopped; retrying in 30 seconds");
                }
            }
            Err(_) => {
                tracing::warn!("Could not initialize Discord; check the bot token and connectivity")
            }
        }
        state.discord_connected.store(false, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_lists_are_private_unless_explicitly_shared() {
        let options = |subcommand: &str, public: Option<bool>| -> Vec<CommandDataOption> {
            serde_json::from_value(serde_json::json!([{
                "name": subcommand,
                "type": 1,
                "options": public.into_iter().map(|value| serde_json::json!({
                    "name": "public", "type": 5, "value": value,
                })).collect::<Vec<_>>(),
            }]))
            .unwrap()
        };
        assert!(!public_task_list("taskboard", &options("list", None)));
        assert!(!public_task_list(
            "taskboard",
            &options("list", Some(false))
        ));
        assert!(public_task_list("taskboard", &options("list", Some(true))));
        assert!(!public_task_list("taskboard", &options("link", Some(true))));
        assert!(!public_task_list("status", &options("list", Some(true))));
        assert!(!public_task_list("taskboard", &[]));
    }
}
