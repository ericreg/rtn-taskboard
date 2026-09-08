import { expect, test } from '@playwright/test';
import type { Task, User } from '../src/lib/types';

// Keep these UI checks independent of a running backend and real workspace data.
test.beforeEach(async ({ page }) => {
  const user: User = {
    id: 1, email: 'editor@example.test', name: 'Test Editor', role: 'editor', active: true,
    theme: 'dark', timezone: 'UTC', activity_in_app: true, activity_discord: false,
    due_in_app: true, due_discord: false, watched_due: false,
  };
  const project = { id: 1, name: 'Project Alpha', slug: 'alpha', description: '', color: '#55b897', creator_id: 1, archived_at: null, version: 1, task_count: 2, done_count: 0 };
  const tasks: Task[] = [1, 2].map(id => ({
    id, project_id: 1, title: id === 1 ? 'First task' : 'Second task', description: '', status: 'todo',
    creator_id: 1, assignee_id: null, due_date: null, due_timezone: 'UTC', reminder_revision: 1,
    version: 1, archived_at: null, archived_by: null, created_at: '2026-09-06T12:00:00Z',
    updated_at: '2026-09-06T12:00:00Z', project_name: project.name, project_color: project.color,
    project_archived_at: null, assignee_name: null, overdue: false,
  }));
  await page.route('**/api/v1/**', async route => {
    const request = route.request();
    const path = new URL(request.url()).pathname.slice('/api/v1'.length);
    let json: unknown;
    if (path === '/auth/me') json = { user, csrf: 'test-csrf' };
    else if (path === '/projects') json = [project];
    else if (path === '/users') json = [user];
    else if (path === '/status') json = { database_bytes: 4096, image_bytes: 0, max_image_bytes: 10485760, limit_bytes: 0, content_blocked: false };
    else if (path === '/notifications') json = { items: [], unread: 0, page: 1, has_more: false };
    else if (path === '/tasks') json = { items: tasks, page: 1, has_more: false };
    else if (path === '/projects/1/tasks' && request.method() === 'POST') {
      const task = { ...tasks[0], ...request.postDataJSON(), id: 3, version: 1 };
      tasks.push(task);
      json = task;
    } else if (/^\/tasks\/\d+$/.test(path)) {
      const task = tasks.find(task => task.id === Number(path.split('/')[2]));
      if (!task) throw new Error(`Unknown test task: ${path}`);
      if (request.method() === 'PATCH') { Object.assign(task, request.postDataJSON()); task.version++; json = task; }
      else json = { task, attachments: [], watchers: [], comments: [], activity: [], links: [] };
    } else if (path === '/tasks/1/attachments') json = { id: 'test-image', filename: 'pixel.png', mime: 'image/png', size: 68, created_at: '2026-09-06T12:00:00Z' };
    else throw new Error(`Unexpected test API request: ${request.method()} ${path}`);
    await route.fulfill({ json });
  });
  await page.goto('/projects/1');
  await expect(page.getByRole('link', { name: 'TB-1 First task', exact: true })).toBeVisible();
});

test('switches tasks beside the list, keeps filters, and also switches from the board', async ({ page }, testInfo) => {
  await page.getByLabel('Filter status').selectOption('all');
  await page.getByRole('link', { name: 'TB-1 First task', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(page.getByLabel('Task title', { exact: true })).toHaveValue('First task');
  expect(await dialog.evaluate(element => element.matches(':modal'))).toBe(false);
  await page.getByRole('link', { name: 'TB-2 Second task', exact: true }).click();
  await expect(dialog).toHaveAccessibleName('Task TB-2');
  await expect(page.getByLabel('Task title', { exact: true })).toHaveValue('Second task');
  await expect(page.locator('.task-row.selected')).toContainText('Second task');
  await expect(page.getByLabel('Filter status')).toHaveValue('all');
  await expect(page.getByRole('heading', { name: 'Project Alpha' })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('task-switching.png'), fullPage: true, animations: 'disabled' });
  await page.getByRole('button', { name: 'Board', exact: true }).click();
  await page.locator('.board-card').filter({ hasText: 'First task' }).click();
  await expect(dialog).toHaveAccessibleName('Task TB-1');
  await expect(page.locator('.board-card.selected')).toContainText('First task');
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);
  await expect(page).toHaveURL('/projects/1');
});

