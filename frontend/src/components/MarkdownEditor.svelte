<script lang="ts">
  import { Bold, Italic, Code, ImagePlus, Upload } from '@lucide/svelte';
  import Markdown from './Markdown.svelte';
  import { api, errorMessage } from '../lib/api';
  import type { Attachment } from '../lib/types';
  let { value = $bindable(''), label = 'Description', placeholder = 'Add context, a checklist, or a little code…', uploadTo, onuploaded, compact = false }: { value?: string; label?: string; placeholder?: string; uploadTo?: string; onuploaded?: () => void; compact?: boolean } = $props();
  let preview = $state(false); let uploading = $state(false); let error = $state('');
  let textarea = $state<HTMLTextAreaElement>(); let fileInput: HTMLInputElement;
  const id = $props.id();
  function insert(before: string, after = '') { const start = textarea?.selectionStart ?? value.length; const end = textarea?.selectionEnd ?? start; const selected = value.slice(start, end); value = value.slice(0, start) + before + selected + after + value.slice(end); textarea?.focus(); }
  async function upload(files: FileList | File[]) {
    if (!uploadTo) { error = 'Save the task first, then upload an image.'; return; }
    uploading = true; error = '';
    try { for (const file of Array.from(files)) { const form = new FormData(); form.append('file', file); const image = await api<Attachment>(uploadTo, 'POST', form); value += `\n\n![${image.filename.replace(/[\[\]\\]/g, '')}](/api/v1/attachments/${image.id})\n`; } onuploaded?.(); }
    catch (e) { error = errorMessage(e); } finally { uploading = false; if (fileInput) fileInput.value = ''; }
  }
  function paste(event: ClipboardEvent) { const files = event.clipboardData?.files; if (files?.length) { event.preventDefault(); void upload(files); } }
</script>
<div class:compact class="editor">
  <div class="editor-toolbar">
    <div class="editor-tabs"><button type="button" class:active={!preview} onclick={() => preview = false}>Write</button><button type="button" class:active={preview} onclick={() => preview = true}>Preview</button></div>
    <div class="editor-tools">
      <button type="button" class="icon-button" title="Bold" aria-label="Bold" disabled={preview} onclick={() => insert('**', '**')}><Bold size={15}/></button>
      <button type="button" class="icon-button" title="Italic" aria-label="Italic" disabled={preview} onclick={() => insert('_', '_')}><Italic size={15}/></button>
      <button type="button" class="icon-button" title="Code block" aria-label="Insert code block" disabled={preview} onclick={() => insert('\n```typescript\n', '\n```\n')}><Code size={16}/></button>
      <button type="button" class="icon-button" title={uploadTo ? 'Upload image' : 'Save first to upload images'} aria-label="Upload image" disabled={uploading || !uploadTo} onclick={() => fileInput.click()}><ImagePlus size={16}/></button>
      <input hidden bind:this={fileInput} type="file" accept="image/png,image/jpeg,image/webp,image/gif" multiple onchange={(e) => { if (e.currentTarget.files) void upload(e.currentTarget.files); }}/>
    </div>
  </div>
  {#if preview}<div class="editor-preview">{#if value}<Markdown {value}/>{:else}<p class="muted">Nothing to preview yet.</p>{/if}</div>
  {:else}<label class="sr-only" for={id}>{label}</label><textarea id={id} bind:this={textarea} bind:value {placeholder} spellcheck="true" onpaste={paste} ondragover={(e) => { if (e.dataTransfer?.types.includes('Files')) e.preventDefault(); }} ondrop={(e) => { if (e.dataTransfer?.files.length) { e.preventDefault(); void upload(e.dataTransfer.files); } }}></textarea>{/if}
  <div class="editor-hint">{#if uploading}<Upload size={12}/> Uploading image…{:else}<span>Markdown supported</span><span>{uploadTo ? 'Paste or drop images' : 'Save first to add images'}</span>{/if}</div>
</div>
{#if error}<p class="form-error" role="alert">{error}</p>{/if}
