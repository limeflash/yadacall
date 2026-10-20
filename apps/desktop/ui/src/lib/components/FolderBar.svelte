<script lang="ts">
  /**
   * The folder filter row: Все · Избранное · each folder · Без папки, plus inline folder management
   * (create, rename, recolour, delete) and drop targets for dragged meeting cards.
   */
  import { tick } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import Button from "../kit/Button.svelte";
  import { t } from "../i18n/index.svelte";
  import { folders, folderColor, colorName, FOLDER_COLORS, createFolder, patchFolder, removeFolder, nextColor } from "../folders.svelte";
  import type { FolderFilter } from "../folders.svelte";
  import { drag } from "../dragMeeting.svelte";

  let { value, onchange, manage = true, label = "Папки" }: { value: FolderFilter; onchange: (v: FolderFilter) => void; manage?: boolean; label?: string } = $props();

  let adding = $state(false);
  let addDraft = $state("");
  let editing = $state<string | null>(null);
  let nameDraft = $state("");
  let confirmDelete = $state(false);
  let group: HTMLElement | undefined = $state();

  const SUGGEST = ["Работа", "Личная", "Проекты"];
  const editFolder = $derived(folders.list.find((f) => f.id === editing));

  async function startAdd() { adding = true; addDraft = ""; await tick(); document.getElementById("folder-add")?.focus(); }
  async function commitAdd() {
    if (!adding) return;
    adding = false;
    const f = await createFolder(addDraft);
    addDraft = "";
    if (f) onchange(f.id);
  }
  async function suggest(name: string) {
    const f = await createFolder(name, SUGGEST.indexOf(name) >= 0 ? (["mint", "rose", "lavender"][SUGGEST.indexOf(name)] ?? nextColor()) : nextColor());
    if (f) onchange(f.id);
  }

  async function openEdit(id: string) {
    const f = folders.list.find((x) => x.id === id); if (!f) return;
    editing = id; nameDraft = f.name; confirmDelete = false;
    await tick();
    const el = document.getElementById("folder-name") as HTMLInputElement | null;
    el?.focus(); el?.select();
  }
  async function closeEdit(refocus = true) {
    const id = editing;
    if (!id) return;
    const n = nameDraft.trim();
    if (n && editFolder && n !== editFolder.name) await patchFolder(id, { name: n });
    editing = null; confirmDelete = false;
    if (refocus) { await tick(); document.getElementById("folder-more-" + id)?.focus(); }
  }
  async function doDelete() {
    const id = editing; if (!id) return;
    editing = null; confirmDelete = false;
    await removeFolder(id);
  }

  function groupKeys(e: KeyboardEvent) {
    const d: Record<string, number> = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 };
    if (!(e.key in d) || !group) return;
    const radios = [...group.querySelectorAll<HTMLElement>('[role="radio"]')];
    const i = radios.indexOf(document.activeElement as HTMLElement);
    if (i < 0) return;
    e.preventDefault();
    const next = radios[(i + d[e.key] + radios.length) % radios.length];
    next.focus(); next.click();
  }
  function onWindowKey(e: KeyboardEvent) { if (e.key === "Escape" && editing) { e.preventDefault(); closeEdit(); } }
  function onWindowClick(e: MouseEvent) { if (editing && !e.composedPath().some((n) => n instanceof HTMLElement && n.matches(".fpop, .fmore"))) closeEdit(false); }
</script>

<svelte:window onkeydown={onWindowKey} onclick={onWindowClick} />