test('keeps edits and comment/link drafts when switching is canceled', async ({ page }) => {
  await page.getByRole('link', { name: 'TB-1 First task', exact: true }).click();
  await expect(page.getByLabel('Task title', { exact: true })).toHaveValue('First task');
  await page.getByLabel('Task title', { exact: true }).fill('Unsaved title');
  page.once('dialog', dialog => dialog.dismiss());
  await page.getByRole('link', { name: 'TB-2 Second task', exact: true }).click();
  await expect(page).toHaveURL('/tasks/TB-1');
  await expect(page.getByLabel('Task title', { exact: true })).toHaveValue('Unsaved title');
  page.once('dialog', dialog => dialog.accept());
  await page.getByRole('link', { name: 'TB-2 Second task', exact: true }).click();
  await expect(page.getByLabel('Task title', { exact: true })).toHaveValue('Second task');
  await page.getByLabel('Comment', { exact: true }).fill('Draft comment');
  page.once('dialog', dialog => dialog.dismiss());
  await page.getByRole('link', { name: 'TB-1 First task', exact: true }).click();
  await expect(page.getByLabel('Comment', { exact: true })).toHaveValue('Draft comment');
  page.once('dialog', dialog => dialog.accept());
  await page.getByRole('link', { name: 'TB-1 First task', exact: true }).click();
  await expect(page.getByLabel('Comment', { exact: true })).toHaveValue('');
  await page.getByRole('button', { name: 'Add link', exact: true }).click();
  await page.getByLabel('Link URL').fill('https://example.test/draft');
  page.once('dialog', dialog => dialog.dismiss());
  await page.getByRole('link', { name: 'TB-2 Second task', exact: true }).click();
  await expect(page.getByLabel('Link URL')).toHaveValue('https://example.test/draft');
  await expect(page).toHaveURL('/tasks/TB-1');
});

test('switches away from a new-task draft and still opens a successfully created task', async ({ page }) => {
  await page.getByRole('button', { name: 'New task', exact: true }).click();
  await page.getByLabel('Task title', { exact: true }).fill('Draft task');
  page.once('dialog', dialog => dialog.accept());
  await page.getByRole('link', { name: 'TB-2 Second task', exact: true }).click();
  await expect(page.getByLabel('Task title', { exact: true })).toHaveValue('Second task');
  await page.getByRole('button', { name: 'New task', exact: true }).click();
  await page.getByLabel('Task title', { exact: true }).fill('Created task');
  await page.getByRole('button', { name: 'Create task', exact: true }).last().click();
  await expect(page).toHaveURL('/tasks/TB-3');
  await expect(page.getByLabel('Task title', { exact: true })).toHaveValue('Created task');
});

test('keeps the current editor while saving or uploading an image', async ({ page }) => {
  await page.getByRole('link', { name: 'TB-1 First task', exact: true }).click();
  await expect(page.getByLabel('Task title', { exact: true })).toHaveValue('First task');
  let finishSave!: () => void;
  const saving = new Promise<void>(resolve => finishSave = resolve);
  await page.route('**/api/v1/tasks/1', async route => { if (route.request().method() === 'PATCH') await saving; await route.fallback(); });
  await page.getByLabel('Task title', { exact: true }).fill('Saved title');
  const saveRequest = page.waitForRequest(request => request.method() === 'PATCH');
  await page.getByRole('button', { name: 'Save changes', exact: true }).click();
  await saveRequest;
  await page.getByRole('link', { name: 'TB-2 Second task', exact: true }).click();
  await expect(page).toHaveURL('/tasks/TB-1');
  finishSave();
  await expect(page.getByRole('button', { name: 'Save changes', exact: true })).toBeDisabled();
  await expect(page.getByText('All changes saved', { exact: true })).toBeVisible();

  let finishUpload!: () => void;
  const uploading = new Promise<void>(resolve => finishUpload = resolve);
  await page.route('**/api/v1/tasks/1/attachments', async route => { await uploading; await route.fallback(); });
  const uploadRequest = page.waitForRequest('**/api/v1/tasks/1/attachments');
  await page.locator('.editor input[type=file]').first().setInputFiles({
    name: 'pixel.png', mimeType: 'image/png',
    buffer: Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=', 'base64'),
  });
  await uploadRequest;
  await page.getByRole('link', { name: 'TB-2 Second task', exact: true }).click();
  await expect(page).toHaveURL('/tasks/TB-1');
  finishUpload();
  await expect(page.getByLabel('Description', { exact: true })).toHaveValue(/test-image/);
  page.once('dialog', dialog => dialog.accept());
  await page.getByRole('link', { name: 'TB-2 Second task', exact: true }).click();
  await expect(page.getByLabel('Task title', { exact: true })).toHaveValue('Second task');
});

test('retains mobile modality and preserves drafts when the viewport changes', async ({ page }) => {
  await page.getByRole('link', { name: 'TB-1 First task', exact: true }).click();
  await expect(page.getByLabel('Task title', { exact: true })).toHaveValue('First task');
  await page.getByLabel('Task title', { exact: true }).fill('Responsive draft');
  await page.setViewportSize({ width: 390, height: 844 });
  await expect.poll(() => page.getByRole('dialog').evaluate(element => element.matches(':modal'))).toBe(true);
  await expect(page.getByLabel('Task title', { exact: true })).toHaveValue('Responsive draft');
  page.once('dialog', dialog => dialog.dismiss());
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toBeVisible();
  await page.setViewportSize({ width: 1600, height: 1000 });
  await expect.poll(() => page.getByRole('dialog').evaluate(element => element.matches(':modal'))).toBe(false);
  page.once('dialog', dialog => dialog.accept());
  await page.getByRole('link', { name: 'TB-2 Second task', exact: true }).click();
  await expect(page.getByLabel('Task title', { exact: true })).toHaveValue('Second task');
});
