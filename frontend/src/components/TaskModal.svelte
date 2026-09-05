<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { X, ArrowUpRight, Eye, EyeOff, Archive, RotateCcw, Trash2, MessageSquare, Activity as ActivityIcon, Link, Plus, Image } from '@lucide/svelte';
  import { api, errorMessage } from '../lib/api';
  import { statuses, initials, timestamp, bytes, type Task, type TaskDetail, type User, type Project } from '../lib/types';
  import Markdown from './Markdown.svelte';
  import MarkdownEditor from './MarkdownEditor.svelte';
  let { id, projects, users, user, defaultProject, onclose, onsaved }: { id: number | null; projects: Project[]; users: User[]; user: User; defaultProject?: number; onclose: () => void; onsaved: (task: Task | null, created?: boolean) => void } = $props();
  const initialProject = untrack(() => defaultProject ? String(defaultProject) : '');
  let dialog: HTMLDialogElement; let data = $state<TaskDetail | null>(null); let loading = $state(untrack(() => !!id)); let busy = $state(false); let error = $state('');
  let title = $state(''); let description = $state(''); let status = $state('todo'); let assignee = $state(''); let due = $state(''); let projectId = $state(initialProject);
  let comment = $state(''); let linkUrl = $state(''); let linkLabel = $state(''); let addingLink = $state(false); let tab = $state('comments');
  let savedSnapshot = $state(JSON.stringify(['','','todo','','',initialProject]));
  const dirty = $derived(JSON.stringify([title,description,status,assignee,due,projectId]) !== savedSnapshot);
  const readOnly = $derived(user.role === 'viewer' || !!(data?.task.archived_at || data?.task.project_archived_at));
  const watching = $derived(data?.watchers.some(w => w.id === user.id) || false);
  async function load(reset = true) {
    if (!id) return; const loaded = await api<TaskDetail>(`/tasks/${id}`); data = loaded;
    if (reset) { title = loaded.task.title; description = loaded.task.description; status = loaded.task.status; assignee = loaded.task.assignee_id ? String(loaded.task.assignee_id) : ''; due = loaded.task.due_date || ''; projectId = String(loaded.task.project_id); savedSnapshot = JSON.stringify([title,description,status,assignee,due,projectId]); }
  }
  onMount(() => { dialog.showModal(); void load().catch(e => error = errorMessage(e)).finally(() => loading = false); const unload = (e: BeforeUnloadEvent) => { if (dirty || comment.trim()) e.preventDefault(); }; window.addEventListener('beforeunload', unload); return () => window.removeEventListener('beforeunload', unload); });
  function close() { if (!busy && ((!dirty && !comment.trim()) || confirm('Discard your unsaved changes?'))) onclose(); }
  async function save(e: SubmitEvent) {
    e.preventDefault(); if (readOnly) return; busy = true; error = '';
    try { const task = await api<Task>(id ? `/tasks/${id}` : `/projects/${projectId}/tasks`, id ? 'PATCH' : 'POST', { title, description, status, assignee_id: assignee ? Number(assignee) : null, due_date: due || null, version: data?.task.version }); if (id) await load(); savedSnapshot = JSON.stringify([title,description,status,assignee,due,projectId]); onsaved(task, !id); }
    catch (e) { error = errorMessage(e); } finally { busy = false; }
  }
  async function action(action: string) {
    if (!id || !data) return;
    const question = action === 'purge' ? `Permanently delete TB-${id}, including comments and images? This cannot be undone.` : action === 'archive' ? `Move TB-${id} to Archive? You can restore it later.` : '';
    if ((dirty && !confirm('Discard unsaved edits and continue?')) || (question && !confirm(question))) return;
    busy = true; error = '';
    try { const suffix = action === 'purge' ? '/permanent' : action === 'restore' ? '/restore' : ''; await api(`/tasks/${id}${suffix}`, action === 'restore' ? 'POST' : 'DELETE', { version: data.task.version }); if (action === 'purge') { onsaved(null); onclose(); } else { await load(); onsaved(data.task); } }
    catch (e) { error = errorMessage(e); } finally { busy = false; }
  }
  async function toggleWatch() { busy = true; error = ''; try { await api(`/tasks/${id}/watchers/me`, watching ? 'DELETE' : 'PUT'); await load(false); } catch (e) { error = errorMessage(e); } finally { busy = false; } }
  async function addComment(e: SubmitEvent) { e.preventDefault(); busy = true; error = ''; try { await api(`/tasks/${id}/comments`, 'POST', { body: comment }); comment = ''; await load(false); onsaved(data!.task); } catch (e) { error = errorMessage(e); } finally { busy = false; } }
  async function addLink(e: SubmitEvent) { e.preventDefault(); busy = true; error = ''; try { await api(`/tasks/${id}/links`, 'POST', { url: linkUrl, label: linkLabel }); linkUrl = ''; linkLabel = ''; addingLink = false; await load(false); } catch (e) { error = errorMessage(e); } finally { busy = false; } }
  async function removeItem(path: string, question: string) { if (!confirm(question)) return; busy = true; error = ''; try { await api(path, 'DELETE'); await load(false); } catch (e) { error = errorMessage(e); } finally { busy = false; } }
