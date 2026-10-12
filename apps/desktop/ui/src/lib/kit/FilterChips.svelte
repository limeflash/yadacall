<script lang="ts" generics="V extends string">
  type Opt = { value: V; label: string };
  let { options, value, onchange, label = "", class: cls = "" }:
    { options: Opt[]; value: V; onchange: (v: V) => void; label?: string; class?: string } = $props();
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

<!-- svelte-ignore a11y_interactive_supports_focus -->
<div bind:this={root} role="radiogroup" aria-label={label} class="mg-fchips {cls}" onkeydown={keys}>
  {#each options as o (o.value)}
    {@const on = o.value === value}
    <button type="button" role="radio" aria-checked={on} tabindex={on ? 0 : -1} class="mg-fchip" onclick={() => onchange(o.value)}>{o.label}</button>
  {/each}
</div>
