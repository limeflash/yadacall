<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";
  let { variant = "secondary", size = "md", full = false, class: cls = "", children, ...rest }:
    { variant?: "primary" | "secondary" | "outline" | "ghost" | "danger" | "ghostDanger"; size?: "lg" | "md" | "sm" | "xs"; full?: boolean; class?: string; children?: Snippet } & HTMLButtonAttributes = $props();
</script>

<button type="button" class="mg-btn mg-btn--{variant} mg-btn--{size} {full ? 'mg-btn--full' : ''} {cls}" {...rest}>{@render children?.()}</button>
