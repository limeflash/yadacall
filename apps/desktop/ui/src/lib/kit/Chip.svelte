<script lang="ts">
  import type { Snippet } from "svelte";
  let { mono = false, class: cls = "", children }: { mono?: boolean; class?: string; children?: Snippet } = $props();
</script>
<span class="mg-chip {mono ? 'mg-mono' : ''} {cls}">{@render children?.()}</span>
