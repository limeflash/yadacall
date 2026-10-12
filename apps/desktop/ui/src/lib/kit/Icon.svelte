<script lang="ts">
  import { iconPaths } from "./icons";
  let { name, size = 16, class: cls = "", ...rest }: { name: string; size?: number; class?: string; [k: string]: unknown } = $props();
</script>

<svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" class="mg-icon {cls}" {...rest}><path d={iconPaths[name] ?? ""} /></svg>