</script>
<dialog class="modal task-panel" aria-label={id ? `Task TB-${id}` : 'Create a task'} bind:this={dialog} oncancel={(e) => { e.preventDefault(); close(); }}>
  <div class="modal-top"><span>{#if data}<span class="project-dot" style:--project-color={data.task.project_color}></span>{data.task.project_name}<span class="slash">/</span><span class="mono">TB-{id}</span>{:else}Create a task{/if}</span><div class="row">{#if id}<a class="icon-button" href={`/tasks/TB-${id}`} title="Task permalink" aria-label="Task permalink"><ArrowUpRight size={17}/></a>{/if}<button class="icon-button" aria-label="Close task" onclick={close}><X size={20}/></button></div></div>
  {#if loading}<div class="loading-state"><span class="spinner"></span>Opening task…</div>
  {:else}<div class="task-modal-body">
    {#if error}<div class="notice danger" role="alert">{error}{#if id}<button type="button" class="text-button" onclick={() => { if (!dirty || confirm('Reload and discard unsaved edits?')) void load().then(() => error = '').catch(e => error = errorMessage(e)); }}>Reload task</button>{/if}</div>{/if}
    {#if readOnly}<div class="notice"><Archive size={17}/><span>{data?.task.archived_at ? 'This task is in the Archive. Its history and images are preserved.' : data?.task.project_archived_at ? 'This project is archived. An editor can restore it.' : 'You have viewer access. Task content is read-only.'}</span></div>{/if}
    <form onsubmit={save} id="task-edit-form">
      <label class="sr-only" for="task-title">Task title</label><input id="task-title" class="task-title-input" bind:value={title} placeholder="What needs to be done?" required maxlength="200" disabled={readOnly}/>
      <div class="task-properties">
        {#if !id}<label>Project<select bind:value={projectId} required><option value="" disabled>Choose a project</option>{#each projects.filter(p => !p.archived_at) as project}<option value={String(project.id)}>{project.name}</option>{/each}</select></label>{/if}
        <label>Status<select bind:value={status} disabled={readOnly}>{#each statuses as item}<option value={item.value}>{item.label}</option>{/each}</select></label>
        <label>Assignee<select bind:value={assignee} disabled={readOnly}><option value="">Unassigned</option>{#each users.filter(u => u.active || String(u.id) === assignee) as person}<option value={String(person.id)}>{person.name}{person.active ? '' : ' (deactivated)'}</option>{/each}</select></label>
        <label>Due date<input type="date" bind:value={due} disabled={readOnly}/></label>
      </div>
      {#if due}<p class="field-hint">Due at the end of {due} in {data?.task.due_timezone || user.timezone}. The deadline is stored as a UTC instant.</p>{/if}
      <div class="section-label">Description</div>
      {#if readOnly}{#if description}<Markdown value={description}/>{:else}<p class="muted">No description added.</p>{/if}{:else}<MarkdownEditor bind:value={description} uploadTo={id ? `/tasks/${id}/attachments` : undefined} onuploaded={() => void load(false).catch(e => error = errorMessage(e))}/>{/if}
      {#if !readOnly}<div class="task-save-row"><span class="field-hint">{dirty ? 'You have unsaved changes' : id ? 'All changes saved' : 'Keep it simple. Add more detail later.'}</span><button type="submit" class="button primary" disabled={busy || (id !== null && !dirty)}>{busy ? 'Saving…' : id ? 'Save changes' : 'Create task'}</button></div>{/if}
    </form>
    {#if data}
      <div class="task-secondary-actions"><button class="button subtle" disabled={busy || (!!(data.task.archived_at || data.task.project_archived_at) && !watching)} onclick={toggleWatch}>{#if watching}<EyeOff size={15}/>Unwatch{:else}<Eye size={15}/>Watch task{/if}</button><span class="field-hint">{data.watchers.length} {data.watchers.length === 1 ? 'watcher' : 'watchers'}</span><div class="spacer"></div>
        {#if data.task.archived_at && user.role === 'editor'}{#if !data.task.project_archived_at}<button class="button subtle" disabled={busy} onclick={() => action('restore')}><RotateCcw size={15}/>Restore</button>{/if}<button class="button danger subtle" disabled={busy} onclick={() => action('purge')}><Trash2 size={15}/>Permanently delete</button>
        {:else if !readOnly}<button class="button subtle muted" disabled={busy} onclick={() => action('archive')}><Archive size={15}/>Move to Archive</button>{/if}
      </div>
      {#if data.attachments.length}<section class="task-section"><div class="section-label"><Image size={15}/>Images <span class="count">{data.attachments.length}</span></div><div class="attachments">{#each data.attachments as image}<div class="attachment"><a href={`/api/v1/attachments/${image.id}`} target="_blank" rel="noreferrer"><img src={`/api/v1/attachments/${image.id}`} alt={image.filename} loading="lazy"/><span>{image.filename}<small>{bytes(image.size)}</small></span></a>{#if !readOnly}<button class="icon-button" title="Delete image" aria-label={`Delete ${image.filename}`} onclick={() => removeItem(`/attachments/${image.id}`, 'Delete this image? Existing Markdown references will stop displaying it.')}><X size={14}/></button>{/if}</div>{/each}</div></section>{/if}
      <section class="task-section"><div class="section-label"><Link size={15}/>Links{#if !readOnly}<button class="text-button" onclick={() => addingLink = !addingLink}><Plus size={13}/>Add link</button>{/if}</div>
        {#each data.links as link}<div class="task-link"><Link size={15}/><a href={link.url} target="_blank" rel="noopener noreferrer">{link.label || link.url}</a>{#if !readOnly}<button class="icon-button" aria-label="Remove link" onclick={() => removeItem(`/tasks/${id}/links/${link.id}`, 'Remove this link?')}><X size={14}/></button>{/if}</div>{/each}
        {#if !data.links.length && !addingLink}<p class="field-hint">Connect an issue, a pull request, or a useful reference.</p>{/if}
        {#if addingLink}<form class="link-form" onsubmit={addLink}><input type="url" bind:value={linkUrl} placeholder="https://github.com/team/repo/issues/123" aria-label="Link URL" required/><input bind:value={linkLabel} placeholder="Label (optional)" aria-label="Link label" maxlength="200"/><button class="button primary" disabled={busy}>Add</button></form>{/if}
      </section>
      <section class="discussion"><div class="tabs"><button class:active={tab === 'comments'} onclick={() => tab = 'comments'}><MessageSquare size={15}/>Comments<span class="count">{data.comments.length}</span></button><button class:active={tab === 'activity'} onclick={() => tab = 'activity'}><ActivityIcon size={15}/>Activity</button></div>
        {#if tab === 'comments'}<div class="comments">{#each data.comments as item}<article class="comment"><div class="avatar small">{initials(item.author_name)}</div><div class="comment-content"><div class="comment-byline"><strong>{item.author_name}</strong><time>{timestamp(item.created_at, user.timezone)}</time></div><Markdown value={item.body}/></div></article>{:else}<p class="empty-discussion">A little context goes a long way. Start the conversation.</p>{/each}</div>
          {#if !readOnly}<form onsubmit={addComment}><MarkdownEditor bind:value={comment} label="Comment" placeholder="Share an update or ask a question…" compact uploadTo={`/tasks/${id}/attachments`} onuploaded={() => void load(false).catch(e => error = errorMessage(e))}/><div class="task-save-row"><span></span><button class="button secondary" disabled={busy || !comment.trim()}>Post comment</button></div></form>{/if}
        {:else}<ol class="activity-list">{#each data.activity as item}<li><span class="activity-dot"></span><div><strong>{item.actor_name}</strong> {item.detail}<small>{timestamp(item.created_at, user.timezone)}{item.source === 'discord' ? ' · via Discord' : ''}</small></div></li>{/each}</ol>{/if}
      </section>
    {/if}
  </div>{/if}
</dialog>

<style>
  .task-panel {
    position: fixed;
    inset: 0 0 0 auto;
    margin: 0;
    width: var(--task-panel-width);
    max-width: 100vw;
    height: 100dvh;
    max-height: 100dvh;
    border: 0;
    border-left: 1px solid var(--border);
    border-radius: 0;
    box-shadow: -12px 0 36px #17172914;
    overscroll-behavior: contain;
  }

  .task-panel::backdrop {
    background: transparent;
    backdrop-filter: none;
  }

  .modal-top > span {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .modal-top > .row {
    flex-shrink: 0;
  }

  @media (max-width: 760px) {
    .task-panel {
      width: 100vw;
      border-left: 0;
    }
  }
</style>
