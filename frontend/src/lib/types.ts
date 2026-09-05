export type Status = 'todo' | 'in_progress' | 'blocked' | 'done' | 'canceled';
export interface User {
  id: number; email: string; name: string; role: 'viewer' | 'editor'; active: boolean;
  theme: 'light' | 'dark' | 'system'; timezone: string; activity_in_app: boolean;
  activity_discord: boolean; due_in_app: boolean; due_discord: boolean; watched_due: boolean;
}
export interface Project {
  id: number; name: string; slug: string; description: string; color: string; creator_id: number;
  archived_at: string | null; version: number; task_count: number; done_count: number;
  created_at: string; updated_at: string;
}
export interface Task {
  id: number; project_id: number; title: string; description: string; status: Status;
  creator_id: number; assignee_id: number | null; due_date: string | null; due_timezone: string;
  reminder_revision: number; version: number; archived_at: string | null; archived_by: number | null;
  created_at: string; updated_at: string; project_name: string; project_color: string;
  project_archived_at: string | null; assignee_name: string | null; overdue: boolean;
}
export interface Attachment { id: string; filename: string; mime: string; size: number; created_at: string }
export interface Activity { id: number; actor_name: string; detail: string; source?: string; created_at: string; task_id?: number }
export interface TaskDetail {
  task: Task; attachments: Attachment[]; watchers: { id: number; name: string }[];
  comments: { id: number; author_id: number; author_name: string; body: string; created_at: string }[];
  activity: Activity[]; links: { id: number; url: string; label: string }[];
}
export interface StorageStatus {
  database_bytes: number; database_file_bytes: number; wal_bytes: number; image_bytes: number;
  limit_bytes: number; content_blocked: boolean; discord_configured: boolean; discord_connected: boolean; failed_deliveries: number;
}
export interface Session { user: User; csrf: string; discord?: { id: string; name: string } | null; pending_discord?: { id: string; name: string } | null }
export interface Notice { id: number; task_id: number; kind: string; message: string; read_at: string | null; created_at: string }
export interface Page<T> { items: T[]; page: number; has_more: boolean }
export interface NotificationPage extends Page<Notice> { unread: number }
export const statuses: { value: Status; label: string; color: string }[] = [
  { value: 'todo', label: 'To do', color: '#9095a5' },
  { value: 'in_progress', label: 'In progress', color: '#e3a348' },
  { value: 'blocked', label: 'Blocked', color: '#d86c79' },
  { value: 'done', label: 'Done', color: '#4caa89' },
  { value: 'canceled', label: 'Canceled', color: '#9692aa' },
];
export const statusLabel = (value: string) => statuses.find(s => s.value === value)?.label || value;
export const initials = (name: string) => name.trim().split(/\s+/).slice(0, 2).map(s => s[0]).join('').toUpperCase();
export function bytes(value: number) { if (!value) return '0 B'; const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), 4); return `${(value / 1024 ** index).toFixed(index ? 1 : 0)} ${['B', 'KiB', 'MiB', 'GiB', 'TiB'][index]}`; }
export function dateLabel(date: string | null) { return date ? new Date(`${date}T12:00:00`).toLocaleDateString(undefined, { month: 'short', day: 'numeric' }) : 'No due date'; }
export function timestamp(date: string, timezone?: string) { return new Date(date).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit', ...(timezone ? { timeZone: timezone } : {}) }); }
