<script lang="ts">
  import type { Snippet } from "svelte";
  let { as = "div", class: cls = "", children }: { as?: string; class?: string; children: Snippet } = $props();
</script>
<svelte:element this={as} class="mg-label {cls}">{@render children()}</svelte:element>
