CREATE TABLE users (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 email TEXT NOT NULL UNIQUE COLLATE NOCASE,
 password_hash TEXT NOT NULL,
 name TEXT NOT NULL,
 role TEXT NOT NULL DEFAULT 'viewer' CHECK(role IN ('viewer','editor')),
 active INTEGER NOT NULL DEFAULT 1,
 theme TEXT NOT NULL DEFAULT 'system' CHECK(theme IN ('system','light','dark')),
 timezone TEXT NOT NULL DEFAULT 'UTC',
 activity_in_app INTEGER NOT NULL DEFAULT 1,
 activity_discord INTEGER NOT NULL DEFAULT 1,
 due_in_app INTEGER NOT NULL DEFAULT 1,
 due_discord INTEGER NOT NULL DEFAULT 1,
 watched_due INTEGER NOT NULL DEFAULT 0,
 created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE TABLE sessions (
 token_hash TEXT PRIMARY KEY, user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 csrf TEXT NOT NULL, expires_at INTEGER NOT NULL
);
CREATE INDEX sessions_user ON sessions(user_id);
CREATE TABLE account_tokens (
 token_hash TEXT PRIMARY KEY, email TEXT NOT NULL COLLATE NOCASE,
 purpose TEXT NOT NULL CHECK(purpose IN ('invite','reset')),
 expires_at INTEGER NOT NULL, used_at INTEGER,
 created_by INTEGER REFERENCES users(id)
);
CREATE TABLE projects (
 id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, slug TEXT NOT NULL UNIQUE,
 description TEXT NOT NULL DEFAULT '', color TEXT NOT NULL DEFAULT '#5e6ad2',
 creator_id INTEGER NOT NULL REFERENCES users(id), archived_at TEXT,
 version INTEGER NOT NULL DEFAULT 1,
 created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
 updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE TABLE tasks (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE RESTRICT,
 title TEXT NOT NULL CHECK(length(title) BETWEEN 1 AND 200), description TEXT NOT NULL DEFAULT '',
 status TEXT NOT NULL DEFAULT 'todo' CHECK(status IN ('todo','in_progress','blocked','done','canceled')),
 assignee_id INTEGER REFERENCES users(id), creator_id INTEGER NOT NULL REFERENCES users(id),
 due_date TEXT, due_timezone TEXT NOT NULL DEFAULT 'UTC', due_start INTEGER, due_at INTEGER,
 reminder_revision INTEGER NOT NULL DEFAULT 1, version INTEGER NOT NULL DEFAULT 1,
 archived_at TEXT, archived_by INTEGER REFERENCES users(id),
 created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
 updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX tasks_project_status ON tasks(project_id,status) WHERE archived_at IS NULL;
CREATE INDEX tasks_assignee ON tasks(assignee_id) WHERE archived_at IS NULL;
CREATE INDEX tasks_due ON tasks(due_date) WHERE archived_at IS NULL;
CREATE INDEX tasks_archived ON tasks(archived_at) WHERE archived_at IS NOT NULL;
CREATE TABLE comments (
 id INTEGER PRIMARY KEY AUTOINCREMENT, task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
 author_id INTEGER NOT NULL REFERENCES users(id), body TEXT NOT NULL,
 created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX comments_task ON comments(task_id,id);
CREATE TABLE task_links (
 id INTEGER PRIMARY KEY AUTOINCREMENT, task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
 url TEXT NOT NULL, label TEXT NOT NULL DEFAULT '', UNIQUE(task_id,url)
);
CREATE TABLE attachments (
 id TEXT PRIMARY KEY,
 task_id INTEGER REFERENCES tasks(id) ON DELETE CASCADE,
 project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE,
 uploader_id INTEGER NOT NULL REFERENCES users(id), filename TEXT NOT NULL,
 mime TEXT NOT NULL, size INTEGER NOT NULL DEFAULT 0,
 created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
 CHECK ((task_id IS NOT NULL) != (project_id IS NOT NULL))
);
CREATE INDEX attachments_task ON attachments(task_id);
CREATE INDEX attachments_project ON attachments(project_id);
CREATE TABLE attachment_chunks (
 attachment_id TEXT NOT NULL REFERENCES attachments(id) ON DELETE CASCADE,
 sequence INTEGER NOT NULL, data BLOB NOT NULL,
 PRIMARY KEY(attachment_id,sequence)
) WITHOUT ROWID;
CREATE TABLE task_watchers (
 task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
 user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 PRIMARY KEY(task_id,user_id)
);
CREATE TABLE activity_events (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 task_id INTEGER REFERENCES tasks(id) ON DELETE CASCADE,
 project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 actor_id INTEGER NOT NULL REFERENCES users(id), kind TEXT NOT NULL, detail TEXT NOT NULL,
 source TEXT NOT NULL DEFAULT 'web',
 created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX activity_task ON activity_events(task_id,id);
CREATE INDEX activity_project ON activity_events(project_id,id);
CREATE TABLE notifications (
 id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
 event_id INTEGER REFERENCES activity_events(id) ON DELETE CASCADE,
 kind TEXT NOT NULL, message TEXT NOT NULL, dedupe_key TEXT NOT NULL UNIQUE,
 in_app INTEGER NOT NULL DEFAULT 1, read_at TEXT,
 reminder_revision INTEGER,
 created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX notifications_user ON notifications(user_id,read_at,id);
CREATE TABLE notification_deliveries (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 notification_id INTEGER NOT NULL UNIQUE REFERENCES notifications(id) ON DELETE CASCADE,
 state TEXT NOT NULL DEFAULT 'pending', attempts INTEGER NOT NULL DEFAULT 0,
 next_attempt INTEGER NOT NULL DEFAULT 0, lease_until INTEGER,
 last_error TEXT, provider_message_id TEXT
);
CREATE INDEX delivery_next ON notification_deliveries(state,next_attempt);
CREATE TABLE discord_accounts (
 user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
 discord_id TEXT NOT NULL UNIQUE, discord_name TEXT NOT NULL,
 linked_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE TABLE discord_link_tokens (
 token_hash TEXT PRIMARY KEY, user_id INTEGER NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
 expires_at INTEGER NOT NULL, discord_id TEXT, discord_name TEXT
);
CREATE TABLE discord_interactions (
 interaction_id TEXT PRIMARY KEY, user_id INTEGER NOT NULL REFERENCES users(id),
 task_id INTEGER REFERENCES tasks(id) ON DELETE CASCADE, action TEXT NOT NULL,
 created_at INTEGER NOT NULL
);
CREATE TABLE discord_confirmations (
 token TEXT PRIMARY KEY, discord_id TEXT NOT NULL, user_id INTEGER NOT NULL REFERENCES users(id),
 task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
 version INTEGER NOT NULL, expires_at INTEGER NOT NULL
);
CREATE TABLE admin_audit_log (
 id INTEGER PRIMARY KEY AUTOINCREMENT, actor_id INTEGER REFERENCES users(id),
 action TEXT NOT NULL, target TEXT NOT NULL,
 created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE TABLE app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
