<script lang="ts" generics="V extends string">
  type Opt = { value: V; label: string; title?: string };
  let { options, value, onchange, variant = "inset", label = "", class: cls = "" }:
    { options: Opt[]; value: V; onchange: (v: V) => void; variant?: "thumb" | "inset" | "flat" | "lang"; label?: string; class?: string } = $props();
  let root: HTMLElement;
  const index = $derived(Math.max(0, options.findIndex((o) => o.value === value)));
  function keys(e: KeyboardEvent) {
    const d: Record<string, number> = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 };
    if (!(e.key in d)) return;
    e.preventDefault();
    const next = (index + d[e.key] + options.length) % options.length;
    onchange(options[next].value);
    (root.querySelectorAll<HTMLElement>('[role="radio"]')[next])?.focus();
  }
</script>

<div bind:this={root} role="radiogroup" aria-label={label} class="mg-seg mg-seg--{variant} {cls}" onkeydown={keys}>
  {#if variant === "thumb"}
    <span aria-hidden="true" class="mg-seg__thumb" style="width: calc((100% - 6px) / {options.length}); transform: translateX({index * 100}%)"></span>
  {/if}
  {#each options as o (o.value)}
    {@const on = o.value === value}
    <button type="button" role="radio" aria-checked={on} tabindex={on ? 0 : -1} title={o.title} class="mg-seg__item" onclick={() => onchange(o.value)}>{o.label}</button>
  {/each}
</div>
