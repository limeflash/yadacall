<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import Switch from "../kit/Switch.svelte";
  import Mascot from "../kit/Mascot.svelte";
  import StatusDot from "../kit/StatusDot.svelte";
  import { api, integrations } from "../api";
  import type { AgendaEvent, CalendarAccount, DetectedCall, MeetingListItem } from "../api";
  import { app, elapsedMs, theme, levels, startRecording, stopRecording, togglePause, addBookmark, onIntegrationEvent } from "../state.svelte";
  import { t } from "../i18n/index.svelte";
  import { clock, hhmm, mss } from "../format";
  import { isMac, prettyAccel } from "../platform";
  import { visibleInterval } from "../visibility";

  let meetings = $state<MeetingListItem[]>([]);
  let micOn = $state(true), sysOn = $state(true);
  let root: HTMLElement;
  let bars = $state<number[]>(Array.from({ length: 40 }, () => 4));
  const hist: number[] = Array.from({ length: 40 }, () => 4);

  const recording = $derived(app.recorder.phase !== "idle");
  const paused = $derived(app.recorder.phase === "paused");
  const today = $derived(meetings.filter((m) => new Date(m.startedAt).toDateString() === new Date().toDateString()).slice(0, 3));
  const lastLive = $derived(app.live.at(-1));
  const hotkey = $derived(prettyAccel(app.settings?.hotkeyRecord ?? "Alt+Shift+R"));
  const busyModel = $derived(Object.keys(app.downloads)[0]);

  // Before a call: the next calendar call (when it is close) and a call the shell noticed that no calendar knows.
  let agenda = $state<AgendaEvent[]>([]);
  let accounts = $state<CalendarAccount[]>([]);
  let detected = $state<DetectedCall | null>(null);
  async function loadCalls() {
    const end = new Date(); end.setHours(24, 0, 0, 0);
    try {
      [agenda, accounts, detected] = await Promise.all([
        integrations.agenda({ from: new Date().toISOString(), to: end.toISOString() }), integrations.accounts(), integrations.detectedCall(),
      ]);
    } catch { /* integrations not ready */ }
  }
  const upcoming = $derived(agenda.filter((e) => Date.parse(e.end) > app.now));
  const nextCall = $derived(upcoming[0]);
  const left = $derived(nextCall ? Math.round((Date.parse(nextCall.start) - app.now) / 1000) : 0);
  /** The card replaces the idle popover from 15 minutes before the start until 5 minutes into the call. */
  const soon = $derived(!!nextCall && left <= 900 && left > -300);
  const nextAcc = $derived(accounts.find((a) => a.id === nextCall?.accountId));
  const cd = $derived(left < 0 ? `+${mss(-left)}` : mss(left));
  const accColor = (id: string) => accounts.find((a) => a.id === id)?.color;
  async function joinRec(e: AgendaEvent) { await integrations.joinAndRecord(e.id); await api.hidePopover("tray"); }
  async function joinOnly(e: AgendaEvent) { await integrations.joinWithoutRecording(e.id); await api.hidePopover("tray"); }
  async function answer(c: DetectedCall, record: boolean) { detected = null; await integrations.resolveDetectedCall(c.id, record); if (record) await api.hidePopover("tray"); }

  async function load() { meetings = await api.listMeetings(); }
  onMount(() => {
    load(); loadCalls();
    const off = onIntegrationEvent((e) => { if (e.type !== "addProgress") loadCalls(); });
    const poll = setInterval(loadCalls, 15000);
    const stopBars = visibleInterval(() => {
      if (!recording && hist.every((h) => h === 3)) return; // nothing moves while idle
      const lvl = Math.max(levels.mic.peak, levels.system.peak);
      hist.push(paused || !recording ? 3 : Math.round(4 + lvl * 20 + Math.random() * 6)); hist.shift();
      bars = [...hist];
    }, 90);
    const ro = new ResizeObserver(() => api.popoverFit("tray", Math.ceil(root.getBoundingClientRect().height) + 20));
    ro.observe(root);
    const onFocus = () => { load(); loadCalls(); };
    window.addEventListener("focus", onFocus);
    return () => { stopBars(); clearInterval(poll); off(); ro.disconnect(); window.removeEventListener("focus", onFocus); };
  });
  $effect(() => { void app.rev; load(); });

  async function openMain(hash: string) { await api.openInMain(hash); await api.hidePopover("tray"); }
  async function finish() { const id = await stopRecording(); await openMain(id ? `#/meeting/${id}/summary` : "#/"); }
  async function startNow() { await startRecording({}); }
  const stateOf = (m: MeetingListItem) => m.status.state;
