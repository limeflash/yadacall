<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import Button from "../kit/Button.svelte";
  import IconButton from "../kit/IconButton.svelte";
  import StatusPill from "../kit/StatusPill.svelte";
  import Toast from "../kit/Toast.svelte";
  import SectionLabel from "../kit/SectionLabel.svelte";
  import { api } from "../api";
  import type { HardwareProfile, ModelEntry, ModelKind, Verdict } from "../api";
  import { t } from "../i18n/index.svelte";
  import { back, go } from "../router.svelte";
  import { bytes, mbps } from "../format";
  import { onEngineEvent, app } from "../state.svelte";
  import { toast } from "../toast.svelte";

  let hardware = $state<HardwareProfile | null>(null);
  let models = $state<ModelEntry[]>([]);
  let toastModel = $state<string | null>(null);
  let toastTimer: ReturnType<typeof setTimeout>;
  let benching = $state<string | null>(null);
  let measured = $state<Record<string, number>>({});

  async function load() { const o = await api.modelsOverview(); hardware = o.hardware; models = o.models; }
  onMount(() => {
    load();
    return onEngineEvent((e) => {
      if (e.type !== "download") return;
      const m = models.find((x) => x.id === e.item);
      if (!m) return;
      if (e.state.state === "done") { load(); toastModel = m.name; clearTimeout(toastTimer); toastTimer = setTimeout(() => (toastModel = null), 5000); return; }
      const prev = m.download;
      m.download = { state: e.state, doneBytes: e.doneBytes, totalBytes: e.totalBytes, bytesPerSec: e.bytesPerSec, etaSecs: e.bytesPerSec ? Math.ceil((e.totalBytes - e.doneBytes) / e.bytesPerSec) : prev?.etaSecs };
    });
  });

  const kinds: { kind: ModelKind; title: string; sub: string }[] = [
    { kind: "asr", title: "Расшифровка", sub: "Речь → текст на компьютере" },
    { kind: "voice", title: "Голоса", sub: "Разметка по голосам и узнавание знакомых" },
    { kind: "embed", title: "Поиск", sub: "Смысловой поиск по встречам" },
    { kind: "llm", title: "Итоги (LLM)", sub: "Итоги, задачи и ответы на вопросы без облака" },
  ];
  const verdictPill: Record<Verdict, { kind: "ok" | "blip" | "warn" | "bad"; label: string }> = {
    great: { kind: "ok", label: "Отлично" }, ok: { kind: "blip", label: "Нормально" }, slow: { kind: "warn", label: "Медленно" }, wontRun: { kind: "bad", label: "Не потянет" },
  };
  const used = $derived(models.filter((m) => m.installed).reduce((s, m) => s + m.sizeBytes, 0) + models.reduce((s, m) => s + (m.download?.doneBytes ?? 0), 0));
  const usedW = $derived(Math.min(100, (used / Math.max(1, hardware?.diskFreeBytes ?? 1)) * 100 * 8));

  const pctOf = (m: ModelEntry) => (m.download ? Math.floor((m.download.doneBytes / Math.max(1, m.download.totalBytes)) * 100) : 0);
  function etaLabel(s?: number) { if (!s) return ""; return s > 60 ? "~" + Math.ceil(s / 60) + " " + t("мин") : "~" + Math.max(1, s) + " " + t("с"); }

  async function act(fn: () => Promise<unknown>) { try { await fn(); await load(); } catch (e) { toast.error(e); } }
  async function bench(m: ModelEntry) {
    benching = m.id;
    try { const r = await api.modelBenchmark(m.id); measured[m.id] = r.realtimeFactor; toast(t("{name}: измерено ≈ {x}× быстрее реального времени", { name: m.name, x: r.realtimeFactor })); } catch (e) { toast.error(e); }
    benching = null;
  }
  const hwLine = $derived(hardware ? `${hardware.label} · ${hardware.chip} · ${hardware.ramGb} ${t("ГБ")}` : "");
</script>

