<script lang="ts">
  let { keys }: { keys: string[] } = $props();
</script>
<span class="mg-kbds">{#each keys as k (k)}<kbd class="mg-kbd">{k}</kbd>{/each}</span>
