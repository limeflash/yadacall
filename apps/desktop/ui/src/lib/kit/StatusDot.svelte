<script lang="ts">
  let { kind }: { kind: "ok" | "warn" | "bad" | "off" | "busy" | "blip" } = $props();
</script>
<span aria-hidden="true" class="mg-dot mg-tone-{kind} {kind === 'busy' ? 'mg-dot--busy' : ''}"></span>
