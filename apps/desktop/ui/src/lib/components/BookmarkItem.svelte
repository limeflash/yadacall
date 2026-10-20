<script lang="ts">
  /**
   * One bookmark of a meeting. `row` is the list in the summary (with room for a screenshot thumbnail),
   * `pill` the compact marker inside the transcript. Click jumps to the moment; the pencil renames inline; × removes.
   */
  import { tick } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import { t } from "../i18n/index.svelte";
  import { clock } from "../format";
  import type { Bookmark } from "../api";

  let { bm, variant = "row", shot, editing = false, onjump, onedit, onrename, onremove }:
    { bm: Bookmark; variant?: "row" | "pill"; shot?: string; editing?: boolean; onjump: (ms: number) => void; onedit: () => void; onrename: (label: string | null) => void; onremove: () => void } = $props();

  let draft = $state("");
  let shotFailed = $state(false);
  let input: HTMLInputElement | undefined = $state();
  let wasEditing = false;
  $effect(() => {
    if (editing && !wasEditing) { draft = bm.label ?? ""; tick().then(() => { input?.focus(); input?.select(); input?.scrollIntoView({ block: "nearest" }); }); }
    wasEditing = editing;
  });
  function commit() { if (!editing) return; onrename(draft.trim() || null); }
  const name = $derived(bm.label || t("твоя закладка"));
  const showShot = $derived(!!shot && !shotFailed);
</script>

{#snippet field()}
  <input bind:this={input} class="inp" maxlength="120" aria-label={t("Название закладки")} placeholder={t("Название закладки")} bind:value={draft}
    onkeydown={(e) => { if (e.key === "Enter") { e.preventDefault(); commit(); } else if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onrename(bm.label ?? null); } }} onblur={commit} />
{/snippet}
{#snippet actions()}
  <span class="acts">
    <button type="button" class="act" aria-label={t("Переименовать закладку")} title={t("Переименовать")} onclick={onedit}><Icon name="pencil" size={13} /></button>
    <button type="button" class="act" aria-label={t("Удалить закладку")} title={t("Удалить")} onclick={onremove}><Icon name="x" size={13} /></button>
  </span>
{/snippet}

{#if variant === "pill"}
  <span class="pill" class:ed={editing} data-bm={bm.id}>
    <span class="pj">
      <Icon name="bookmark" size={12} />
      <button type="button" class="mg-mono pt" onclick={() => onjump(bm.atMs)} aria-label={t("Перейти к {time}", { time: clock(bm.atMs) })}>{clock(bm.atMs)}</button>
      {#if editing}{@render field()}{:else}<button type="button" class="pl yc-ellipsis" onclick={() => onjump(bm.atMs)} ondblclick={onedit}>{name}</button>{/if}
      {#if bm.createdAfter && !editing}<span class="later">{t("добавлена позже")}</span>{/if}
    </span>
    {#if !editing}{@render actions()}{/if}
  </span>
{:else}
  <div class="row" class:edit={editing} data-bm={bm.id}>
    <button type="button" class="mg-mono yc-tc rt" onclick={() => onjump(bm.atMs)} aria-label={t("Перейти к {time}", { time: clock(bm.atMs) })}>▸ {clock(bm.atMs)}</button>
    <div class="mid">
      {#if editing}{@render field()}
      {:else}
        <button type="button" class="yc-dash rl yc-ellipsis" class:ph={!bm.label} onclick={() => onjump(bm.atMs)} ondblclick={onedit}>{bm.label || t("Без названия")}</button>
        {#if bm.createdAfter}<span class="later">{t("добавлена позже")}</span>{/if}
      {/if}
    </div>
    {#if showShot && !editing}
      <button type="button" class="thumb" tabindex="-1" aria-hidden="true" onclick={() => onjump(bm.atMs)}><img src={shot} alt="" loading="lazy" onerror={() => (shotFailed = true)} /></button>
    {/if}
    {#if !editing}{@render actions()}{/if}
  </div>
{/if}

<style>
  .acts { flex: none; display: inline-flex; gap: 1px; opacity: 0; transition: opacity 0.15s; }
  .row:hover .acts, .row:focus-within .acts, .pill:hover .acts, .pill:focus-within .acts { opacity: 1; }
  .act { display: grid; place-items: center; width: 22px; height: 22px; padding: 0; border: 0; border-radius: 7px; background: transparent; color: var(--muted); cursor: pointer; }
  .act:hover { background: var(--surface-2); color: var(--text); }
  .inp { flex: 1; min-width: 0; height: 28px; box-sizing: border-box; padding: 0 9px; border: 1px solid var(--accent-line); border-radius: 9px; background: var(--bg); color: var(--text); font: inherit; font-size: 13px; }
  .later { flex: none; font-size: 11px; color: var(--muted); opacity: 0.75; white-space: nowrap; }

  .row { display: flex; align-items: center; gap: 10px; min-height: 44px; padding: 7px 8px 7px 10px; border-radius: 12px; }
  .row:hover, .row:focus-within { background: var(--surface-2); }
  .thumb { flex: none; width: 72px; height: 45px; padding: 0; border: 1px solid var(--border); border-radius: 8px; background: var(--surface-2); overflow: hidden; cursor: pointer; }
  .thumb img { display: block; width: 100%; height: 100%; object-fit: cover; }
  .rt { flex: none; padding: 2px 7px; border: 1px solid var(--border); border-radius: 6px; background: var(--surface-2); color: var(--accent-text); font-size: 11px; font-weight: 700; }
  .row:hover .rt { background: var(--surface); }
  .mid { flex: 1; min-width: 0; display: flex; align-items: baseline; gap: 10px; }
  .rl { min-width: 0; font-size: 14px; line-height: 1.4; }
  .rl.ph { color: var(--muted); }

  .pill { display: inline-flex; align-items: center; gap: 2px; max-width: 100%; height: 26px; padding: 0 4px 0 10px; border-radius: 13px; background: var(--tone-sand-bg); color: var(--tone-sand); font-size: 12px; }
  .pill.ed .pj { min-width: auto; }
  .pj { min-width: 0; display: inline-flex; align-items: center; gap: 6px; }
  .pt, .pl { padding: 0; border: 0; background: none; color: inherit; font-size: 12px; cursor: pointer; }
  .pt { font-weight: 700; }
  .pl { min-width: 0; max-width: 260px; }
  .pill .act { color: inherit; opacity: 0.8; }
  .pill .act:hover { background: color-mix(in oklch, var(--tone-sand) 18%, transparent); color: inherit; }
  .pill .inp { height: 22px; font-size: 12px; width: 220px; flex: none; }
  .pill .later { color: inherit; opacity: 0.65; }
</style>