</script>

{#snippet soonCard(e: AgendaEvent, win: boolean)}
  <div class="scard" class:win>
    <div style="display: flex; align-items: baseline; gap: 8px">
      <span style="flex: 1; font-size: 12px; color: var(--muted)">{t("Следующая встреча")}</span>
      <span class="mg-mono yc-num" style="font-size: 20px; font-weight: 600; letter-spacing: -0.04em; color: {left < 0 ? 'var(--danger)' : 'var(--accent-text)'}">{cd}</span>
    </div>
    <div style="display: flex; align-items: center; gap: 10px">
      <span aria-hidden="true" style="flex: none; width: 3px; height: 34px; border-radius: 2px; background: {nextAcc?.color}"></span>
      <span style="flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 1px">
        <span class="yc-ellipsis" style="font-size: 14px; font-weight: 700">{e.title}</span>
        <span class="yc-ellipsis" style="font-size: 12px; color: var(--muted)">{hhmm(e.start)} · {nextAcc?.label}</span>
      </span>
    </div>
    <div style="display: flex; gap: 6px">
      <button type="button" class="yc-press sj" onclick={() => joinRec(e)}><Icon name="video" size={14} />{t("Войти и записать")}</button>
      <button type="button" aria-label={t("Войти без записи")} class="yc-press sn" onclick={() => joinOnly(e)}>{t("Без записи")}</button>
    </div>
  </div>
{/snippet}

{#snippet detectedCard(c: DetectedCall)}
  <div class="scard dcard">
    <div style="display: flex; flex-direction: column; gap: 1px">
      <span style="font-size: 13px; font-weight: 700">{t("Ты в звонке {p}", { p: c.platform })}</span>
      <span style="font-size: 13px">{t("Записать его?")}</span>
      <span class="mg-mono yc-ellipsis" style="font-size: 11.5px; color: var(--muted)">{c.url}</span>
    </div>
    <div style="display: flex; gap: 6px">
      <button type="button" class="yc-press sj" style="flex: 1.2; justify-content: center" onclick={() => answer(c, true)}>{t("Записать")}</button>
      <button type="button" class="yc-press sn" onclick={() => answer(c, false)}>{t("Не сейчас")}</button>
    </div>
  </div>
{/snippet}

<div class="stage" class:win={!isMac}>
  <div bind:this={root} class="root">
    {#if recording}
      <div class="rec">
        <div style="display: flex; align-items: center; gap: 8px">
          <span class={paused ? "" : "yc-rec"} style="width: 7px; height: 7px; border-radius: 50%; background: {paused ? 'var(--warning)' : 'var(--danger)'}"></span>
          <span style="font-size: 12px; font-weight: 700">{paused ? t("Пауза") : t("Запись")}</span>
          <span style="flex: 1"></span>
          <span class="mg-mono yc-num" style="font-size: 22px; font-weight: 600; letter-spacing: -0.04em">{clock(elapsedMs())}</span>
        </div>
        <div style="display: flex; flex-direction: column; gap: 2px">
          <span class="yc-ellipsis" style="font-size: 14px; font-weight: 700">{app.recorder.title || t("Без названия")}</span>
          <span class="yc-ellipsis" style="font-size: 12px; color: var(--muted); min-height: 17px">{lastLive ? `${lastLive.speaker === "me" ? t("Вы") : t("Собеседник")} — ${lastLive.text}` : t("Текст появится, как только кто-то заговорит")}</span>
        </div>
        <div aria-hidden="true" class="wave">
          {#each bars as h, i (i)}<span class={i >= 33 && !paused ? "yc-live" : ""} style="height: {h}px; opacity: {(0.25 + (0.75 * i) / 39).toFixed(2)}; animation-delay: {(i * 37) % 500}ms"></span>{/each}
        </div>
        <div style="display: flex; gap: 8px">
          <button type="button" class="yc-press b" onclick={togglePause}>{paused ? t("Дальше") : t("Пауза")}</button>
          <button type="button" class="yc-press b" onclick={addBookmark}>{t("Закладка")}</button>
          <button type="button" class="yc-press b fin" onclick={finish}>{t("Завершить")}</button>
        </div>
      </div>
    {:else if isMac && soon && nextCall}
      <div class="idle">
        <div class="hdr" style="padding-bottom: 10px">
          <span style="display: block; width: 28px; height: 28px; flex: none"><Mascot size={28} accent={theme.accent} /></span>
          <span class="word">yada<span>call</span></span>
        </div>
        {#if detected}{@render detectedCard(detected)}{/if}
        {@render soonCard(nextCall, false)}
        {#if upcoming.length > 1}<div class="cap" style="padding: 12px 10px 4px">{t("Дальше сегодня")}</div>{/if}
        {#each upcoming.slice(1, 4) as m (m.id)}
          <button type="button" class="yc-mi li" onclick={() => openMain("#/calendar")}>
            <span class="mg-mono" style="flex: none; width: 38px; font-size: 11.5px; color: var(--muted); text-align: left">{hhmm(m.start)}</span>
            <span aria-hidden="true" style="flex: none; width: 6px; height: 6px; border-radius: 50%; background: {accColor(m.accountId)}"></span>
            <span class="yc-ellipsis" style="flex: 1; text-align: left">{m.title}</span>
          </button>
        {/each}
        <span aria-hidden="true" class="sep"></span>
        <button type="button" class="yc-mi li sm" onclick={() => openMain("#/calendar")}><span style="flex: 1; text-align: left">{t("Весь календарь")}</span><span class="mg-mono k">⌘K</span></button>
        <button type="button" class="yc-mi li sm" onclick={() => openMain("#/")}><span style="flex: 1; text-align: left">{t("Открыть YadaCall")}</span><span class="mg-mono k">⌘O</span></button>
        <button type="button" class="yc-mi li sm" onclick={() => api.quit()}><span style="flex: 1; text-align: left">{t("Выйти")}</span><span class="mg-mono k">⌘Q</span></button>
      </div>
    {:else if isMac}
      <div class="idle">
        <div class="hdr">
          <span style="display: block; width: 28px; height: 28px; flex: none"><Mascot size={28} accent={theme.accent} /></span>
          <span class="word">yada<span>call</span></span>
          <span style="display: inline-flex; align-items: center; gap: 6px; font-size: 12px; color: var(--muted)"><StatusDot kind={busyModel ? "busy" : "ok"} />{busyModel ? t("загрузка") : t("готов")}</span>
        </div>
        {#if detected}{@render detectedCard(detected)}{/if}
        <button type="button" class="yc-press recbtn" onclick={startNow}>
          <Icon name="mic" size={14} /><span style="flex: 1; text-align: left">{t("Записать встречу")}</span><span class="mg-mono hk">{hotkey}</span>
        </button>
        <div style="display: flex; flex-direction: column; padding: 8px 4px 4px">
          <label class="tg"><span class="tic"><Icon name="mic" size={14} /></span><span style="flex: 1">{t("Микрофон")} <span style="color: var(--muted)">· {app.recorder.micDevice ?? "MacBook Pro"}</span></span><Switch bind:checked={micOn} aria-label={t("Микрофон")} /></label>
          <label class="tg"><span class="tic"><Icon name="monitor" size={14} /></span><span style="flex: 1">{t("Звук собеседников")}</span><Switch bind:checked={sysOn} aria-label={t("Звук собеседников")} /></label>
        </div>
        <span aria-hidden="true" class="sep"></span>
        {#if today.length}<div class="cap">{t("Сегодня")}</div>{/if}
        {#each today as m (m.id)}
          <button type="button" class="yc-mi li" onclick={() => openMain(`#/meeting/${m.id}/summary`)}>
            <span class="yc-ellipsis" style="flex: 1; text-align: left">{m.title}</span>
            {#if stateOf(m) === "processing"}<span class="proc"><svg class="yc-ic yc-spin" width="10" height="10" viewBox="0 0 24 24" style="color: var(--busy); stroke-width: 3.4"><path d="M12 3a9 9 0 1 0 9 9" /></svg>{t("итоги")}</span>
            {:else}<span class="mg-mono" style="font-size: 11px; color: var(--muted)">{hhmm(m.startedAt)}</span>{/if}
          </button>
        {/each}
        {#if today.length}<span aria-hidden="true" class="sep"></span>{/if}
        <button type="button" class="yc-mi li sm" onclick={() => openMain("#/")}><span style="flex: 1; text-align: left">{t("Открыть YadaCall")}</span><span class="mg-mono k">⌘O</span></button>
        <button type="button" class="yc-mi li sm" onclick={() => openMain("#/settings")}><span style="flex: 1; text-align: left">{t("Настройки…")}</span><span class="mg-mono k">⌘,</span></button>
        <button type="button" class="yc-mi li sm" onclick={() => api.quit()}><span style="flex: 1; text-align: left">{t("Выйти")}</span><span class="mg-mono k">⌘Q</span></button>
      </div>
    {:else}
      <div class="win-card">
        <div class="whdr">
          <span style="display: block; width: 28px; height: 28px; flex: none"><Mascot size={28} accent={theme.accent} /></span>
          <span class="word" style="flex: 1">yada<span>call</span></span>
          <button type="button" aria-label={t("Настройки")} class="yc-nav ib" onclick={() => openMain("#/settings")}><Icon name="gear" size={16} /></button>
          <button type="button" aria-label={t("Открыть окно")} class="yc-nav ib" onclick={() => openMain("#/")}><Icon name="externalLink" size={16} /></button>
        </div>
        {#if detected || (soon && nextCall)}
          <div style="display: flex; flex-direction: column; gap: 8px; padding: 0 16px 12px">
            {#if detected}{@render detectedCard(detected)}{/if}
            {#if soon && nextCall}{@render soonCard(nextCall, true)}{/if}
          </div>
        {/if}
        <div style="display: flex; flex-direction: column; gap: 10px; padding: 0 16px 14px">
          <button type="button" class="yc-press recbtn big" onclick={startNow}><Icon name="mic" size={14} /><span style="flex: 1; text-align: left">{t("Записать встречу")}</span><span class="mg-mono hk" style="font-size: 10px">{hotkey}</span></button>
          <div style="display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px">
            <button type="button" aria-pressed={micOn} class="yc-press tile" style="border-color: {micOn ? 'var(--accent-line)' : 'var(--border)'}; background: {micOn ? 'var(--accent-soft)' : 'var(--bg)'}" onclick={() => (micOn = !micOn)}>
              <Icon name="mic" size={15} /><span style="display: flex; flex-direction: column"><span style="font-size: 13px; font-weight: 700">{t("Микрофон")}</span><span style="font-size: 11px; color: var(--muted)">{micOn ? `${app.recorder.micDevice ?? "Realtek"} · ${t("вкл")}` : t("выключен")}</span></span>
            </button>
            <button type="button" aria-pressed={sysOn} class="yc-press tile" style="border-color: {sysOn ? 'var(--accent-line)' : 'var(--border)'}; background: {sysOn ? 'var(--accent-soft)' : 'var(--bg)'}" onclick={() => (sysOn = !sysOn)}>
              <svg class="yc-ic" width="15" height="15" viewBox="0 0 24 24"><path d="M4 10v4h4l5 4V6l-5 4zM17 9a4 4 0 0 1 0 6" /></svg><span style="display: flex; flex-direction: column"><span style="font-size: 13px; font-weight: 700">{t("Собеседники")}</span><span style="font-size: 11px; color: var(--muted)">{sysOn ? t("звук системы · вкл") : t("выключены")}</span></span>
            </button>
          </div>
        </div>
        {#if today.length}
          <div style="display: flex; flex-direction: column; padding: 8px 6px; border-top: 1px solid var(--border)">
            <div style="padding: 2px 10px 4px; font-size: 12px; color: var(--muted)">{t("Сегодня")}</div>
            {#each today.slice(0, 2) as m (m.id)}
              <button type="button" class="yc-mi li" onclick={() => openMain(`#/meeting/${m.id}/summary`)}><span class="yc-ellipsis" style="flex: 1; text-align: left">{m.title}</span><span class="mg-mono" style="font-size: 11px; color: var(--muted)">{hhmm(m.startedAt)}</span></button>
            {/each}
          </div>
        {/if}
        <div class="wfoot"><StatusDot kind={busyModel ? "busy" : "ok"} /><span style="flex: 1">{t("Готов · расшифровка на компьютере")}</span><span class="mg-mono" style="font-size: 11px">{(app.settings && "turbo") || ""}</span></div>
      </div>
    {/if}
  </div>
</div>

<style>
  .stage { padding: 10px; display: flex; }
  .stage.win { align-items: flex-end; min-height: 100vh; box-sizing: border-box; }
  .root { width: 100%; }
  .idle, .rec, .win-card { width: 100%; box-sizing: border-box; display: flex; flex-direction: column; border: 1px solid var(--border); border-radius: 16px; background: var(--surface); box-shadow: var(--shadow-pop); overflow: hidden; }
  .idle { padding: 8px; }
  .rec { gap: 14px; padding: 16px; }
  .hdr { display: flex; align-items: center; gap: 8px; padding: 6px 8px 10px; }
  .word { flex: 1; font-size: 14px; font-weight: 800; letter-spacing: -0.03em; }
  .word span { color: var(--accent-text); }
  .recbtn { display: flex; align-items: center; gap: 8px; height: 40px; padding: 0 14px; border: 0; border-radius: 12px; background: var(--accent); color: var(--ink-on-accent); font-size: 14px; font-weight: 800; }
  .recbtn.big { height: 44px; border-radius: 14px; }
  .hk { font-size: 11px; font-weight: 600; opacity: 0.7; }
  .tg { display: flex; align-items: center; gap: 10px; height: 34px; padding: 0 6px; font-size: 13px; }
  .tic { display: inline-flex; color: var(--muted); }
  .sep { height: 1px; margin: 4px 6px; background: var(--border); display: block; }
  .cap { padding: 6px 10px 4px; font-size: 12px; color: var(--muted); }
  .li { display: flex; align-items: center; gap: 10px; height: 32px; padding: 0 10px; border: 0; border-radius: 9px; background: transparent; color: var(--text); font-size: 13px; }
  .li.sm { height: 30px; }
  .k { font-size: 11px; color: var(--muted); }
  .proc { display: inline-flex; align-items: center; gap: 5px; font-size: 11px; color: var(--muted); }
  .wave { height: 30px; display: flex; align-items: center; gap: 2px; }
  .wave span { display: block; flex: 1; border-radius: 2px; background: var(--accent); }
  .b { flex: 1; height: 36px; border: 1px solid var(--border); border-radius: 11px; background: var(--bg); color: var(--text); font-size: 13px; font-weight: 700; }
  .b.fin { flex: 1.3; border: 0; background: var(--accent); color: var(--ink-on-accent); font-weight: 800; }
  .whdr { display: flex; align-items: center; gap: 8px; padding: 12px 10px 10px 16px; }
  .ib { width: 30px; height: 30px; border: 0; border-radius: 9px; background: transparent; display: grid; place-items: center; color: var(--muted); }
  .tile { display: flex; flex-direction: column; align-items: flex-start; gap: 6px; padding: 10px 12px; border: 1px solid var(--border); border-radius: 12px; color: var(--text); text-align: left; }
  .scard { display: flex; flex-direction: column; gap: 12px; padding: 14px; border: 1px solid var(--accent-line); border-radius: 13px; background: var(--bg); }
  .scard.win { border-radius: 8px; }
  .dcard { margin: 0 0 6px; padding: 12px 14px 14px; border-color: var(--border); }
  .sj { flex: 1.6; height: 34px; display: flex; align-items: center; justify-content: center; gap: 7px; border: 0; border-radius: 10px; background: var(--accent); color: var(--ink-on-accent); font-size: 13px; font-weight: 800; }
  .sn { flex: 1; height: 34px; border: 1px solid var(--border); border-radius: 10px; background: transparent; color: var(--text); font-size: 12.5px; font-weight: 700; }
  .wfoot { display: flex; align-items: center; gap: 8px; padding: 10px 16px; border-top: 1px solid var(--border); background: var(--bg); font-size: 12px; color: var(--muted); }
</style>
