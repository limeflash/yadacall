<script lang="ts" generics="V extends string">
  /** Multi-select filter chips: independent toggle buttons (`aria-pressed`), each with an optional live count. */
  type Opt = { value: V; label: string; count?: number };
  let { options, selected, ontoggle, label = "", class: cls = "" }:
    { options: Opt[]; selected: ReadonlySet<V>; ontoggle: (v: V) => void; label?: string; class?: string } = $props();
</script>

<div role="group" aria-label={label} class="chips {cls}">
  {#each options as o (o.value)}
    {@const on = selected.has(o.value)}
    <button type="button" class="chip yc-press" class:on class:empty={o.count === 0 && !on} aria-pressed={on} onclick={() => ontoggle(o.value)}>
      {#if on}<svg class="tick" width="11" height="11" viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12.5l4.5 4.5L19 7.5" /></svg>{/if}{o.label}{#if o.count !== undefined}<span class="n mg-mono">{o.count}</span>{/if}
    </button>
  {/each}
</div>

<style>
  .chips { display: flex; flex-wrap: wrap; gap: 6px; }
  .chip {
    display: inline-flex; align-items: center; gap: 6px; height: 30px; padding: 0 12px; border: 1px solid var(--border); border-radius: 15px;
    background: var(--surface); color: var(--muted); font: inherit; font-size: 12px; font-weight: 700; white-space: nowrap; cursor: pointer;
    transition: color 0.2s, background-color 0.2s, border-color 0.2s;
  }
  .chip:hover { color: var(--text); }
  .chip.on { border-color: var(--accent-line); background: var(--accent-soft); color: var(--text); }
  .chip.empty { opacity: 0.55; }
  .chip:focus-visible { outline: 2px solid var(--accent-line); outline-offset: 2px; }
  .tick { fill: none; stroke: var(--accent-text); stroke-width: 3; stroke-linecap: round; stroke-linejoin: round; }
  .n { font-size: 11px; font-weight: 700; color: var(--muted); font-variant-numeric: tabular-nums; }
  .chip.on .n { color: var(--accent-text); }
</style>
