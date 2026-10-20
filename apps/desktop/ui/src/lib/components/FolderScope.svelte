<script lang="ts">
  /** Small chip in the ask bar: limits the question to one folder. Hidden while there are no folders. */
  import { tick } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import { t } from "../i18n/index.svelte";
  import { folders, folderColor, folderById } from "../folders.svelte";

  let { value, onchange }: { value?: string; onchange: (id: string | undefined) => void } = $props();
  let open = $state(false);
  let root: HTMLElement | undefined = $state();
  const cur = $derived(folderById(value));

  async function toggle() {
    open = !open;
    if (open) { await tick(); root?.querySelector<HTMLElement>('[aria-checked="true"]')?.focus(); }
  }
  function pick(id?: string) { open = false; onchange(id); root?.querySelector<HTMLElement>(".chip")?.focus(); }
  function keys(e: KeyboardEvent) {
    if (!open) return;
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); open = false; root?.querySelector<HTMLElement>(".chip")?.focus(); return; }
    const d = e.key === "ArrowDown" ? 1 : e.key === "ArrowUp" ? -1 : 0;
    if (!d) return;
    e.preventDefault();
    const list = [...root!.querySelectorAll<HTMLElement>('[role="menuitemradio"]')];
    const i = list.indexOf(document.activeElement as HTMLElement);
    list[(i + d + list.length) % list.length]?.focus();
  }
</script>

<svelte:window onpointerdown={(e) => { if (open && !(e.target as HTMLElement).closest(".scope")) open = false; }} />

{#if folders.list.length > 0 || value}
  <span class="scope" bind:this={root} onkeydown={keys} role="presentation">
    <button type="button" class="yc-press chip" class:on={!!cur} aria-haspopup="menu" aria-expanded={open} title={t("Искать только в одной папке")} onclick={toggle}>
      {#if cur}<span aria-hidden="true" class="cd" style="background: {folderColor(cur)}"></span>{:else}<Icon name="folder" size={13} />{/if}
      <span class="yc-ellipsis lbl">{cur ? cur.name : t("Все встречи")}</span>
    </button>
    {#if open}
      <div class="yc-pop pop" role="menu" aria-label={t("Где искать")}>
        <button type="button" role="menuitemradio" aria-checked={!value} class="yc-mi mi" onclick={() => pick(undefined)}><Icon name="folder" size={14} /><span class="grow">{t("Все встречи")}</span>{#if !value}<Icon name="check" size={13} />{/if}</button>
        {#each folders.list as f (f.id)}
          <button type="button" role="menuitemradio" aria-checked={value === f.id} class="yc-mi mi" onclick={() => pick(f.id)}><span aria-hidden="true" class="cd" style="background: {folderColor(f)}"></span><span class="grow yc-ellipsis">{f.name}</span>{#if value === f.id}<Icon name="check" size={13} />{/if}</button>
        {/each}
      </div>
    {/if}
  </span>
{/if}

<style>
  .scope { position: relative; flex: none; display: inline-flex; margin-left: -8px; }
  .chip { display: inline-flex; align-items: center; gap: 6px; max-width: 150px; height: 28px; padding: 0 10px; border: 1px solid var(--border); border-radius: 14px; background: var(--surface-2); color: var(--muted); font-size: 12px; font-weight: 600; cursor: pointer; }
  .chip:hover { color: var(--text); }
  .chip.on { border-color: var(--accent-line); background: var(--accent-soft); color: var(--text); }
  .lbl { min-width: 0; }
  .cd { flex: none; width: 8px; height: 8px; border-radius: 50%; }
  .pop { position: absolute; z-index: 30; bottom: 36px; left: 0; width: 210px; box-sizing: border-box; padding: 5px; display: flex; flex-direction: column; border: 1px solid var(--border); border-radius: 14px; background: var(--surface); box-shadow: var(--shadow-pop); transform-origin: bottom left; }
  .mi { display: flex; align-items: center; gap: 9px; height: 32px; padding: 0 10px; border: 0; border-radius: 9px; background: transparent; color: var(--text); font-size: 13px; text-align: left; cursor: pointer; }
  .mi:focus-visible { background: var(--surface-2); }
  .grow { flex: 1; min-width: 0; }
</style>
