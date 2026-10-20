<script lang="ts">
  import { onMount } from "svelte";
  import Button from "../kit/Button.svelte";
  import Segmented from "../kit/Segmented.svelte";
  import Switch from "../kit/Switch.svelte";
  import Kbd from "../kit/Kbd.svelte";
  import SectionLabel from "../kit/SectionLabel.svelte";
  import AccentPicker from "../kit/AccentPicker.svelte";
  import Mascot from "../kit/Mascot.svelte";
  import StatusPill from "../kit/StatusPill.svelte";
  import SummarySection from "./SummarySection.svelte";
  import ModelPicker from "../components/ModelPicker.svelte";
  import { api, integrations } from "../api";
  import type { Connection, Permissions, ThemeChoice, KeepAudio, PermissionKind, BookmarkScreenshot } from "../api";
  import { app, saveSettings, theme, update, checkForUpdates, installUpdate } from "../state.svelte";
  import { t, plural, i18n } from "../i18n/index.svelte";
  import { go } from "../router.svelte";
  import Icon from "../kit/Icon.svelte";
  import { bytes } from "../format";
  import { prettyAccel, isMac } from "../platform";
  import { toast } from "../toast.svelte";

  const s = $derived(app.settings!);
  let perms = $state<Permissions | null>(null);
  let capturing = $state<null | "hotkeyRecord" | "hotkeyBookmark">(null);

  // ---- term dictionary ----------------------------------------------------------------------------
  let termDraft = $state("");
  function addTerms(raw: string) {
    const add = raw.split(/[,;\n]/).map((x) => x.trim()).filter(Boolean);
    termDraft = "";
    if (!add.length) return;
    const have = s.terms ?? [];
    const lower = new Set(have.map((x) => x.toLowerCase()));
    const fresh = add.filter((x, i) => !lower.has(x.toLowerCase()) && add.findIndex((y) => y.toLowerCase() === x.toLowerCase()) === i);
    if (fresh.length) saveSettings({ terms: [...have, ...fresh] });
  }
  function removeTerm(term: string) { saveSettings({ terms: (s.terms ?? []).filter((x) => x !== term) }); }

  let meet = $state<Connection | null>(null);
  onMount(() => {
    api.permissions().then((p) => (perms = p));
    integrations.connections().then((c) => (meet = c.find((x) => x.state === "connected") ?? null));
  });

  const L = { Bg: "#f8f8f6", Surf: "#ffffff", Line: "#e3e3df" };
  const D = { Bg: "#0c0c0e", Surf: "#1d1d22", Line: "#2c2c33" };
  const themeOpts: { id: ThemeChoice; label: string; A: typeof L; B: typeof L; clip: string }[] = [
    { id: "system", label: "Система", A: L, B: D, clip: "polygon(62% 0, 100% 0, 100% 100%, 38% 100%)" },
    { id: "light", label: "Светлая", A: L, B: L, clip: "none" },
    { id: "dark", label: "Тёмная", A: D, B: D, clip: "none" },
  ];
  const keepOptions = $derived<{ value: KeepAudio; label: string }[]>([{ value: "none", label: t("Не хранить") }, { value: "7d", label: t("7 дней") }, { value: "30d", label: t("30 дней") }, { value: "forever", label: t("Всегда") }]);
  const screenshotOptions = $derived<{ value: BookmarkScreenshot; label: string }[]>([
    { value: "off", label: t("Нет") },
    { value: "window", label: t("Активное окно") },
    { value: "screen", label: t("Экран с окном") },
  ]);

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
  async function askPerm(k: PermissionKind) { perms = await api.requestPermission(k); }
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

      <div style="padding: 14px 4px 0"><SectionLabel as="h2">{t("Подключения")}</SectionLabel></div>
      <button type="button" class="yc-lift conn" onclick={() => go({ name: "connections" })}>
        <span aria-hidden="true" class="gtile">G</span>
        <span class="two">
          <span style="font-size: 14px">Google Meet</span>
          <span class="sub">{#if meet?.accounts != null}{meet.accounts} {plural(meet.accounts, ["аккаунт", "аккаунта", "аккаунтов"], ["account", "accounts"])} · {/if}{t("встречи из календаря и вход в звонок")}</span>
        </span>
        <span style="flex: none; font-size: 12px; color: var(--muted)">{t("Все сервисы")}</span>
        <span style="display: inline-flex; color: var(--muted)"><Icon name="chevronRight" size={16} /></span>
      </button>

      <ModelPicker kind="asr" title={t("Модель расшифровки")} />

      <div class="group">
        <div class="grow">
          <span style="flex: 1; min-width: 0; font-size: 14px">{t("Язык встреч")}</span>
          <Segmented variant="inset" label={t("Язык встреч")} value={s.meetingLanguage} onchange={(v) => saveSettings({ meetingLanguage: v })} options={[{ value: "auto", label: t("Авто") }, { value: "ru", label: "RU" }, { value: "en", label: "EN" }]} />
        </div>
        <label class="grow bt">
          <span class="two"><span style="font-size: 14px">{t("Узнавать голоса")}</span><span class="sub">{t("Подписывает знакомых по прошлым встречам")}</span></span>
          <Switch checked={s.recognizeVoices} onchange={(v) => saveSettings({ recognizeVoices: v })} />
        </label>
        <div class="grow bt terms">
          <span class="two" style="flex: none"><span style="font-size: 14px">{t("Словарь терминов")}</span><span class="sub">{t("Названия продуктов, имена, жаргон — расшифровка поправит написание, например «Google Mit» → «Google Meet»")}</span></span>
          <div class="chips">
            {#each s.terms ?? [] as term (term)}
              <span class="chip">{term}<button type="button" aria-label={t("Убрать {term}", { term })} onclick={() => removeTerm(term)}><Icon name="x" size={10} /></button></span>
            {/each}
            <input class="tin" bind:value={termDraft} placeholder={t("Добавить термин")} aria-label={t("Добавить термин")}
              onkeydown={(e) => { if (e.key === "Enter" || e.key === ",") { e.preventDefault(); addTerms(termDraft); } else if (e.key === "Backspace" && !termDraft && (s.terms ?? []).length) removeTerm(s.terms[s.terms.length - 1]); }}
              onpaste={(e) => { const txt = e.clipboardData?.getData("text") ?? ""; if (/[,\n;]/.test(txt)) { e.preventDefault(); addTerms(txt); } }}
              onblur={() => addTerms(termDraft)} />
          </div>
        </div>
      </div>

      <SummarySection />

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
        <div class="grow bt" style="display: block">
          <span style="display: block; font-size: 14px; margin-bottom: 9px">{t("Снимок к закладке")}</span>
          <Segmented variant="thumb" label={t("Снимок к закладке")} value={s.bookmarkScreenshot} onchange={(v) => saveSettings({ bookmarkScreenshot: v })} options={screenshotOptions} />
          <div class="hint" style="padding-top: 7px">{t("Снимки хранятся только в папке встречи")}</div>
        </div>
      </div>

      {#if perms}
        <div style="padding: 14px 4px 0"><SectionLabel as="h2">{t("Разрешения")}</SectionLabel></div>
        <div class="group">
        {#each [["microphone", "Микрофон", "Ваш голос"], ["systemAudio", "Звук собеседников", isMac ? "macOS отдаёт его через разрешение «Запись экрана»" : "Системный звук"], ...(isMac ? [["screenRecording", "Запись экрана", "Нужно для снимков к закладкам"]] : [])] as [k, label, sub], i (k)}
            {@const st = perms[k as PermissionKind]}
            <div class="grow" class:bt={i > 0}>
              <span class="two" style="flex: 1"><span style="font-size: 14px">{t(label)}</span><span class="sub">{t(sub)}</span></span>
              {#if st === "granted"}<StatusPill kind="ok" label={t("Разрешено")} sm />
              {:else if st === "denied"}<Button size="sm" onclick={() => api.openSystemSettings(k as PermissionKind)}>{t("Открыть настройки")}</Button>
              {:else}<Button size="sm" onclick={() => askPerm(k as PermissionKind)}>{t("Разрешить")}</Button>{/if}
            </div>
          {/each}
        </div>
      {/if}

      <div class="foot">
        <div style="width: 28px; height: 28px; flex: none"><Mascot size={28} variant={theme.resolved === "dark" ? "white" : "black"} accent={theme.accent} /></div>
        <span class="mg-mono" style="font-size: 11px; color: var(--muted)">YadaCall {update.current} · MIT · {t("исходники на GitHub")} ·
          {#if update.phase === "checking"}{t("проверяю…")}
          {:else if update.phase === "none"}{t("последняя версия")}
          {:else if update.phase === "available" || update.phase === "installing"}<button type="button" class="uplink" onclick={() => (update.sheet = true)}>{t("доступна {v}", { v: update.version ?? "" })}</button>
          {:else}<button type="button" class="uplink" onclick={checkForUpdates}>{t("Проверить обновления")}</button>{/if}
          · <button type="button" class="uplink" onclick={() => api.revealLogs().catch((e) => toast.error(e))}>{t("Открыть логи")}</button>
          · <button type="button" class="uplink" onclick={() => go({ name: "onboarding" })}>{t("Пройти настройку заново")}</button>
        </span>
        {#if update.phase === "error" && update.error}<span class="sub" role="alert" style="color: var(--danger-text)">{update.error}</span>{/if}
      </div>
    </div>
  </div>

  {#if update.sheet && update.version}
    <div class="veil" role="presentation" onclick={() => { if (update.phase !== "installing") update.sheet = false; }}></div>
    <div class="yc-pop usheet" role="dialog" aria-label={t("Обновление")}>
      <h2>{t("Доступна версия {v}", { v: update.version })}</h2>
      {#if update.date}<p>{new Date(update.date).toLocaleDateString(i18n.lang === "en" ? "en-GB" : "ru-RU", { day: "numeric", month: "long", year: "numeric" })}</p>{/if}
      {#if update.notes}<div class="unotes yc-scroll">{update.notes}</div>{/if}
      {#if update.phase === "installing"}
        {@const pct = update.total ? Math.min(100, Math.floor((update.downloaded / update.total) * 100)) : null}
        <span aria-hidden="true" class="ubar"><span style="width: {pct ?? 100}%; {pct === null ? 'opacity: 0.4' : ''}"></span></span>
        <span class="sub mg-mono">{#if update.total && update.downloaded < update.total}{t("Скачиваю")} · {bytes(update.downloaded)} {t("из")} {bytes(update.total)}{:else}{t("Устанавливаю и перезапускаю…")}{/if}</span>
      {:else}
        {#if update.error}<span class="sub" role="alert" style="color: var(--danger-text)">{update.error}</span>{/if}
        <div class="ubtns"><Button size="md" variant="ghost" onclick={() => (update.sheet = false)}>{t("Позже")}</Button><Button size="md" variant="primary" onclick={installUpdate}>{t("Обновить")}</Button></div>
      {/if}
    </div>
  {/if}
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
  .conn { display: flex; align-items: center; gap: 12px; padding: 12px 14px; border: 1px solid var(--border); border-radius: 16px; background: var(--surface); color: var(--text); text-align: left; font: inherit; }
  .gtile { width: 32px; height: 32px; flex: none; border-radius: 10px; background: var(--accent-soft); color: var(--accent-text); font-size: 14px; font-weight: 800; display: grid; place-items: center; }
  .group { border: 1px solid var(--border); border-radius: 16px; background: var(--surface); }
  .grow { display: flex; align-items: center; gap: 16px; padding: 12px 16px; }
  .bt { border-top: 1px solid var(--border); }
  .two { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .sub { font-size: 12px; color: var(--muted); }
  .hint { padding: 2px 4px 0; font-size: 12px; color: var(--muted); }
  .kbtn { padding: 2px; border: 0; border-radius: 8px; background: none; cursor: pointer; }
  .kbtn:hover { background: var(--surface-2); }
  .cap { font-size: 12px; color: var(--accent-text); padding: 0 6px; }
  .foot { flex-wrap: wrap; }
  .terms { flex-direction: column; align-items: stretch; gap: 8px; }
  .chips { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; }
  .chip { display: inline-flex; align-items: center; gap: 4px; padding: 3px 4px 3px 10px; border: 1px solid var(--border); border-radius: 999px; background: var(--surface-2); font-size: 12px; font-weight: 600; }
  .chip button { display: grid; place-items: center; width: 16px; height: 16px; padding: 0; border: 0; border-radius: 50%; background: none; color: var(--muted); }
  .chip button:hover { color: var(--text); background: var(--surface); }
  .tin { flex: 1; min-width: 120px; height: 26px; box-sizing: border-box; padding: 0 6px; border: 0; background: transparent; color: var(--text); font: inherit; font-size: 12px; outline: none; }
  .uplink { padding: 0; border: 0; background: none; font: inherit; color: var(--accent-text); text-decoration: underline; }
  .veil { position: fixed; inset: 0; z-index: 6; background: var(--backdrop); }
  .usheet { position: fixed; z-index: 7; left: 50%; top: 18vh; transform: translateX(-50%); width: min(400px, calc(100% - 32px)); box-sizing: border-box; padding: 18px; border: 1px solid var(--border); border-radius: 18px; background: var(--surface); box-shadow: var(--shadow-pop); display: flex; flex-direction: column; gap: 10px; }
  .usheet h2 { margin: 0; font-size: 16px; font-weight: 800; letter-spacing: -0.02em; }
  .usheet p { margin: 0; font-size: 12px; color: var(--muted); }
  .unotes { max-height: 220px; overflow: auto; white-space: pre-wrap; font-size: 13px; line-height: 1.5; padding: 10px 12px; border-radius: 12px; background: var(--bg); }
  .ubar { display: block; height: 6px; border-radius: 3px; background: var(--surface-2); overflow: hidden; }
  .ubar span { display: block; height: 100%; background: var(--accent); transition: width 0.2s; }
  .ubtns { display: flex; justify-content: flex-end; gap: 8px; margin-top: 4px; }
  .foot { display: flex; align-items: center; gap: 10px; padding: 22px 4px 0; }
</style>
