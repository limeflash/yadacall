<script lang="ts">
  import { onMount } from "svelte";
  import Button from "../kit/Button.svelte";
  import Segmented from "../kit/Segmented.svelte";
  import Select from "../kit/Select.svelte";
  import Switch from "../kit/Switch.svelte";
  import Kbd from "../kit/Kbd.svelte";
  import SectionLabel from "../kit/SectionLabel.svelte";
  import AccentPicker from "../kit/AccentPicker.svelte";
  import Mascot from "../kit/Mascot.svelte";
  import StatusPill from "../kit/StatusPill.svelte";
  import { api } from "../api";
  import type { ModelEntry, Permissions, SummaryProvider, ThemeChoice, KeepAudio } from "../api";
  import { app, saveSettings, theme } from "../state.svelte";
  import { t } from "../i18n/index.svelte";
  import { go } from "../router.svelte";
  import { bytes } from "../format";
  import { prettyAccel, isMac } from "../platform";
  import { toast } from "../toast.svelte";

  const s = $derived(app.settings!);
  let asr = $state<ModelEntry[]>([]);
  let perms = $state<Permissions | null>(null);
  let capturing = $state<null | "hotkeyRecord" | "hotkeyBookmark">(null);
  let apiKey = $state("");

  async function loadModels() { asr = (await api.modelsOverview()).models.filter((m) => m.kind === "asr"); }
  onMount(() => { loadModels(); api.permissions().then((p) => (perms = p)); });
  // keep the three ASR rows live while something downloads
  $effect(() => { const d = JSON.stringify(app.downloads); void d; loadModels(); });

  const L = { Bg: "#f8f8f6", Surf: "#ffffff", Line: "#e3e3df" };
  const D = { Bg: "#0c0c0e", Surf: "#1d1d22", Line: "#2c2c33" };
  const themeOpts: { id: ThemeChoice; label: string; A: typeof L; B: typeof L; clip: string }[] = [
    { id: "system", label: "Система", A: L, B: D, clip: "polygon(62% 0, 100% 0, 100% 100%, 38% 100%)" },
    { id: "light", label: "Светлая", A: L, B: L, clip: "none" },
    { id: "dark", label: "Тёмная", A: D, B: D, clip: "none" },
  ];
  const providers = $derived<{ value: SummaryProvider; label: string; hint?: string }[]>([
    { value: "claude", label: "Claude", hint: t("облако, только текст") },
    { value: "local", label: "Gemma 4", hint: t("на компьютере, без интернета") },
    { value: "ollama", label: "Ollama", hint: t("локальный сервер") },
    { value: "openai", label: t("OpenAI-совместимый"), hint: "OpenRouter, LM Studio…" },
  ]);
  const modelOptions: Record<SummaryProvider, { value: string; label: string; hint?: string }[]> = {
    claude: [{ value: "claude-sonnet-5-5", label: "Claude Sonnet 5.5", hint: "по умолчанию" }, { value: "claude-haiku-5-5", label: "Claude Haiku 5.5", hint: "быстрее и дешевле" }, { value: "claude-opus-5-5", label: "Claude Opus 5.5", hint: "для длинных встреч" }],
    local: [{ value: "gemma-4-e4b", label: "Gemma 4 E4B", hint: "рекомендуется" }, { value: "gemma-4-e2b", label: "Gemma 4 E2B", hint: "легче" }],
    ollama: [{ value: "gemma4:e4b", label: "gemma4:e4b" }, { value: "llama3.1:8b", label: "llama3.1:8b" }],
    openai: [],
  };
  const keepOptions = $derived<{ value: KeepAudio; label: string }[]>([{ value: "none", label: t("Не хранить") }, { value: "7d", label: t("7 дней") }, { value: "30d", label: t("30 дней") }, { value: "forever", label: t("Всегда") }]);
  const provider = $derived(s.summaries.provider);
  const remote = $derived(provider === "claude" || provider === "openai");

  function accelKeys(a: string): string[] {
    const pretty = prettyAccel(a);
    return isMac ? pretty.split("") : a.split("+");
  }
  function onKey(e: KeyboardEvent) {
    if (!capturing) return;
    e.preventDefault();
    if (e.key === "Escape") { capturing = null; return; }
    if (["Shift", "Alt", "Control", "Meta"].includes(e.key)) return;
    const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Command"].filter(Boolean) as string[];
    if (!mods.length) { toast(t("Добавь Alt, Shift, Ctrl или ⌘")); return; }
    const key = e.code.startsWith("Key") ? e.code.slice(3) : e.code.startsWith("Digit") ? e.code.slice(5) : e.code;
    saveSettings({ [capturing]: [...mods, key].join("+") });
    capturing = null;
  }
  async function pickFolder() { const f = await api.pickFolder(); if (f) saveSettings({ libraryDir: f }); }
  async function askPerm(k: "microphone" | "systemAudio") { perms = await api.requestPermission(k); }
  function saveKey() { if (apiKey.trim()) { saveSettings({ summaries: { apiKey: apiKey.trim() } }); apiKey = ""; toast(t("Ключ сохранён в связке ключей системы")); } }
  function modelBtn(m: ModelEntry): string {
    const d = m.download;
    if (m.active && m.installed) return t("Основная");
    if (m.installed) return t("Сделать основной");
    if (d && d.state.state === "running") return Math.floor((d.doneBytes / d.totalBytes) * 100) + "%";
    return t("Скачать");
  }
  async function modelAct(m: ModelEntry) {
    if (m.installed && !m.active) { await api.modelSetActive(m.id); loadModels(); }
    else if (!m.installed && !m.download) { await api.modelDownload(m.id); loadModels(); }
  }