<div class="bar" class:dropping={drag.active}>
  <div bind:this={group} role="radiogroup" aria-label={t(label)} class="chips" tabindex="-1" onkeydown={groupKeys}>
    {@render chip("all", t("Все"))}
    {@render chip("fav", t("Избранное"), "star")}
    {#each folders.list as f (f.id)}
      <span class="slot">
        {@render chip(f.id, f.name, undefined, folderColor(f))}
        {#if manage && value === f.id}
          <button type="button" id={"folder-more-" + f.id} class="fmore" aria-haspopup="dialog" aria-expanded={editing === f.id} aria-label={t("Настроить папку «{name}»", { name: f.name })} title={t("Переименовать, цвет, удалить")} onclick={() => (editing === f.id ? closeEdit() : openEdit(f.id))}><Icon name="dots" size={14} style="stroke-width: 3.4" /></button>
        {/if}
        {#if editing === f.id}
          <div class="yc-pop fpop" role="dialog" aria-label={t("Папка «{name}»", { name: f.name })}>
            <label class="fl"><span class="mg-sr">{t("Название папки")}</span>
              <input id="folder-name" class="fin" maxlength="40" bind:value={nameDraft} onkeydown={(e) => { if (e.key === "Enter") { e.preventDefault(); closeEdit(); } }} /></label>
            <div role="radiogroup" aria-label={t("Цвет папки")} class="sw">
              {#each FOLDER_COLORS as c (c)}
                <button type="button" role="radio" aria-checked={f.color === c} aria-label={colorName(c)} title={colorName(c)} class="dot" style="--c: var(--accent-{c})" onclick={() => patchFolder(f.id, { color: c })}></button>
              {/each}
            </div>
            <span aria-hidden="true" class="sep"></span>
            {#if !confirmDelete}
              <button type="button" class="del" onclick={() => (confirmDelete = true)}><Icon name="trash" size={14} />{t("Удалить папку")}</button>
            {:else}
              <div class="ask" role="alert">
                <span>{t("Встречи останутся — просто без папки.")}</span>
                <span class="askbtns"><Button size="xs" variant="ghost" onclick={() => (confirmDelete = false)}>{t("Отмена")}</Button><Button size="xs" variant="danger" onclick={doDelete}>{t("Удалить папку")}</Button></span>
              </div>
            {/if}
          </div>
        {/if}
      </span>
    {/each}
    {#if folders.list.length > 0}{@render chip("none", t("Без папки"))}{/if}
  </div>

  {#if manage}
    {#if adding}
      <label class="addf"><span class="mg-sr">{t("Название новой папки")}</span>
        <input id="folder-add" class="addin" maxlength="40" placeholder={t("Название папки")} bind:value={addDraft}
          onkeydown={(e) => { if (e.key === "Enter") { e.preventDefault(); commitAdd(); } else if (e.key === "Escape") { e.stopPropagation(); adding = false; } }}
          onblur={commitAdd} /></label>
    {:else}
      <button type="button" class="yc-press addb" onclick={startAdd}><Icon name="plus" size={13} />{t("Папка")}</button>
      {#if folders.loaded && folders.list.length === 0}
        <span class="hint">{t("или сразу:")}</span>
        {#each SUGGEST as s (s)}<button type="button" class="yc-press sugg" onclick={() => suggest(s)}>{t(s)}</button>{/each}
      {/if}
    {/if}
  {/if}
</div>

{#snippet chip(v: string, text: string, icon?: string, color?: string)}
  {@const on = value === v}
  {@const droppable = v !== "all"}
  <button type="button" role="radio" aria-checked={on} tabindex={on ? 0 : -1} class="mg-fchip fc" class:over={drag.active && drag.over === v} data-drop={droppable ? v : undefined}
    oncontextmenu={(e) => { if (manage && v !== "all" && v !== "fav" && v !== "none") { e.preventDefault(); onchange(v); openEdit(v); } }}
    ondblclick={() => { if (manage && v !== "all" && v !== "fav" && v !== "none") openEdit(v); }}
    onclick={() => onchange(v)}>
    {#if icon}<Icon name={icon} size={13} style={on ? "fill: currentColor" : undefined} />{/if}
    {#if color}<span aria-hidden="true" class="cd" style="background: {color}"></span>{/if}
    <span class="yc-ellipsis ct">{text}</span>
  </button>
{/snippet}

<style>
  .bar { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; margin: 0 0 10px; min-width: 0; }
  .chips { display: contents; }
  .slot { position: relative; display: inline-flex; align-items: center; gap: 2px; max-width: 100%; }
  .fc { height: 28px; padding: 0 11px; max-width: 200px; cursor: pointer; transition: color 0.2s, background-color 0.2s, border-color 0.2s, transform 0.2s var(--ease-spring); }
  .fc:hover { border-color: var(--accent-line); color: var(--text); }
  .ct { min-width: 0; }
  .cd { flex: none; width: 8px; height: 8px; border-radius: 50%; }
  .dropping .fc[data-drop] { border-style: dashed; border-color: var(--faint); }
  .dropping .fc.over { border-style: solid; border-color: var(--accent); background: var(--accent-soft); color: var(--text); transform: scale(1.05); }
  .fmore { flex: none; display: grid; place-items: center; width: 24px; height: 24px; padding: 0; border: 0; border-radius: 8px; background: transparent; color: var(--muted); cursor: pointer; }
  .fmore:hover, .fmore[aria-expanded="true"] { background: var(--surface-2); color: var(--text); }
  .addb { display: inline-flex; align-items: center; gap: 5px; height: 28px; padding: 0 10px; border: 1px dashed var(--faint); border-radius: 14px; background: transparent; color: var(--muted); font-size: 12px; font-weight: 600; cursor: pointer; }
  .addb:hover { color: var(--text); border-color: var(--accent-line); }
  .hint { font-size: 12px; color: var(--muted); margin-left: 2px; }
  .sugg { height: 28px; padding: 0 11px; border: 1px dashed var(--faint); border-radius: 14px; background: transparent; color: var(--muted); font-size: 12px; font-weight: 700; cursor: pointer; }
  .sugg:hover { color: var(--text); border-color: var(--accent-line); background: var(--surface); }
  .addf { display: inline-flex; }
  .addin { width: 150px; height: 28px; box-sizing: border-box; padding: 0 11px; border: 1px solid var(--accent-line); border-radius: 14px; background: var(--surface); color: var(--text); font: inherit; font-size: 12px; font-weight: 600; }
  .fpop { position: absolute; z-index: 20; top: 34px; left: 0; width: 244px; box-sizing: border-box; padding: 10px; display: flex; flex-direction: column; gap: 10px; border: 1px solid var(--border); border-radius: 16px; background: var(--surface); box-shadow: var(--shadow-pop); transform-origin: top left; }
  .fl { display: flex; }
  .fin { flex: 1; min-width: 0; height: 34px; box-sizing: border-box; padding: 0 11px; border: 1px solid var(--border); border-radius: 11px; background: var(--bg); color: var(--text); font: inherit; font-size: 13px; font-weight: 600; }
  .sw { display: flex; gap: 8px; padding: 0 2px; }
  .dot { width: 22px; height: 22px; padding: 0; border: 0; border-radius: 50%; background: var(--c); cursor: pointer; transition: transform 0.2s var(--ease-spring); }
  .dot:hover { transform: scale(1.12); }
  .dot[aria-checked="true"] { box-shadow: 0 0 0 2px var(--surface), 0 0 0 4px var(--c); }
  .sep { height: 1px; background: var(--border); }
  .del { display: flex; align-items: center; gap: 8px; height: 30px; padding: 0 8px; border: 0; border-radius: 9px; background: transparent; color: var(--danger-text); font-size: 12.5px; font-weight: 600; cursor: pointer; }
  .del:hover { background: var(--danger-soft); }
  .ask { display: flex; flex-direction: column; gap: 8px; font-size: 12.5px; line-height: 1.4; color: var(--text); }
  .askbtns { display: flex; justify-content: flex-end; gap: 6px; }
</style>
