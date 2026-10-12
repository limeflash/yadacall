<script lang="ts">
  import type { Snippet } from "svelte";
  import IconButton from "./IconButton.svelte";
  let { decLabel, incLabel, decDisabled = false, incDisabled = false, ondec, oninc, children }:
    { decLabel: string; incLabel: string; decDisabled?: boolean; incDisabled?: boolean; ondec: () => void; oninc: () => void; children: Snippet } = $props();
</script>

<div role="group" class="mg-stepper">
  <IconButton variant="round" aria-label={decLabel} disabled={decDisabled} onclick={ondec}><span aria-hidden="true" class="mg-stepper__minus"></span></IconButton>
  <output class="mg-stepper__value">{@render children()}</output>
  <IconButton variant="roundAccent" aria-label={incLabel} disabled={incDisabled} onclick={oninc}><span aria-hidden="true" class="mg-stepper__plus"></span></IconButton>
</div>
