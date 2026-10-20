<script lang="ts">
  /** Floating menu for one meeting: star it and move it to a folder. Opens at a point (button or right-click). */
  import { onMount, tick } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import { t } from "../i18n/index.svelte";
  import { folders, folderColor, createFolder } from "../folders.svelte";

  let { x, y, above, favorite, folderId, onfavorite, onmove, onclose }:
    { x: number; y: number; /** Top edge of the trigger: the menu opens above it when there is no room below. */ above?: number; favorite: boolean; folderId?: string; onfavorite: (v: boolean) => void; onmove: (folderId: string | null) => void; onclose: (escaped?: boolean) => void } = $props();

  const W = 236;
  let el: HTMLElement | undefined = $state();
  let pos = $state({ left: 0, top: 0 });
  let adding = $state(false);
  let draft = $state("");

  /** Moves the node to <body>: ancestors with a transform would otherwise capture `position: fixed`. */
  function portal(node: HTMLElement) { document.body.appendChild(node); return { destroy() { node.remove(); } }; }

  function place() {
    const h = el?.offsetHeight ?? 260;
    pos = {
      left: Math.max(8, Math.min(x, window.innerWidth - W - 8)),
      top: y + h + 8 <= window.innerHeight ? y : above !== undefined && above - h - 4 >= 8 ? above - h - 4 : Math.max(8, window.innerHeight - h - 8),
    };
  }
  $effect(() => { x; y; folders.list.length; adding; tick().then(place); });
  onMount(() => { place(); items()[0]?.focus(); });

  const items = () => [...(el?.querySelectorAll<HTMLElement>('[role^="menuitem"]') ?? [])];
  function keys(e: KeyboardEvent) {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onclose(true); return; }
    if (adding) return;
    const d = e.key === "ArrowDown" ? 1 : e.key === "ArrowUp" ? -1 : 0;
    if (e.key === "Tab") { e.preventDefault(); onclose(); return; }
    if (!d && e.key !== "Home" && e.key !== "End") return;
    e.preventDefault();
    const list = items(); const i = list.indexOf(document.activeElement as HTMLElement);
    list[e.key === "Home" ? 0 : e.key === "End" ? list.length - 1 : (i + d + list.length) % list.length]?.focus();
  }
  function outside(e: PointerEvent) {
    const n = e.target as HTMLElement;
    if (!n.closest(".mmenu") && !n.closest("[data-menu-trigger]")) onclose();
  }
  async function startAdd() { adding = true; draft = ""; await tick(); document.getElementById("mm-new")?.focus(); }
  async function commitAdd() {
    const name = draft.trim();
    if (!name) { adding = false; return; }
    adding = false;
    const f = await createFolder(name);
    onclose();
    if (f) onmove(f.id);
  }
</script>

<svelte:window onpointerdown={outside} onblur={() => onclose()} />

<div use:portal bind:this={el} class="yc-pop mmenu" role="menu" tabindex="-1" aria-label={t("Действия со встречей")} style="left: {pos.left}px; top: {pos.top}px; width: {W}px" onkeydown={keys}>
  <button type="button" role="menuitem" class="yc-mi mi" onclick={() => { onfavorite(!favorite); onclose(); }}>
    <span class="ic" class:on={favorite}><Icon name="star" size={16} style={favorite ? "fill: currentColor" : undefined} /></span>
    <span class="grow">{favorite ? t("Убрать из избранного") : t("В избранное")}</span>
  </button>
  <span aria-hidden="true" class="sep"></span>
  <div class="head" id="mm-folders">{t("Папка")}</div>
  <div role="group" aria-labelledby="mm-folders" class="grp">
    <button type="button" role="menuitemradio" aria-checked={!folderId} class="yc-mi mi" onclick={() => { onmove(null); onclose(); }}>
      <span class="ic"><Icon name="folder" size={16} /></span><span class="grow">{t("Без папки")}</span>{#if !folderId}<Icon name="check" size={14} />{/if}
    </button>
    {#each folders.list as f (f.id)}
      <button type="button" role="menuitemradio" aria-checked={folderId === f.id} class="yc-mi mi" onclick={() => { onmove(f.id); onclose(); }}>
        <span class="ic"><span aria-hidden="true" class="cd" style="background: {folderColor(f)}"></span></span><span class="grow yc-ellipsis">{f.name}</span>{#if folderId === f.id}<Icon name="check" size={14} />{/if}
      </button>
    {/each}
  </div>
  {#if adding}
    <label class="newrow"><span class="mg-sr">{t("Название новой папки")}</span>
      <input id="mm-new" maxlength="40" placeholder={t("Название папки")} bind:value={draft} onkeydown={(e) => { if (e.key === "Enter") { e.preventDefault(); commitAdd(); } else if (e.key === "Escape") { e.stopPropagation(); adding = false; items().at(-1)?.focus(); } }} onblur={() => { if (!draft.trim()) adding = false; }} /></label>
  {:else}
    <button type="button" role="menuitem" class="yc-mi mi" onclick={startAdd}><span class="ic"><Icon name="plus" size={16} /></span><span class="grow">{t("Новая папка…")}</span></button>
  {/if}
</div>

<style>
  .mmenu { position: fixed; z-index: 60; box-sizing: border-box; padding: 6px; border: 1px solid var(--border); border-radius: 16px; background: var(--surface); box-shadow: var(--shadow-pop); display: flex; flex-direction: column; max-height: calc(100vh - 16px); overflow-y: auto; outline: none; transform-origin: top left; }
  .mi { display: flex; align-items: center; gap: 10px; height: 34px; flex: none; padding: 0 10px; border: 0; border-radius: 10px; background: transparent; color: var(--text); font-size: 13px; text-align: left; cursor: pointer; }
  .mi:focus-visible { background: var(--surface-2); }
  .ic { display: inline-flex; width: 16px; justify-content: center; color: var(--muted); }
  .ic.on { color: var(--accent-text); }
  .grow { flex: 1; min-width: 0; }
  .cd { width: 9px; height: 9px; border-radius: 50%; }
  .sep { flex: none; height: 1px; margin: 5px 4px; background: var(--border); }
  .head { padding: 4px 10px 2px; font-size: 11px; font-weight: 700; letter-spacing: 0.08em; text-transform: uppercase; color: var(--muted); }
  .grp { display: flex; flex-direction: column; max-height: 190px; overflow-y: auto; }
  .newrow { display: flex; padding: 3px 4px 2px; }
  .newrow input { flex: 1; min-width: 0; height: 32px; box-sizing: border-box; padding: 0 10px; border: 1px solid var(--accent-line); border-radius: 10px; background: var(--bg); color: var(--text); font: inherit; font-size: 13px; }
</style>
