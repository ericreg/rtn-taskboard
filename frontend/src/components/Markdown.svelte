<script lang="ts">
  import { renderMarkdown } from '../lib/markdown';
  let { value }: { value: string } = $props();
</script>
<div class="markdown">{@html renderMarkdown(value)}</div>