<main>
  <div class="yc-scroll" style="flex: 1; min-height: 0">
    <div class="yc-in col">
      <button type="button" class="backlink" onclick={() => back({ name: "settings" })}><Icon name="back" size={14} />{t("Настройки")}</button>

      <header>
        <h1>{t("Модели")}</h1>
        <span class="yc-pretty lead">{t("Работают без интернета. Качаются один раз с Hugging Face — если связь пропадёт, докачаются с того же места.")}</span>
      </header>

      {#if hardware}
        <div class="hw">
          <span class="hw-ic"><Icon name="cpu" size={18} /></span>
          <span style="flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 1px">
            <span style="font-size: 13px; font-weight: 700">{hwLine}</span>
            <span style="font-size: 12px; color: var(--muted)">{t("Ускорение")}: <span class="mg-mono">{hardware.backend.toUpperCase()}</span> · {t("оценки ниже посчитаны под эту машину")}</span>
          </span>
        </div>

        <div style="display: flex; flex-direction: column; gap: 6px; padding: 2px 2px 4px">
          <div style="display: flex; justify-content: space-between; gap: 10px; font-size: 12px; color: var(--muted)">
            <span>{t("Модели занимают")} <span class="mg-mono" style="color: var(--text)">{bytes(used)}</span></span>
            <span>{t("свободно на диске")} <span class="mg-mono">{bytes(hardware.diskFreeBytes)}</span></span>
          </div>
          <span aria-hidden="true" class="usedbar"><span style="width: {usedW}%"></span></span>
        </div>
      {/if}

      {#each kinds as k (k.kind)}
        {@const list = models.filter((m) => m.kind === k.kind)}
        {#if list.length}
          <div style="padding: 10px 4px 0; display: flex; flex-direction: column; gap: 2px"><SectionLabel as="h2">{t(k.title)}</SectionLabel></div>
          <div style="display: flex; flex-direction: column; gap: 6px">
            {#each list as m (m.id)}
              {@const d = m.download}
              {@const ds = d?.state.state}
              {@const vp = verdictPill[m.verdict]}
              <div class="card" style="border-color: {m.active && m.installed ? 'var(--accent-line)' : ds === 'failed' ? 'var(--warning-line)' : 'var(--border)'}">
                <div style="display: flex; align-items: center; gap: 12px">
                  <span style="flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px">
                    <span style="display: flex; align-items: center; gap: 8px; min-width: 0">
                      <span class="mg-mono yc-ellipsis" style="font-size: 13px; font-weight: 700">{m.name}</span>
                      {#if m.active && m.installed && (k.kind === "asr" || k.kind === "llm")}<span class="yc-popin tag-main">{t("основная")}</span>{/if}
                    </span>
                    <span style="font-size: 12px; color: var(--muted)"><span class="mg-mono">{bytes(m.sizeBytes)}</span> · {t(m.blurb)}</span>
                  </span>

                  {#if !m.installed || ds === "failed"}<span class="vp"><StatusPill kind={vp.kind} label={t(vp.label)} sm /></span>{/if}

                  {#if m.installed && !m.active && (k.kind === "asr" || k.kind === "llm")}
                    <Button size="sm" onclick={() => act(() => api.modelSetActive(m.id))}>{t("Сделать основной")}</Button>
                    <IconButton variant="flat" aria-label={t("Удалить модель")} onclick={() => act(() => api.modelRemove(m.id))}><Icon name="trash" size={14} /></IconButton>
                  {:else if m.installed && !d && m.benchmarkable !== false}
                    <Button size="sm" variant="ghost" disabled={benching === m.id} onclick={() => bench(m)}>{benching === m.id ? t("Меряю…") : t("Измерить")}</Button>
                  {:else if !m.installed && !d}
                    <Button size="sm" disabled={m.verdict === "wontRun"} onclick={() => act(() => api.modelDownload(m.id))}>
                      <svg class="yc-ic" width="12" height="12" viewBox="0 0 24 24"><path d="M12 4v11M7 10l5 5 5-5M5 20h14" /></svg>{t("Скачать")}
                    </Button>
                  {:else if ds === "running" || ds === "queued"}
                    <span class="mg-mono yc-num" style="font-size: 13px; font-weight: 700; color: var(--accent-text)">{pctOf(m)}%</span>
                    <IconButton variant="flat" aria-label={t("Пауза")} onclick={() => act(() => api.modelPause(m.id))}><svg class="yc-ic" width="13" height="13" viewBox="0 0 24 24" style="stroke-width: 3"><path d="M8 5v14M16 5v14" /></svg></IconButton>
                    <IconButton variant="flat" aria-label={t("Отменить загрузку")} onclick={() => act(() => api.modelCancel(m.id))}><Icon name="x" size={14} /></IconButton>
                  {:else if ds === "paused"}
                    <Button size="sm" onclick={() => act(() => api.modelDownload(m.id))}>{t("Продолжить")}</Button>
                    <IconButton variant="flat" aria-label={t("Отменить загрузку")} onclick={() => act(() => api.modelCancel(m.id))}><Icon name="x" size={14} /></IconButton>
                  {:else if ds === "verifying"}
                    <span style="display: inline-flex; align-items: center; gap: 7px; font-size: 12px; color: var(--muted)"><svg class="yc-ic yc-spin" width="13" height="13" viewBox="0 0 24 24" style="color: var(--accent-text); stroke-width: 3"><path d="M12 3a9 9 0 1 0 9 9" /></svg>{t("проверяю")}</span>
                  {:else if ds === "failed"}
                    <Button size="sm" onclick={() => act(() => api.modelDownload(m.id))}><Icon name="refresh" size={12} />{t("Повторить")}</Button>
                    <IconButton variant="flat" aria-label={t("Отменить загрузку")} onclick={() => act(() => api.modelCancel(m.id))}><Icon name="x" size={14} /></IconButton>
                  {/if}
                </div>

                <span class="est"><span class="edot" style="background: var(--{m.verdict === 'great' ? 'ok' : m.verdict === 'ok' ? 'blip' : m.verdict === 'slow' ? 'warning' : 'danger'})"></span>{measured[m.id] ? t("измерено: ≈ {x}× быстрее реального времени", { x: measured[m.id] }) : t(m.estimate)}</span>

                {#if d && ds !== "done"}
                  <div class="yc-in" style="display: flex; flex-direction: column; gap: 6px">
                    <span aria-hidden="true" class="pbar">
                      <span style="width: {pctOf(m)}%; background: {ds === 'running' || ds === 'queued' ? 'linear-gradient(90deg, var(--accent), var(--accent-soft), var(--accent))' : ds === 'failed' ? 'var(--warning)' : ds === 'verifying' ? 'var(--accent)' : 'var(--faint)'}; background-size: 200% 100%; animation: {ds === 'running' ? 'ycShimmer 1.6s linear infinite' : 'none'}"></span>
                    </span>
                    <span style="display: flex; align-items: center; gap: 8px; font-size: 11px; color: {ds === 'failed' ? 'var(--warning-text)' : 'var(--muted)'}">
                      <span class="mg-mono yc-num" style="flex: 1">
                        {#if ds === "running" || ds === "queued"}{bytes(d.doneBytes)} {t("из")} {bytes(d.totalBytes)} · {mbps(d.bytesPerSec)}
                        {:else if ds === "paused"}{t("На паузе")} · {bytes(d.doneBytes)} {t("из")} {bytes(d.totalBytes)}
                        {:else if ds === "failed"}{t("Связь прервалась на {p}% — докачаю с этого места", { p: pctOf(m) })}
                        {:else}{t("Скачано · сверяю контрольную сумму")}{/if}
                      </span>
                      <span class="mg-mono yc-num">{ds === "running" ? etaLabel(d.etaSecs) : ""}</span>
                    </span>
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        {/if}
      {/each}

      <span class="note"><Icon name="info" size={12} />{t("Запись не ждёт загрузку: пока качается новая, расшифровывает основная.")}</span>
    </div>
  </div>

  {#if toastModel}
    <div style="position: absolute; left: 50%; bottom: 20px; transform: translateX(-50%)">
      <Toast message={t("{name} скачана и проверена", { name: toastModel })} />
    </div>
  {/if}
</main>

<style>
  main { position: relative; flex: 1 1 520px; min-width: 0; display: flex; flex-direction: column; min-height: 0; }
  .col { max-width: 600px; box-sizing: border-box; margin: 0 auto; padding: 24px 28px 40px; display: flex; flex-direction: column; gap: 14px; }
  .backlink { align-self: flex-start; display: inline-flex; align-items: center; gap: 6px; padding: 0; border: 0; background: none; font-size: 13px; color: var(--muted); }
  header { display: flex; flex-direction: column; gap: 4px; }
  h1 { margin: 0; font-size: 28px; line-height: 1.15; font-weight: 800; letter-spacing: -0.04em; }
  .lead { font-size: 13px; color: var(--muted); }
  .hw { display: flex; align-items: center; gap: 12px; padding: 11px 14px; border: 1px solid var(--border); border-radius: 14px; background: var(--surface); }
  .hw-ic { width: 36px; height: 36px; flex: none; border-radius: 11px; background: var(--accent-soft); color: var(--accent-text); display: grid; place-items: center; }
  .usedbar { display: block; height: 4px; border-radius: 2px; overflow: hidden; background: var(--surface-2); }
  .usedbar span { display: block; height: 100%; border-radius: 2px; background: var(--accent); transition: width 0.4s var(--ease-out-soft); }
  .card { position: relative; display: flex; flex-direction: column; gap: 9px; padding: 12px 12px 12px 14px; border: 1px solid var(--border); border-radius: 14px; background: var(--surface); transition: border-color 0.3s; }
  .tag-main { flex: none; height: 20px; padding: 0 8px; border-radius: 10px; background: var(--accent); color: var(--ink-on-accent); font-size: 11px; font-weight: 800; display: inline-flex; align-items: center; }
  .vp { flex: none; }
  .est { display: flex; align-items: center; gap: 7px; font-size: 12px; color: var(--muted); }
  .edot { width: 6px; height: 6px; border-radius: 50%; flex: none; }
  .pbar { position: relative; display: block; height: 6px; border-radius: 3px; overflow: hidden; background: var(--surface-2); }
  .pbar span { position: absolute; left: 0; top: 0; bottom: 0; border-radius: 3px; transition: width 0.2s; }
  .note { display: flex; align-items: center; gap: 7px; padding: 2px 2px 0; font-size: 12px; color: var(--muted); }
</style>
