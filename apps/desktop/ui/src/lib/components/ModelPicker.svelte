<script lang="ts">
  /**
   * "Current model" row for Settings: what is in use, how it will run here, and «Сменить», which opens a sheet with the catalog in
   * pick mode. Picking an installed model switches at once; picking a new one asks once, downloads (progress stays in the row)
   * and makes it primary when it finishes. Nothing is ever downloaded without that one confirmation.
   */
  import { onMount } from "svelte";
  import Button from "../kit/Button.svelte";
  import IconButton from "../kit/IconButton.svelte";
  import Icon from "../kit/Icon.svelte";
  import Sheet from "../kit/Sheet.svelte";
  import StatusPill from "../kit/StatusPill.svelte";
  import VendorMark from "../kit/VendorMark.svelte";
  import ModelCatalog from "./ModelCatalog.svelte";
  import { api } from "../api";
  import type { ModelEntry, ModelKind, Verdict } from "../api";
  import { t } from "../i18n/index.svelte";
  import { go } from "../router.svelte";
  import { bytes, mbps } from "../format";
  import { toast } from "../toast.svelte";
  import { activateModel, attachModels, modelAct, modelById, mstore, setPending } from "../modelsStore.svelte";
  import { displayName, quantOf, sharesName, sorted } from "../models";

  let { kind, title, currentId = "", sheetTitle = "" }: {
    kind: Extract<ModelKind, "asr" | "llm">;
    /** Row heading, e.g. «Модель расшифровки». */
    title: string;
    /** The id the settings point at; empty means "whatever is active". */
    currentId?: string;
    sheetTitle?: string;
  } = $props();

  onMount(() => attachModels());

  let open = $state(false);
  /** A not-yet-installed model the user clicked in the sheet, waiting for «Скачать». */
  let askId = $state<string | null>(null);

  const mine = $derived(mstore.models.filter((m) => m.kind === kind));
  const installed = $derived(sorted(mine.filter((m) => m.installed)));
  const current = $derived<ModelEntry | undefined>((currentId ? modelById(currentId) : undefined)?.installed ? modelById(currentId) : (installed.find((m) => m.active) ?? installed[0]));
  const auto = $derived(kind === "llm" && !currentId && !!current);
  const pendingId = $derived(mstore.pending[kind]?.id);
  const pending = $derived(pendingId ? modelById(pendingId) : undefined);
  const askModel = $derived(askId ? modelById(askId) : undefined);
  const recommended = $derived(mine.find((m) => m.recommended && (m.verdict === "great" || m.verdict === "ok")));

  const pill: Record<Verdict, { kind: "ok" | "blip" | "warn" | "bad"; label: string }> = {
    great: { kind: "ok", label: "Отлично" }, ok: { kind: "blip", label: "Нормально" }, slow: { kind: "warn", label: "Медленно" }, wontRun: { kind: "bad", label: "Не потянет" },
  };
  const pctOf = (m: ModelEntry) => (m.download ? Math.floor((m.download.doneBytes / Math.max(1, m.download.totalBytes)) * 100) : 0);

  function openSheet() { askId = null; open = true; }
  async function pickModel(id: string) {
    const m = modelById(id);
    if (!m) return;
    if (m.installed) {
      askId = null;
      if (current?.id !== id || auto) { try { await activateModel(id); toast(t("{name} теперь основная", { name: m.name })); } catch (e) { toast.error(e); } }
      open = false;
    } else askId = id;
  }
  async function startDownload() {
    const m = askModel;
    if (!m) return;
    setPending(kind, m.id);
    askId = null; open = false;
    await modelAct(() => api.modelDownload(m.id));
  }
  async function cancelPending() {
    if (!pending) return;
    const id = pending.id;
    setPending(kind, null);
    await modelAct(() => api.modelCancel(id));
  }
</script>

