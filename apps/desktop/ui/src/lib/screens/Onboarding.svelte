<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import Button from "../kit/Button.svelte";
  import Segmented from "../kit/Segmented.svelte";
  import ThemeToggle from "../kit/ThemeToggle.svelte";
  import StatusPill from "../kit/StatusPill.svelte";
  import Mascot from "../kit/Mascot.svelte";
  import { api } from "../api";
  import type { ModelEntry, OllamaStatus, Permissions, SummaryMode } from "../api";
  import { app, saveSettings, theme, onEngineEvent } from "../state.svelte";
  import { t, i18n } from "../i18n/index.svelte";
  import { go } from "../router.svelte";
  import { isMac } from "../platform";
  import { bytes } from "../format";

  let perms = $state<Permissions>({ microphone: "notDetermined", systemAudio: "notDetermined" });
  let model = $state<ModelEntry | null>(null);
  type Choice = "local" | "ollama" | "off";
  let choice = $state<Choice>("local");
  let gemma = $state<ModelEntry | null>(null);
  let ollama = $state<OllamaStatus | null>(null);

  onMount(() => {
    api.permissions().then((p) => (perms = p));
    // the smallest Gemma the predictor knows, and whether Ollama is around
    api.modelsOverview().then((o) => (gemma = o.models.filter((m) => m.kind === "llm").sort((a, b) => a.sizeBytes - b.sizeBytes)[0] ?? null));
    api.ollamaStatus("local").then((st) => (ollama = st)).catch(() => (ollama = null));
    api.modelsOverview().then(async (o) => {
      model = o.models.filter((m) => m.kind === "asr").sort((a, b) => Number(b.active) - Number(a.active) || a.sizeBytes - b.sizeBytes)[0] ?? null;
      if (model && !model.installed && !model.download) { await api.modelDownload(model.id); }
    });
    return onEngineEvent((e) => {
      if (e.type !== "download" || !model || e.item !== model.id) return;
      if (e.state.state === "done") model = { ...model, installed: true, download: undefined };
      else model = { ...model, download: { state: e.state, doneBytes: e.doneBytes, totalBytes: e.totalBytes, bytesPerSec: e.bytesPerSec } };
    });
  });

  const pct = $derived(model?.installed ? 100 : model?.download ? Math.min(100, (model.download.doneBytes / Math.max(1, model.download.totalBytes)) * 100) : 0);
  const dlLabel = $derived(pct >= 100 ? t("готово") : model ? `${bytes(model.download?.doneBytes ?? 0)} ${t("из")} ${bytes(model.sizeBytes)}` : "");

  async function grantSystem() { perms = await api.requestPermission("systemAudio"); }
  /** What Ollama offers for text: the account's cloud models first, never embedders. */
  const found = $derived.by(() => {
    if (!ollama?.running) return null;
    const chat = ollama.models.filter((m) => !/embed/i.test(m.name));
    return (ollama.signedIn ? chat.find((m) => m.cloud) : undefined) ?? chat.find((m) => !m.cloud) ?? null;
  });
  const speed = $derived(gemma?.estimate.split("·")[0].trim() ?? "");
  const gemmaLine = $derived(gemma ? [gemma.name, bytes(gemma.sizeBytes), speed].filter(Boolean).join(" · ") : t("Gemma 4"));
  $effect(() => { if (choice === "ollama" && !found) choice = "local"; });

  async function finish() {
    let mode: SummaryMode = { mode: "off" };
    if (choice === "local") mode = { mode: "local", model: gemma?.id ?? "" };
    else if (choice === "ollama" && found) mode = { mode: "ollama", base: "local", model: found.name };
    await saveSettings({ summaries: { mode }, onboarded: true });
    // The chosen on-device model downloads in the background, like the recogniser.
    if (choice === "local" && gemma && !gemma.installed && !gemma.download) api.modelDownload(gemma.id).catch(() => {});
    go({ name: "meetings" });
  }
</script>

