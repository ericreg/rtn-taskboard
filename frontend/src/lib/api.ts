let csrf = '';
export function setCsrf(value: string) { csrf = value; }
export class ApiError extends Error { constructor(message: string, public status: number, public code: string) { super(message); } }
export async function api<T = { ok: boolean }>(path: string, method = 'GET', body?: unknown): Promise<T> {
  const headers: Record<string, string> = {};
  if (method !== 'GET') headers['X-CSRF-Token'] = csrf;
  const form = body instanceof FormData;
  if (body !== undefined && !form) headers['Content-Type'] = 'application/json';
  let response: Response;
  try { response = await fetch(`/api/v1${path}`, { method, headers, credentials: 'same-origin', body: body === undefined ? undefined : form ? body as FormData : JSON.stringify(body) }); }
  catch { throw new ApiError('Cannot reach Taskboard. Check your connection and try again.', 0, 'network'); }
  const data = await response.json().catch(() => null);
  if (!response.ok) {
    if (response.status === 401 && !path.startsWith('/auth/')) window.dispatchEvent(new Event('taskboard:session-expired'));
    throw new ApiError(data?.error?.message || `The request could not be completed (${response.status}).`, response.status, data?.error?.code || 'request');
  }
  return data as T;
}
export function navigate(path: string) { history.pushState({}, '', path); window.dispatchEvent(new Event('taskboard:navigate')); }
export function go(event: MouseEvent, path: string) { if (!event.ctrlKey && !event.metaKey && !event.shiftKey && event.button === 0) { event.preventDefault(); navigate(path); } }
export const errorMessage = (error: unknown) => error instanceof Error ? error.message : 'Something went wrong. Please try again.';

