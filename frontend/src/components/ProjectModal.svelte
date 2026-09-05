<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { X, Folder } from '@lucide/svelte';
  import MarkdownEditor from './MarkdownEditor.svelte';
  import { api, errorMessage } from '../lib/api';
  import type { Project } from '../lib/types';
  let { project, onclose, onsaved }: { project?: Project; onclose: () => void; onsaved: (project: Project) => void } = $props();
  let dialog: HTMLDialogElement; let name = $state(untrack(() => project?.name || '')); let description = $state(untrack(() => project?.description || '')); let color = $state(untrack(() => project?.color || '#5e6ad2')); let busy = $state(false); let error = $state('');
  const colors = ['#5e6ad2','#4caa89','#dc9862','#cb7298','#669fc6','#8d78c4'];
  onMount(() => dialog.showModal());
  function close() { if (!busy && ((name === (project?.name || '') && description === (project?.description || '') && color === (project?.color || '#5e6ad2')) || confirm('Discard unsaved project changes?'))) onclose(); }
  async function save(e: SubmitEvent) { e.preventDefault(); busy = true; error = ''; try { const saved = await api<Project>(project ? `/projects/${project.id}` : '/projects', project ? 'PATCH' : 'POST', { name, description, color, version: project?.version }); onsaved(saved); } catch (e) { error = errorMessage(e); } finally { busy = false; } }
</script>
<dialog class="modal project-modal" bind:this={dialog} oncancel={(e) => { e.preventDefault(); close(); }}>
  <div class="modal-top"><span><Folder size={16}/> {project ? 'Edit project' : 'New project'}</span><button class="icon-button" aria-label="Close project editor" onclick={close}><X size={20}/></button></div>
  <form onsubmit={save} class="project-form"><h2>{project ? 'A little more clarity.' : 'Make space for good work.'}</h2><p class="muted">Projects bring related tasks and conversations together.</p>
    <label for="project-name">Project name</label><input id="project-name" bind:value={name} required maxlength="100" placeholder="e.g. Website refresh"/>
    <fieldset class="color-picker"><legend>Project color</legend>{#each colors as choice}<label class:selected={color === choice} style:--swatch={choice}><input type="radio" name="color" value={choice} bind:group={color} aria-label={`Color ${choice}`}/><span></span></label>{/each}</fieldset>
    <div class="section-label">Description</div><MarkdownEditor bind:value={description} label="Project description" uploadTo={project ? `/projects/${project.id}/attachments` : undefined}/>
    {#if error}<div class="form-error" role="alert">{error}</div>{/if}
    <div class="modal-footer"><span class="muted">Visible to everyone in the workspace</span><button class="button primary" disabled={busy}>{busy ? 'Saving…' : project ? 'Save changes' : 'Create project'}</button></div>
  </form>
</dialog>
