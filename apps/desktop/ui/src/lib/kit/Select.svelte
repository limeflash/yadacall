<script lang="ts" generics="V extends string">
  type Opt = { value: V; label: string; hint?: string; /** Options with the same group are listed under one heading. */ group?: string };
  let { options, value, onchange, label = "", compact = false, class: cls = "" }:
    { options: Opt[]; value: V; onchange: (v: V) => void; label?: string; compact?: boolean; class?: string } = $props();
  let open = $state(false), active = $state(0);
  let root: HTMLElement, trigger: HTMLButtonElement;
  const uid = "sel" + Math.random().toString(36).slice(2, 8);
  const current = $derived(options.find((o) => o.value === value));

  $effect(() => {
    if (!open) return;
    active = Math.max(0, options.findIndex((o) => o.value === value));
    const away = (e: PointerEvent) => { if (root && !root.contains(e.target as Node)) open = false; };
    document.addEventListener("pointerdown", away);
    return () => document.removeEventListener("pointerdown", away);
  });
  function choose(v: V | undefined) { open = false; if (v !== undefined) onchange(v); trigger?.focus(); }
  function key(e: KeyboardEvent) {
    if (e.key === "Escape") { if (open) { e.preventDefault(); open = false; } return; }
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (!open) { open = true; return; }
      active = (active + (e.key === "ArrowDown" ? 1 : -1) + options.length) % options.length;
      return;
    }
    if ((e.key === "Enter" || e.key === " ") && open) { e.preventDefault(); choose(options[active]?.value); }
  }
</script>

<div bind:this={root} class="mg-select {cls}" onkeydown={key} role="presentation">
  <button bind:this={trigger} type="button" aria-label={label} aria-haspopup="listbox" aria-expanded={open} aria-controls={open ? uid : undefined}
    class="mg-select__trigger {compact ? 'mg-select__trigger--compact' : ''}" onclick={() => (open = !open)}>
    <span>{current?.label ?? ""}</span>
    <span class="mg-select__chev"><svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 4.5l3 3 3-3" /></svg></span>
  </button>
  {#if open}
    <div class="mg-select__popup">
      <ul id={uid} role="listbox" class="mg-select__list">
        {#each options as o, i (o.value)}
          {#if o.group && o.group !== options[i - 1]?.group}<li role="presentation" class="mg-select__group">{o.group}</li>{/if}
          <li role="option" aria-selected={o.value === value} data-highlighted={i === active ? "" : undefined} class="mg-select__item"
            onpointerenter={() => (active = i)} onclick={() => choose(o.value)} onkeydown={() => {}}>
            <span class="mg-select__item-text"><span>{o.label}</span>{#if o.hint}<span class="mg-select__hint">{o.hint}</span>{/if}</span>
            {#if o.value === value}<span class="mg-select__check"><svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 6.5l2.5 2.5 4.5-5.5" /></svg></span>{/if}
          </li>
        {/each}
      </ul>
    </div>
  {/if}
</div>
