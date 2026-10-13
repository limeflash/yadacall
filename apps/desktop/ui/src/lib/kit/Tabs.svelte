<script lang="ts" generics="V extends string">
  /** A real ARIA tablist (arrow keys / Home / End, roving tabindex) with an optional count per tab. */
  type Tab = { value: V; label: string; count?: number };
  let { tabs, value, onchange, label = "", idPrefix = "tab", class: cls = "" }:
    { tabs: Tab[]; value: V; onchange: (v: V) => void; label?: string; idPrefix?: string; class?: string } = $props();
  let root: HTMLElement;
  const index = $derived(Math.max(0, tabs.findIndex((o) => o.value === value)));
  function keys(e: KeyboardEvent) {
    let next = index;
    if (e.key === "ArrowRight") next = (index + 1) % tabs.length;
    else if (e.key === "ArrowLeft") next = (index - 1 + tabs.length) % tabs.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = tabs.length - 1;
    else return;
    e.preventDefault();
    onchange(tabs[next].value);
    root.querySelectorAll<HTMLElement>('[role="tab"]')[next]?.focus();
  }
</script>

<!-- svelte-ignore a11y_interactive_supports_focus -->
<div bind:this={root} role="tablist" aria-label={label} class="tabs mg-no-scrollbar {cls}" onkeydown={keys}>
  {#each tabs as o (o.value)}
    {@const on = o.value === value}
    <button type="button" role="tab" id="{idPrefix}-{o.value}" aria-selected={on} aria-controls="{idPrefix}-panel" tabindex={on ? 0 : -1} class="tab" class:on onclick={() => onchange(o.value)}>
      {o.label}{#if o.count !== undefined}<span class="n mg-mono" class:zero={o.count === 0}>{o.count}</span>{/if}
    </button>
  {/each}
</div>

<style>
  .tabs { display: flex; gap: 2px; padding: 3px; overflow-x: auto; border: 1px solid var(--border); border-radius: 12px; background: var(--surface); }
  .tab {
    flex: 1 0 auto; display: inline-flex; align-items: center; justify-content: center; gap: 7px; height: 28px; padding: 0 12px; border: 0; border-radius: var(--radius-inner);
    background: transparent; color: var(--muted); font: inherit; font-size: 13px; font-weight: 700; white-space: nowrap; cursor: pointer;
    transition: color 0.2s, background-color 0.2s;
  }
  .tab:hover:not(.on) { color: var(--text); }
  .tab.on { background: var(--surface-2); color: var(--text); }
  .tab:focus-visible { outline: 2px solid var(--accent-line); outline-offset: 1px; }
  .n { font-size: 11px; font-weight: 700; color: var(--muted); font-variant-numeric: tabular-nums; }
  .tab.on .n { color: var(--accent-text); }
  .n.zero { opacity: 0.55; }
</style>
