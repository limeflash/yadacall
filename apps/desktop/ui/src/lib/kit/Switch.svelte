<script lang="ts">
  import type { HTMLButtonAttributes } from "svelte/elements";
  let { checked = $bindable(false), onchange, ...rest }: { checked?: boolean; onchange?: (v: boolean) => void } & Omit<HTMLButtonAttributes, "onchange"> = $props();
  function flip() { checked = !checked; onchange?.(checked); }
</script>

<button type="button" role="switch" aria-checked={checked} class="mg-switch" onclick={flip} {...rest}><span class="mg-switch__thumb"></span></button>