</script>

<svelte:window onkeydown={onKey} />

<main>
  <div class="yc-scroll" style="flex: 1; min-height: 0">
    <div class="yc-in col">
      <h1>{t("Настройки")}</h1>

      <div style="padding: 0 4px"><SectionLabel as="h2">{t("Тема")}</SectionLabel></div>
      <div role="radiogroup" aria-label={t("Тема")} class="themes">
        {#each themeOpts as o (o.id)}
          {@const on = s.theme === o.id}
          <button type="button" role="radio" aria-checked={on} onclick={() => saveSettings({ theme: o.id })} class="yc-lift tcard" style="background: {on ? 'var(--accent-soft)' : 'var(--surface)'}; box-shadow: {on ? 'inset 0 0 0 2px var(--accent)' : 'inset 0 0 0 1px var(--border)'}; transform: scale({on ? 1 : 0.97})">
            <span class="prev" style="box-shadow: 0 0 0 1px {theme.resolved === 'dark' ? 'rgba(255,255,255,.1)' : 'rgba(0,0,0,.08)'}">
              <span class="layer" style="background: {o.A.Bg}"><span class="sun"></span><span class="bar" style="background: {o.A.Surf}; box-shadow: inset 0 0 0 1px {o.A.Line}"></span></span>
              <span class="layer" style="background: {o.B.Bg}; clip-path: {o.clip}"><span class="sun"></span><span class="bar" style="background: {o.B.Surf}; box-shadow: inset 0 0 0 1px {o.B.Line}"></span></span>
            </span>
            <span class="tl">
              {#if on}<span class="yc-popin chk"><span></span></span>{/if}
              <span style="font-size: 12px; font-weight: {on ? 800 : 600}; color: {on ? 'var(--text)' : 'var(--muted)'}">{t(o.label)}</span>
            </span>
          </button>
        {/each}
      </div>
      <div style="padding: 10px 2px 4px"><AccentPicker value={s.accent} onchange={(hex) => saveSettings({ accent: hex })} /></div>

      <div class="hrow">
        <SectionLabel as="h2">{t("Модель расшифровки")}</SectionLabel>
        <button type="button" class="yc-press alllink" onclick={() => go({ name: "models" })}>{t("Все модели")} →</button>
      </div>
      <div style="display: flex; flex-direction: column; gap: 6px">
        {#each asr.slice(0, 3) as m (m.id)}
          {@const d = m.download}
          {@const fill = d && d.state.state === "running" ? (d.doneBytes / d.totalBytes) * 100 : 0}
          <div class="model" style="border-color: {m.active && m.installed ? 'var(--accent-line)' : 'var(--border)'}">
            <span aria-hidden="true" class="fill" style="width: {fill}%"></span>
            <span style="position: relative; flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px">
              <span class="mg-mono" style="font-size: 13px; font-weight: 700">{m.name}</span>
              <span style="font-size: 12px; color: var(--muted)">{bytes(m.sizeBytes)} · {m.blurb}</span>
            </span>
            <button type="button" class="mbtn" class:primary={m.active && m.installed} class:mono={!!d && !m.installed} onclick={() => modelAct(m)}>{modelBtn(m)}</button>
          </div>
        {/each}
      </div>
      <div class="group">
        <div class="grow">
          <span style="flex: 1; min-width: 0; font-size: 14px">{t("Язык встреч")}</span>
          <Segmented variant="inset" label={t("Язык встреч")} value={s.meetingLanguage} onchange={(v) => saveSettings({ meetingLanguage: v })} options={[{ value: "auto", label: t("Авто") }, { value: "ru", label: "RU" }, { value: "en", label: "EN" }]} />
        </div>
        <label class="grow bt">
          <span class="two"><span style="font-size: 14px">{t("Узнавать голоса")}</span><span class="sub">{t("Подписывает знакомых по прошлым встречам")}</span></span>
          <Switch checked={s.recognizeVoices} onchange={(v) => saveSettings({ recognizeVoices: v })} />
        </label>
      </div>

      <div style="padding: 14px 4px 0"><SectionLabel as="h2">{t("Итоги")}</SectionLabel></div>
      <div class="group">
        <div class="grow">
          <span style="flex: 1; min-width: 0; font-size: 14px">{t("Чем собирать итоги")}</span>
          <div style="width: 220px; flex: none"><Select compact label={t("Чем собирать итоги")} value={provider} onchange={(v) => saveSettings({ summaries: { provider: v, model: modelOptions[v][0]?.value ?? s.summaries.model } })} options={providers} /></div>
        </div>
        {#if modelOptions[provider].length}
          <div class="grow bt">
            <span style="flex: 1; min-width: 0; font-size: 14px">{t("Модель")}</span>
            <div style="width: 220px; flex: none"><Select compact label={t("Модель итогов")} value={s.summaries.model} onchange={(v) => saveSettings({ summaries: { model: v } })} options={modelOptions[provider].map((o) => ({ ...o, hint: o.hint ? t(o.hint) : undefined }))} /></div>
          </div>
        {/if}
        {#if provider === "openai" || provider === "ollama"}
          <label class="grow bt">
            <span style="flex: 1; min-width: 0; font-size: 14px">{provider === "ollama" ? "Ollama URL" : t("Адрес API")}</span>
            <input class="tf mg-mono" style="width: 220px" value={s.summaries.baseUrl ?? (provider === "ollama" ? "http://localhost:11434" : "")} placeholder="https://…/v1" onchange={(e) => saveSettings({ summaries: { baseUrl: e.currentTarget.value } })} />
          </label>
          {#if provider === "openai"}
            <label class="grow bt"><span style="flex: 1; min-width: 0; font-size: 14px">{t("Модель (имя)")}</span>
              <input class="tf mg-mono" style="width: 220px" value={s.summaries.model} onchange={(e) => saveSettings({ summaries: { model: e.currentTarget.value } })} /></label>
          {/if}
        {/if}
        {#if provider === "claude" || provider === "openai"}
          <div class="grow bt">
            <span class="two" style="flex: 1"><span style="font-size: 14px">{t("Ключ API")}</span><span class="sub">{s.summaries.apiKeySet ? t("Сохранён в связке ключей системы") : t("Хранится в связке ключей системы, не в файлах")}</span></span>
            <input class="tf mg-mono" style="width: 150px" type="password" bind:value={apiKey} placeholder={s.summaries.apiKeySet ? "••••••••" : "sk-…"} autocomplete="off" onkeydown={(e) => { if (e.key === "Enter") saveKey(); }} />
            <Button size="sm" onclick={saveKey} disabled={!apiKey.trim()}>{t("Сохранить")}</Button>
          </div>
        {/if}
      </div>
      <div class="hint">{remote ? t("Уходит только текст расшифровки, аудио остаётся у тебя") : t("Всё считается на этом компьютере, ничего не уходит в сеть")}</div>

      <div style="padding: 14px 4px 0"><SectionLabel as="h2">{t("Хранить аудио")}</SectionLabel></div>
      <Segmented variant="thumb" label={t("Сколько хранить аудио")} value={s.keepAudio} onchange={(v) => saveSettings({ keepAudio: v })} options={keepOptions} />
      <div class="hint" style="padding-top: 2px">{t("Потом аудио удаляется само — расшифровка и итоги остаются")}</div>

      <div style="padding: 14px 4px 0"><SectionLabel as="h2">{t("Запись")}</SectionLabel></div>
      <div class="group">
        {#each [["hotkeyRecord", "Начать или остановить"], ["hotkeyBookmark", "Поставить закладку"]] as [key, label], i (key)}
          <div class="grow" class:bt={i > 0}>
            <span style="flex: 1; min-width: 0; font-size: 14px">{t(label)}</span>
            <button type="button" class="kbtn" onclick={() => (capturing = key as "hotkeyRecord")} aria-label={t("Изменить сочетание")} title={t("Нажми, чтобы изменить")}>
              {#if capturing === key}<span class="cap">{t("нажми сочетание…")}</span>{:else}<Kbd keys={accelKeys(s[key as "hotkeyRecord"])} />{/if}
            </button>
          </div>
        {/each}
        <div class="grow bt">
          <span style="flex: 1; min-width: 0; font-size: 14px">{t("Папка записей")}</span>
          <span class="mg-mono" style="font-size: 12px; color: var(--muted)">{s.libraryDir}</span>
          <Button size="sm" onclick={pickFolder}>{t("Изменить")}</Button>
        </div>
        <label class="grow bt"><span style="flex: 1; min-width: 0; font-size: 14px">{t("Запуск при входе")}</span><Switch checked={s.launchAtLogin} onchange={(v) => saveSettings({ launchAtLogin: v })} /></label>
        <label class="grow bt">
          <span class="two"><span style="font-size: 14px">{t("Напоминать предупредить о записи")}</span><span class="sub">{t("Перед стартом подскажет сказать собеседникам, что идёт запись")}</span></span>
          <Switch checked={s.recordingReminder} onchange={(v) => saveSettings({ recordingReminder: v })} />
        </label>
        <label class="grow bt"><span style="flex: 1; min-width: 0; font-size: 14px">{t("Мини-плеер поверх окон")}</span><Switch checked={s.miniPlayer} onchange={(v) => saveSettings({ miniPlayer: v })} /></label>
      </div>

      {#if perms}
        <div style="padding: 14px 4px 0"><SectionLabel as="h2">{t("Разрешения")}</SectionLabel></div>
        <div class="group">
          {#each [["microphone", "Микрофон", "Ваш голос"], ["systemAudio", "Звук собеседников", isMac ? "macOS отдаёт его через разрешение «Запись экрана»" : "Системный звук"]] as [k, label, sub], i (k)}
            {@const st = perms[k as "microphone"]}
            <div class="grow" class:bt={i > 0}>
              <span class="two" style="flex: 1"><span style="font-size: 14px">{t(label)}</span><span class="sub">{t(sub)}</span></span>
              {#if st === "granted"}<StatusPill kind="ok" label={t("Разрешено")} sm />
              {:else if st === "denied"}<Button size="sm" onclick={() => api.openSystemSettings(k as "microphone")}>{t("Открыть настройки")}</Button>
              {:else}<Button size="sm" onclick={() => askPerm(k as "microphone")}>{t("Разрешить")}</Button>{/if}
            </div>
          {/each}
        </div>
      {/if}

      <div class="foot">
        <div style="width: 28px; height: 28px; flex: none"><Mascot size={28} variant={theme.resolved === "dark" ? "white" : "black"} accent={theme.accent} /></div>
        <span class="mg-mono" style="font-size: 11px; color: var(--muted)">YadaCall 0.1.0 · MIT · {t("исходники на GitHub")}</span>
      </div>
    </div>
  </div>
</main>

<style>
  main { flex: 1 1 520px; min-width: 0; display: flex; flex-direction: column; min-height: 0; }
  .col { max-width: 600px; box-sizing: border-box; margin: 0 auto; padding: 40px 28px 56px; display: flex; flex-direction: column; gap: 10px; }
  h1 { margin: 0 0 14px 4px; font-size: 30px; line-height: 1.15; font-weight: 800; letter-spacing: -0.04em; }
  .themes { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8px; }
  .tcard { display: flex; flex-direction: column; gap: 8px; padding: 6px 6px 9px; border: 0; border-radius: 16px; color: var(--text); transition: box-shadow 0.3s, background-color 0.3s, transform 0.35s var(--ease-spring); }
  .prev { position: relative; display: block; height: 66px; border-radius: 11px; overflow: hidden; }
  .layer { position: absolute; inset: 0; display: block; }
  .sun { position: absolute; left: 50%; top: 14px; width: 24px; height: 24px; margin-left: -12px; border-radius: 50%; background: var(--accent); }
  .bar { position: absolute; left: 10px; right: 10px; bottom: 9px; height: 9px; border-radius: 4px; }
  .tl { display: flex; align-items: center; justify-content: center; gap: 5px; height: 16px; }
  .chk { width: 14px; height: 14px; border-radius: 50%; background: var(--accent); display: grid; place-items: center; }
  .chk span { display: block; width: 3px; height: 6px; margin-top: -2px; border-right: 1.8px solid var(--ink-on-accent); border-bottom: 1.8px solid var(--ink-on-accent); transform: rotate(45deg); }
  .hrow { display: flex; align-items: flex-end; justify-content: space-between; gap: 12px; padding: 14px 4px 0; }
  .alllink { flex: none; margin-bottom: 8px; padding: 0; border: 0; background: none; font-size: 12.5px; font-weight: 600; color: var(--accent-text); white-space: nowrap; }
  .model { position: relative; overflow: hidden; display: flex; align-items: center; gap: 12px; padding: 11px 12px 11px 14px; border: 1px solid var(--border); border-radius: 14px; background: var(--surface); transition: border-color 0.3s; }
  .fill { position: absolute; left: 0; top: 0; bottom: 0; background: var(--accent-soft); transition: width 0.2s; }
  .mbtn { position: relative; height: 28px; padding: 0 12px; border: 1px solid var(--border); border-radius: 11px; background: transparent; color: var(--text); font-size: 12px; font-weight: 700; white-space: nowrap; }
  .mbtn.primary { background: var(--accent); color: var(--ink-on-accent); border-color: transparent; }
  .mbtn.mono { font-family: var(--font-mono); }
  .group { border: 1px solid var(--border); border-radius: 16px; background: var(--surface); }
  .grow { display: flex; align-items: center; gap: 16px; padding: 12px 16px; }
  .bt { border-top: 1px solid var(--border); }
  .two { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .sub { font-size: 12px; color: var(--muted); }
  .hint { padding: 2px 4px 0; font-size: 12px; color: var(--muted); }
  .tf { height: 32px; box-sizing: border-box; padding: 0 10px; border: 1px solid var(--border); border-radius: 10px; background: var(--bg); color: var(--text); font-size: 12px; }
  .kbtn { padding: 2px; border: 0; border-radius: 8px; background: none; cursor: pointer; }
  .kbtn:hover { background: var(--surface-2); }
  .cap { font-size: 12px; color: var(--accent-text); padding: 0 6px; }
  .foot { display: flex; align-items: center; gap: 10px; padding: 22px 4px 0; }
</style>
