use serde::{Serialize, Deserialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct User {
    pub id: i64, pub email: String, pub name: String, pub role: String, pub active: bool,
    pub theme: String, pub timezone: String,
    pub activity_in_app: bool, pub activity_discord: bool, pub due_in_app: bool, pub due_discord: bool, pub watched_due: bool,
}
impl User { pub fn editor(&self) -> bool { self.role == "editor" } }

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Project {
    pub id: i64, pub name: String, pub slug: String, pub description: String, pub color: String,
    pub creator_id: i64, pub archived_at: Option<String>, pub version: i64, pub created_at: String, pub updated_at: String,
    #[sqlx(default)] pub task_count: i64, #[sqlx(default)] pub done_count: i64,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Task {
    pub id: i64, pub project_id: i64, pub title: String, pub description: String, pub status: String,
    pub creator_id: i64, pub assignee_id: Option<i64>, pub due_date: Option<String>, pub due_timezone: String,
    pub reminder_revision: i64, pub version: i64, pub archived_at: Option<String>, pub archived_by: Option<i64>,
    pub created_at: String, pub updated_at: String,
    #[sqlx(default)] pub project_name: String, #[sqlx(default)] pub project_color: String,
    #[sqlx(default)] pub project_archived_at: Option<String>,
    #[sqlx(default)] pub assignee_name: Option<String>, #[sqlx(default)] pub overdue: bool,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Attachment { pub id: String, pub filename: String, pub mime: String, pub size: i64, pub created_at: String }

#[derive(Debug, Deserialize, Default)]
pub struct TaskFilter {
    pub project: Option<i64>, pub assignee: Option<i64>, pub status: Option<String>,
    pub q: Option<String>, pub due: Option<String>, pub archived: Option<bool>, pub page: Option<i64>, pub limit: Option<i64>,
}
