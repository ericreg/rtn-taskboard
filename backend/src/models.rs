use crate::db::{self, FromRow, Row};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub name: String,
    pub role: String,
    pub active: bool,
    pub theme: String,
    pub timezone: String,
    pub activity_in_app: bool,
    pub activity_discord: bool,
    pub due_in_app: bool,
    pub due_discord: bool,
    pub watched_due: bool,
}
impl User {
    pub fn editor(&self) -> bool {
        self.role == "editor"
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub color: String,
    pub creator_id: i64,
    pub archived_at: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub task_count: i64,
    pub done_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Task {
    pub id: i64,
    pub project_id: i64,
    pub title: String,
    pub description: String,
    pub status: String,
    pub creator_id: i64,
    pub assignee_id: Option<i64>,
    pub due_date: Option<String>,
    pub due_timezone: String,
    pub reminder_revision: i64,
    pub version: i64,
    pub archived_at: Option<String>,
    pub archived_by: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
    pub project_name: String,
    pub project_color: String,
    pub project_archived_at: Option<String>,
    pub assignee_name: Option<String>,
    pub overdue: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Attachment {
    pub id: String,
    pub filename: String,
    pub mime: String,
    pub size: i64,
    pub created_at: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct TaskFilter {
    pub project: Option<i64>,
    pub assignee: Option<i64>,
    pub status: Option<String>,
    pub q: Option<String>,
    pub due: Option<String>,
    pub archived: Option<bool>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}

impl FromRow for User {
    fn from_row(row: Row) -> db::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            email: row.try_get("email")?,
            name: row.try_get("name")?,
            role: row.try_get("role")?,
            active: row.try_get("active")?,
            theme: row.try_get("theme")?,
            timezone: row.try_get("timezone")?,
            activity_in_app: row.try_get("activity_in_app")?,
            activity_discord: row.try_get("activity_discord")?,
            due_in_app: row.try_get("due_in_app")?,
            due_discord: row.try_get("due_discord")?,
            watched_due: row.try_get("watched_due")?,
        })
    }
}
impl FromRow for Project {
    fn from_row(row: Row) -> db::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            slug: row.try_get("slug")?,
            description: row.try_get("description")?,
            color: row.try_get("color")?,
            creator_id: row.try_get("creator_id")?,
            archived_at: row.try_get("archived_at")?,
            version: row.try_get("version")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            task_count: row.get_or_default("task_count")?,
            done_count: row.get_or_default("done_count")?,
        })
    }
}
impl FromRow for Task {
    fn from_row(row: Row) -> db::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            project_id: row.try_get("project_id")?,
            title: row.try_get("title")?,
            description: row.try_get("description")?,
            status: row.try_get("status")?,
            creator_id: row.try_get("creator_id")?,
            assignee_id: row.try_get("assignee_id")?,
            due_date: row.try_get("due_date")?,
            due_timezone: row.try_get("due_timezone")?,
            reminder_revision: row.try_get("reminder_revision")?,
            version: row.try_get("version")?,
            archived_at: row.try_get("archived_at")?,
            archived_by: row.try_get("archived_by")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            project_name: row.get_or_default("project_name")?,
            project_color: row.get_or_default("project_color")?,
            project_archived_at: row.get_or_default("project_archived_at")?,
            assignee_name: row.get_or_default("assignee_name")?,
            overdue: row.get_or_default("overdue")?,
        })
    }
}
impl FromRow for Attachment {
    fn from_row(row: Row) -> db::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            filename: row.try_get("filename")?,
            mime: row.try_get("mime")?,
            size: row.try_get("size")?,
            created_at: row.try_get("created_at")?,
        })
    }
}