<div class="wrap">
  <div class="top">
    <Segmented variant="lang" label={t("Язык")} value={i18n.lang} onchange={(v) => saveSettings({ uiLanguage: v })} options={[{ value: "ru", label: "RU" }, { value: "en", label: "EN" }]} />
    <ThemeToggle dark={theme.resolved === "dark"} onchange={(v) => saveSettings({ theme: v })} />
  </div>

  <div class="yc-in inner">
    <div class="hero">
      <div class="yc-float" style="width: 104px; height: 104px"><Mascot size={104} accent={theme.accent} /></div>
      <div style="display: flex; flex-direction: column; gap: 6px">
        <h1 class="yc-balance">{t("Пара шагов — и можно записывать")}</h1>
        <p class="yc-pretty sub">{t("Записи и расшифровки остаются на этом компьютере.")}</p>
      </div>
    </div>

    <div class="card">
      <div class="row">
        <span aria-hidden="true" class="ic"><Icon name="mic" size={15} /></span>
        <span style="flex: 1; min-width: 0; font-size: 14px; font-weight: 600">{t("Микрофон")}</span>
        {#if perms.microphone === "granted"}<StatusPill kind="ok" sm label={t("Разрешено")} />
        {:else}<Button size="sm" onclick={async () => (perms = await api.requestPermission("microphone"))}>{t("Разрешить")}</Button>{/if}
      </div>
      <div class="row bt">
        <span aria-hidden="true" class="ic"><Icon name="monitor" size={15} /></span>
        <span class="two"><span style="font-size: 14px; font-weight: 600">{t("Звук собеседников")}</span><span class="s">{isMac ? t("macOS отдаёт его через разрешение «Запись экрана»") : t("Системный звук записывается без дополнительных разрешений")}</span></span>
        {#if perms.systemAudio === "granted" || !isMac && false}<StatusPill kind="ok" sm label={t("Разрешено")} />
        {:else}<Button size="sm" onclick={grantSystem}>{t("Разрешить")}</Button>{/if}
      </div>
      <div class="col bt">
        <div style="display: flex; align-items: center; gap: 12px">
          <span aria-hidden="true" class="ic"><Icon name="cpu" size={16} /></span>
          <span class="two"><span style="font-size: 14px; font-weight: 600">{t("Модель расшифровки")}</span><span class="mg-mono" style="font-size: 11px; color: var(--muted)">{model?.name ?? "…"} · RU · EN</span></span>
          <span class="mg-mono yc-num" style="font-size: 12px; color: var(--muted)">{dlLabel}</span>
        </div>
        <span aria-hidden="true" class="bar"><span style="width: {pct}%"></span></span>
      </div>
      <div class="col bt" role="radiogroup" aria-label={t("Итоги встреч")}>
        <div style="display: flex; align-items: center; gap: 12px">
          <span aria-hidden="true" class="ic"><Icon name="sparkle" size={16} /></span>
          <span class="two"><span style="font-size: 14px; font-weight: 600">{t("Итоги встреч")}</span><span class="s">{t("Можно не собирать — останется только расшифровка")}</span></span>
        </div>
        <button type="button" role="radio" aria-checked={choice === "local"} class="opt yc-press" class:on={choice === "local"} onclick={() => (choice = "local")}>
          <span aria-hidden="true" class="dot"></span>
          <span class="two"><span class="ot">{t("На компьютере")}</span><span class="s mg-mono">{gemmaLine}</span></span>
        </button>
        {#if found}
          <button type="button" role="radio" aria-checked={choice === "ollama"} class="opt yc-press" class:on={choice === "ollama"} onclick={() => (choice = "ollama")}>
            <span aria-hidden="true" class="dot"></span>
            <span class="two"><span class="ot">{t("Через Ollama")}</span><span class="s mg-mono">{t("найдено: {name}", { name: found.name })}</span></span>
          </button>
        {/if}
        <button type="button" role="radio" aria-checked={choice === "off"} class="opt yc-press" class:on={choice === "off"} onclick={() => (choice = "off")}>
          <span aria-hidden="true" class="dot"></span>
          <span class="two"><span class="ot">{t("Без итогов")}</span><span class="s">{t("Только расшифровка и поиск")}</span></span>
        </button>
        <span class="s" style="padding: 0 2px">{choice === "ollama" && found?.cloud ? t("Текст расшифровки уходит в Ollama Cloud, звук — никогда") : choice === "off" ? t("Позже можно включить в настройках — в том числе Claude или Ollama Cloud по ключу") : t("Всё считается на этом компьютере, ничего не уходит в сеть")}</span>
      </div>
    </div>

    <div style="display: flex; flex-direction: column; align-items: center; gap: 10px">
      <button type="button" class="yc-press done" onclick={finish}>{t("Готово")}</button>
      <span style="font-size: 12px; color: var(--muted)">{pct >= 100 ? t("Всё готово к первой записи") : t("Модель докачается в фоне")}</span>
    </div>
  </div>
</div>

<style>
  .wrap { min-height: 100vh; height: 100%; box-sizing: border-box; position: relative; display: flex; align-items: center; justify-content: center; padding: 48px 20px; background: var(--bg); overflow-y: auto; }
  .top { position: absolute; top: 16px; right: 18px; display: flex; align-items: center; gap: 10px; z-index: 60; }
  .inner { width: 100%; max-width: 400px; display: flex; flex-direction: column; gap: 22px; }
  .hero { display: flex; flex-direction: column; align-items: center; gap: 14px; text-align: center; }
  h1 { margin: 0; font-size: 24px; line-height: 1.2; font-weight: 800; letter-spacing: -0.035em; }
  .sub { margin: 0; font-size: 14px; color: var(--muted); }
  .card { border: 1px solid var(--border); border-radius: 16px; background: var(--surface); }
  .row { display: flex; align-items: center; gap: 12px; padding: 14px 16px; }
  .col { display: flex; flex-direction: column; gap: 10px; padding: 14px 16px; }
  .bt { border-top: 1px solid var(--border); }
  .ic { width: 30px; height: 30px; flex: none; border-radius: 9px; background: var(--surface-2); color: var(--muted); display: grid; place-items: center; }
  .two { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .s { font-size: 12px; color: var(--muted); }
  .bar { display: block; height: 5px; border-radius: 3px; background: var(--surface-2); overflow: hidden; }
  .bar span { display: block; height: 100%; border-radius: 3px; background: var(--accent); transition: width 0.6s var(--ease-out-soft); }
  .opt { display: flex; align-items: center; gap: 12px; width: 100%; box-sizing: border-box; padding: 10px 12px; border: 1px solid var(--border); border-radius: 12px; background: var(--bg); color: var(--text); text-align: left; font: inherit; }
  .opt.on { border-color: var(--accent); background: var(--accent-soft); }
  .opt .dot { flex: none; width: 16px; height: 16px; box-sizing: border-box; border: 1.5px solid var(--border-strong, var(--muted)); border-radius: 50%; }
  .opt.on .dot { border: 5px solid var(--accent); }
  .ot { font-size: 14px; font-weight: 600; }
  .done { width: 100%; display: flex; align-items: center; justify-content: center; height: 44px; border: 0; border-radius: 14px; background: var(--accent); color: var(--ink-on-accent); font-size: 14px; font-weight: 800; }
</style>
