<script lang="ts">
  /** "Освободить место": installed-but-unused models and stalled downloads with checkboxes, removed in one go. */
  import Button from "../kit/Button.svelte";
  import Icon from "../kit/Icon.svelte";
  import { t } from "../i18n/index.svelte";
  import { bytes } from "../format";
  import { toast } from "../toast.svelte";
  import { diskBytes, isRequired, mstore, removeModel } from "../modelsStore.svelte";
  import type { ModelEntry } from "../api";

  let open = $state(false);
  let picked = $state<Set<string>>(new Set());
  let asking = $state(false);
  let busy = $state(false);

  /** Not primary, not needed for recording; plus downloads that are stuck (paused / failed) holding partial bytes. */
  const stalled = (m: ModelEntry) => !m.installed && !!m.download && (m.download.state.state === "paused" || m.download.state.state === "failed") && m.download.doneBytes > 0;
  const candidates = $derived(mstore.models.filter((m) => !isRequired(m) && ((m.installed && !m.active) || stalled(m))).sort((a, b) => diskBytes(b) - diskBytes(a)));
  const chosen = $derived(candidates.filter((m) => picked.has(m.id)));
  const total = $derived(chosen.reduce((s, m) => s + diskBytes(m), 0));
  const reclaimable = $derived(candidates.reduce((s, m) => s + diskBytes(m), 0));

  function toggle(id: string) { const n = new Set(picked); if (n.has(id)) n.delete(id); else n.add(id); picked = n; asking = false; }
  function toggleAll() { picked = chosen.length === candidates.length ? new Set() : new Set(candidates.map((m) => m.id)); asking = false; }
  function show() { open = !open; asking = false; if (open) picked = new Set(); }
  async function run() {
    busy = true;
    let freed = 0, n = 0;
    for (const m of chosen) { const b = diskBytes(m); if (await removeModel(m)) { freed += b; n++; } }
    busy = false; asking = false; picked = new Set();
    if (n) toast(t("Удалено: {n} · освободилось {size}", { n, size: bytes(freed) }));
    if (!candidates.length) open = false;
  }
</script>

<div class="wrap">
  <button type="button" class="open yc-press" aria-expanded={open} aria-controls="cleanup-panel" onclick={show}>
    <Icon name="trash" size={13} />{t("Освободить место")}{#if reclaimable > 0}<span class="mg-mono hint">{bytes(reclaimable)}</span>{/if}
  </button>

  {#if open}
    <div id="cleanup-panel" class="panel yc-in">
      {#if candidates.length === 0}
        <p class="none">{t("Лишнего нет: всё, что скачано, сейчас используется.")}</p>
      {:else}
        <p class="lead">{t("Здесь модели, которыми ты не пользуешься, и недокачанные загрузки. Основные модели и всё нужное для записи не показываются.")}</p>
        <label class="all"><input type="checkbox" checked={chosen.length === candidates.length} indeterminate={chosen.length > 0 && chosen.length < candidates.length} onchange={toggleAll} />{t("Выбрать всё")}</label>
        <ul>
          {#each candidates as m (m.id)}
            <li>
              <label>
                <input type="checkbox" checked={picked.has(m.id)} onchange={() => toggle(m.id)} />
                <span class="nm mg-mono yc-ellipsis">{m.name}</span>
                <span class="why">{stalled(m) ? t("недокачана") : t("не используется")}</span>
                <span class="sz mg-mono">{bytes(diskBytes(m))}</span>
              </label>
            </li>
          {/each}
        </ul>
        {#if asking && chosen.length}
          <div class="ask" role="alert">
            <p><strong>{t("Удалить выбранное: {n}?", { n: chosen.length })}</strong> {t("Освободится {size}. Вернуть можно только скачав заново — отменить удаление нельзя.", { size: bytes(total) })}</p>
            <span class="acts">
              <Button size="sm" variant="danger" disabled={busy} onclick={run}>{busy ? t("Удаляю…") : t("Удалить")}</Button>
              <Button size="sm" variant="ghost" disabled={busy} onclick={() => (asking = false)}>{t("Отмена")}</Button>
            </span>
          </div>
        {:else}
          <div class="foot">
            <span class="mg-mono total">{chosen.length ? t("Освободится {size}", { size: bytes(total) }) : ""}</span>
            <Button size="sm" variant="danger" disabled={!chosen.length} onclick={() => (asking = true)}>{t("Удалить выбранные")}{chosen.length ? ` (${chosen.length})` : ""}</Button>
          </div>
        {/if}
      {/if}
    </div>
  {/if}
</div>

<style>
  .wrap { display: flex; flex-direction: column; gap: 8px; align-items: flex-start; }
  .open { display: inline-flex; align-items: center; gap: 7px; height: 30px; padding: 0 12px; border: 1px solid var(--border); border-radius: 15px; background: var(--surface); color: var(--text); font: inherit; font-size: 12px; font-weight: 700; cursor: pointer; }
  .open:hover { border-color: var(--accent-line); }
  .open:focus-visible { outline: 2px solid var(--accent-line); outline-offset: 2px; }
  .hint { font-size: 11px; color: var(--muted); }
  .panel { align-self: stretch; display: flex; flex-direction: column; gap: 10px; padding: 12px 14px; border: 1px solid var(--border); border-radius: 14px; background: var(--surface); }
  .lead, .none { margin: 0; font-size: 12.5px; color: var(--muted); line-height: 1.45; }
  ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; }
  li label, .all { display: flex; align-items: center; gap: 10px; min-height: 34px; font-size: 13px; cursor: pointer; }
  li + li { border-top: 1px solid var(--border); }
  .all { min-height: 24px; font-size: 12px; font-weight: 700; color: var(--muted); }
  input[type="checkbox"] { width: 16px; height: 16px; margin: 0; accent-color: var(--accent); flex: none; }
  .nm { flex: 1; min-width: 0; font-weight: 700; font-size: 12.5px; }
  .why { font-size: 11px; color: var(--muted); white-space: nowrap; }
  .sz { font-size: 12px; min-width: 62px; text-align: right; }
  .foot { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
  .total { font-size: 12px; color: var(--accent-text); }
  .ask { display: flex; flex-direction: column; gap: 9px; padding: 10px 12px; border: 1px solid var(--warning-line); border-radius: 11px; background: color-mix(in oklch, var(--warning) 9%, var(--surface)); font-size: 12.5px; line-height: 1.45; }
  .ask p { margin: 0; }
  .acts { display: flex; gap: 8px; }
</style>
