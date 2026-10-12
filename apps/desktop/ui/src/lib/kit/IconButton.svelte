<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";
  let { variant = "square", class: cls = "", children, ...rest }:
    { variant?: "square" | "close" | "round" | "roundAccent" | "flat" | "field"; class?: string; children?: Snippet } & HTMLButtonAttributes = $props();
</script>

<button type="button" class="mg-ib mg-ib--{variant} {cls}" {...rest}>{@render children?.()}</button>