<div class="wrap">
  <div class="head">
    <h2 class="mg-label">{title}</h2>
    <button type="button" class="manage yc-press" onclick={() => go({ name: "models" })}>{t("Управлять моделями")} →</button>
  </div>

  <div class="cur" class:empty={!current}>
    {#if current}
      <VendorMark vendor={current.vendor} />
      <span class="who">
        <span class="nm yc-ellipsis">{displayName(current)}</span>
        <span class="sub yc-ellipsis">{bytes(current.sizeBytes)}{sharesName(current, mstore.models) ? ` · ${quantOf(current)}` : ""} · {t(current.estimate)}{#if auto} · {t("выбрана автоматически")}{/if}</span>
      </span>
      <StatusPill kind={pill[current.verdict].kind} label={t(pill[current.verdict].label)} sm />
    {:else}
      <span class="none-ic" aria-hidden="true"><Icon name="cpu" size={18} /></span>
      <span class="who">
        <span class="nm">{t("Модель не выбрана")}</span>
        <span class="sub">{kind === "asr" ? t("Без неё запись не расшифруется — выбери и скачай модель") : t("Скачай модель, и итоги станут собираться на этом компьютере")}</span>
      </span>
    {/if}
    <Button size="sm" variant={current ? "secondary" : "primary"} onclick={openSheet}>{current ? t("Сменить") : t("Выбрать")}</Button>
  </div>

  {#if pending && !pending.installed}
    {@const d = pending.download}
    {@const ds = d?.state.state}
    <div class="dl yc-in" role="status">
      <span class="dltop">
        <span class="dlt yc-ellipsis">{ds === "failed" ? t("Загрузка прервалась:") : ds === "paused" ? t("На паузе:") : t("Скачиваю:")} <b>{displayName(pending)}</b> — {t("станет основной")}</span>
        <span class="mg-mono pct">{pctOf(pending)}%</span>
        {#if ds === "paused"}<Button size="sm" onclick={() => modelAct(() => api.modelDownload(pending.id))}>{t("Продолжить")}</Button>
        {:else if ds === "failed"}<Button size="sm" onclick={() => modelAct(() => api.modelDownload(pending.id))}>{t("Повторить")}</Button>
        {:else if d}<IconButton variant="flat" aria-label={t("Пауза")} onclick={() => modelAct(() => api.modelPause(pending.id))}><svg class="yc-ic" width="13" height="13" viewBox="0 0 24 24" style="stroke-width: 3"><path d="M8 5v14M16 5v14" /></svg></IconButton>{/if}
        <IconButton variant="flat" aria-label={t("Отменить загрузку")} onclick={cancelPending}><Icon name="x" size={14} /></IconButton>
      </span>
      <span aria-hidden="true" class="pbar"><span style="width: {pctOf(pending)}%; background: {ds === 'failed' ? 'var(--warning)' : ds === 'paused' ? 'var(--faint)' : 'var(--accent)'}"></span></span>
      {#if d}<span class="dlsub mg-mono">{bytes(d.doneBytes)} {t("из")} {bytes(d.totalBytes)}{#if ds === "running" || ds === "queued"} · {mbps(d.bytesPerSec)}{/if}</span>{/if}
    </div>
  {/if}
</div>

<Sheet bind:open width={640} title={sheetTitle || title} onclose={() => (askId = null)}>
  <ModelCatalog kinds={[kind]} mode="pick" selectedId={askId ?? current?.id ?? null} preselectId={recommended?.id ?? null} onPick={pickModel} autofocusSearch />
  {#snippet footer()}
    {#if askModel}
      <div class="ask yc-in" role="alert">
        <span class="asktxt"><strong>{t("Скачать {size}?", { size: bytes(askModel.sizeBytes) })}</strong> {t("{name} докачается в фоне и станет основной — пока работает {cur}.", { name: askModel.name, cur: current?.name ?? "—" })}</span>
        <span class="acts">
          <Button size="sm" variant="primary" onclick={startDownload}>{t("Скачать")}</Button>
          <Button size="sm" variant="ghost" onclick={() => (askId = null)}>{t("Отмена")}</Button>
        </span>
      </div>
    {:else}
      <span class="hintf">{t("Выбери модель: установленная включится сразу, новая — только после твоего подтверждения.")}</span>
    {/if}
  {/snippet}
</Sheet>

<style>
  .wrap { display: flex; flex-direction: column; gap: 8px; }
  .head { display: flex; align-items: flex-end; justify-content: space-between; gap: 12px; padding: 14px 4px 0; }
  .head h2 { margin: 0; }
  .manage { flex: none; margin-bottom: 8px; padding: 0; border: 0; background: none; font: inherit; font-size: 12.5px; font-weight: 600; color: var(--accent-text); white-space: nowrap; cursor: pointer; }
  .manage:focus-visible { outline: 2px solid var(--accent-line); outline-offset: 3px; border-radius: 4px; }
  .cur { display: flex; align-items: center; gap: 12px; padding: 11px 12px; border: 1px solid var(--accent-line); border-radius: 14px; background: var(--surface); }
  .cur.empty { border-style: dashed; border-color: var(--border); }
  .none-ic { width: 34px; height: 34px; flex: none; border-radius: 10px; background: var(--surface-2); color: var(--muted); display: grid; place-items: center; }
  .who { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .nm { font-size: 13px; font-weight: 700; }
  .sub { font-size: 12px; color: var(--muted); }
  .dl { display: flex; flex-direction: column; gap: 6px; padding: 10px 12px; border: 1px solid var(--border); border-radius: 14px; background: var(--surface); }
  .dltop { display: flex; align-items: center; gap: 8px; font-size: 12.5px; }
  .dlt { flex: 1; min-width: 0; }
  .pct { font-size: 12.5px; font-weight: 700; color: var(--accent-text); }
  .pbar { position: relative; display: block; height: 5px; border-radius: 3px; overflow: hidden; background: var(--surface-2); }
  .pbar span { position: absolute; left: 0; top: 0; bottom: 0; border-radius: 3px; transition: width 0.2s; }
  .dlsub { font-size: 11px; color: var(--muted); }
  .ask { display: flex; flex-direction: column; gap: 10px; font-size: 12.5px; line-height: 1.45; }
  .acts { display: flex; gap: 8px; }
  .hintf { font-size: 12px; color: var(--muted); }
</style>
