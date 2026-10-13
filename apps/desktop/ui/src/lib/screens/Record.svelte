<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import IconButton from "../kit/IconButton.svelte";
  import Stepper from "../kit/Stepper.svelte";
  import Select from "../kit/Select.svelte";
  import Mascot from "../kit/Mascot.svelte";
  import { api } from "../api";
  import type { InputDevice } from "../api";
  import { app, theme, elapsedMs, startRecording, stopRecording, togglePause, addBookmark, levels, onEngineEvent } from "../state.svelte";
  import { t } from "../i18n/index.svelte";
  import { clock } from "../format";
  import { go } from "../router.svelte";
  import { toast } from "../toast.svelte";
  import { Visualizer, type VisState } from "../visualizer";
  import { prettyAccel } from "../platform";
  import { visibleInterval } from "../visibility";

  const PLACEHOLDERS = ["Как назовём встречу?", "Синк по онбордингу", "Интервью с кандидатом", "Один на один с Аней", "Звонок с подрядчиком"];
  const SPK = ["авто", "1", "2", "3+"];

  let orbCanvas: HTMLCanvasElement, bgCanvas: HTMLCanvasElement;
  let vis: Visualizer | null = null;
  let hover = $state(false);
  let title = $state("");
  let ph = $state(0);
  let spk = $state(0);
  let mic = $state("builtin");
  let devices = $state<InputDevice[]>([]);
  let micOn = $state(true), sysOn = $state(true);
  let tip = $state(!(typeof localStorage !== "undefined" && localStorage.getItem("yc.tip.headphones") === "0"));
  let finished = $state<{ id: string; title: string; time: string } | null>(null);
  let job = $state(0);
  let done = $state(false);
  let copied = $state(false);

  const recording = $derived(app.recorder.phase !== "idle");
  const paused = $derived(app.recorder.phase === "paused");
  const phase = $derived<VisState["phase"]>(recording ? "rec" : finished ? (done ? "done" : "proc") : "idle");
  const sec = $derived(recording ? Math.floor(elapsedMs() / 1000) : 0);
  const timer = $derived(recording ? clock(elapsedMs()) : "00:00");
  const placeholder = $derived(PLACEHOLDERS[ph % PLACEHOLDERS.length]);

  // live-level meter on the two chips (4 bars each)
  let meBars = $state([0, 0, 0, 0]), themBars = $state([0, 0, 0, 0]);
  const barsFor = (v: number) => [0, 1, 2, 3].map((j) => (v > (j + 0.5) / 4 ? 1 : 0));

  onMount(() => {
    api.listInputDevices().then((d) => { devices = d; mic = d.find((x) => x.isDefault)?.id ?? d[0]?.id ?? ""; });
    vis = new Visualizer(orbCanvas, bgCanvas, () => ({ phase, paused, job, hover }), () => ({ acc: /^#[0-9a-f]{6}$/i.test(theme.accent) ? theme.accent : "#7dd3a0", dark: theme.resolved === "dark" }));
    vis.start();
    const iv = visibleInterval(() => {
      const live = recording && !paused;
      meBars = barsFor(live && micOn ? levels.mic.peak : 0);
      themBars = barsFor(live && sysOn ? levels.system.peak : 0);
    }, 80);
    const phIv = setInterval(() => { if (!title && phase === "idle") ph++; }, 2600);
    const off = onEngineEvent((e) => {
      if (e.type === "status" && finished && e.meeting === finished.id) {
        if (e.status.state === "processing") job = e.status.progress * 100;
        if (e.status.state === "ready") { job = 100; if (!done) { done = true; vis?.burst(); } }
      }
    });
    return () => { iv(); clearInterval(phIv); off(); };
  });
  onDestroy(() => vis?.stop());

  // A recording that is already running when this screen opens shows its own title.
  $effect(() => { if (recording && app.recorder.title && !title) title = app.recorder.title; });

  async function start() {
    finished = null; done = false; job = 0;
    await startRecording({ title: title.trim() || undefined, expectedSpeakers: spk === 0 ? undefined : spk, micDevice: mic || undefined });
  }
  async function stop() {
    const name = title.trim() || app.recorder.title || t("Без названия");
    const time = clock(elapsedMs());
    const id = await stopRecording();
    if (id) { finished = { id, title: `${name} · ${time}`, time }; job = 0; done = false; title = ""; }
  }
  function orbClick() { if (phase === "idle" || phase === "done") start(); }
  function toggleRec() { if (phase === "proc") return; if (!recording) start(); else stop(); }

  async function copyResult() {
    if (!finished) return;
    try { await navigator.clipboard.writeText(await api.copyForClaude(finished.id)); copied = true; setTimeout(() => (copied = false), 2400); }
    catch { toast.error(t("Не удалось скопировать — скопируй вручную")); }
  }
  function hideTip() { tip = false; try { localStorage.setItem("yc.tip.headphones", "0"); } catch { /* ignore */ } }
  function setTitle() { /* live-rename of a running meeting is applied on stop */ }

  const live = $derived(app.live.slice(-3));
  const speakerWord = (s: string) => (s === "me" ? t("Я") : t("Собеседник"));
  const statusLine = $derived(recording ? (paused ? t("Пауза — звук не пишется") : t("Идёт запись · всё остаётся на компьютере")) : phase === "proc" ? t("Распознаю локально") : phase === "done" ? t("Готово — можно сразу начать новую запись") : t("Нажми на котика или {hotkey} из любого окна", { hotkey: prettyAccel(app.settings?.hotkeyRecord ?? "Alt+Shift+R") }));
  const procInk = $derived(theme.resolved === "dark" ? "#f3f3f5" : "#16161a");
  const glow = $derived(`color-mix(in oklch, ${theme.accent} ${theme.resolved === "dark" ? 22 : 30}%, transparent)`);
  const fmtMark = (ms: number) => clock(ms);
</script>

<svelte:window onkeydown={(e) => { if ((e.altKey && e.shiftKey && e.code === "KeyM") && recording) { e.preventDefault(); addBookmark(); } }} />

<main>
  <div aria-hidden="true" class="glow" style="background: radial-gradient(closest-side, {glow}, transparent)"></div>
  <canvas bind:this={bgCanvas} aria-hidden="true" class="bg"></canvas>

  <div class="yc-scroll" style="position: relative; flex: 1; min-height: 0">
    <div class="col">
      <div class="orb">
        <canvas bind:this={orbCanvas} aria-hidden="true" style="position: absolute; inset: 0; width: 250px; height: 250px"></canvas>
        <button type="button" class="orb-btn" style="cursor: {recording || phase === 'proc' ? 'default' : 'pointer'}"
          aria-label={recording ? t("Идёт запись") : phase === "proc" ? t("Распознаю") : t("Начать запись")}
          onclick={orbClick} onmouseenter={() => (hover = true)} onmouseleave={() => (hover = false)}>
          {#if phase === "idle"}
            <span style="position: relative; display: block; width: 128px; height: 128px">
              <span class="yc-float" style="position: relative; display: block; width: 100%; height: 100%">
                <span class="cat" style="transform: {hover ? 'scale(.5)' : 'scale(1)'}; opacity: {hover ? 0 : 1}; filter: blur({hover ? 4 : 0}px)">
                  <Mascot size={128} accent={theme.accent} />
                </span>
                <span class="rec-btn" style="box-shadow: 0 14px 34px -10px var(--accent), 0 0 0 {hover ? 10 : 0}px color-mix(in oklch, var(--accent) 22%, transparent); transform: {hover ? 'scale(1)' : 'scale(.4)'}; opacity: {hover ? 1 : 0}">
                  <span style="display: block; width: {hover ? 30 : 0}px; height: {hover ? 30 : 0}px; border-radius: 50%; background: var(--ink-on-accent); transition: width 0.6s cubic-bezier(0.34, 1.6, 0.5, 1) 0.3s, height 0.6s cubic-bezier(0.34, 1.6, 0.5, 1) 0.3s"></span>
                </span>
              </span>
            </span>
          {:else if phase === "rec"}
            <span class="yc-popin" style="display: grid; place-items: center">
              {#if !paused}<Icon name="mic" size={26} style="color: var(--accent)" />{:else}<Icon name="pause" size={22} style="color: var(--warning); stroke-width: 3" />{/if}
            </span>
          {:else if phase === "proc"}
            <span class="yc-popin" style="display: flex; flex-direction: column; align-items: center; gap: 1px">
              <span class="mg-mono yc-num" style="font-size: 24px; font-weight: 700; color: {procInk}">{Math.floor(job)}%</span>
              <span style="font-size: 11px; font-weight: 700; color: {procInk}; opacity: 0.7">{t("распознаю")}</span>
            </span>
          {:else}
            <span class="yc-popin" style="display: flex; flex-direction: column; align-items: center; gap: 3px">
              <span style="display: block; width: 11px; height: 22px; margin: -6px 0 4px; border-right: 4px solid var(--ink-on-accent); border-bottom: 4px solid var(--ink-on-accent); border-radius: 1px; transform: rotate(45deg)"></span>
              <span style="font-size: 11px; font-weight: 700; opacity: 0.75">{t("ещё раз?")}</span>
            </span>
          {/if}
        </button>
      </div>

      <div class="timer-wrap">
        <div class="mg-mono yc-num timer" style="color: {recording ? 'var(--text)' : 'var(--faint)'}">{timer}</div>
        <div class="status">
          {#if recording}<span class={paused ? "" : "yc-rec"} style="width: 6px; height: 6px; border-radius: 50%; background: {paused ? 'var(--warning)' : 'var(--danger)'}"></span>{/if}
          {statusLine}
        </div>
      </div>

      {#if recording}
        <div class="yc-in rec-col">
          <label style="display: flex; width: 100%">
            <span class="mg-sr">{t("Название встречи")}</span>
            <input class="live-title" type="text" bind:value={title} onchange={setTitle} placeholder={t("Без названия")} />
          </label>

          <div class="chips">
            <button type="button" onclick={() => (micOn = !micOn)} aria-pressed={micOn} class="yc-chipbtn chip" style="color: {micOn ? 'var(--text)' : 'var(--muted)'}">
              <Icon name="mic" size={13} />{t("Микрофон")}
              <span aria-hidden="true" class="lv">{#each meBars as on, j (j)}<span style="height: {4 + j * 2}px; background: {on ? 'var(--ok)' : 'var(--faint)'}"></span>{/each}</span>
            </button>
            <button type="button" onclick={() => (sysOn = !sysOn)} aria-pressed={sysOn} class="yc-chipbtn chip" style="color: {sysOn ? 'var(--text)' : 'var(--muted)'}">
              <Icon name="monitor" size={13} />{t("Звук {app}", { app: app.recorder.systemSource ?? "Zoom" })}
              <span aria-hidden="true" class="lv">{#each themBars as on, j (j)}<span style="height: {4 + j * 2}px; background: {on ? 'var(--ok)' : 'var(--faint)'}"></span>{/each}</span>
            </button>
          </div>

          <div class="controls">
            <div class="ctl">
              <button type="button" class="yc-press round" onclick={togglePause} aria-label={paused ? t("Дальше") : t("Пауза")}>
                {#if !paused}<Icon name="pause" size={16} style="stroke-width: 3" />{:else}<svg class="yc-icf" width="15" height="15" viewBox="0 0 24 24" style="margin-left: 2px"><path d="M7 4.5v15l12.5-7.5z" /></svg>{/if}
              </button>
              <span class="cap">{paused ? t("Дальше") : t("Пауза")}</span>
            </div>
            <div class="ctl">
              <button type="button" class="yc-press stop" onclick={toggleRec} aria-label={t("Завершить и собрать итоги")}>
                <svg class="yc-icf" width="18" height="18" viewBox="0 0 24 24"><rect x="5" y="5" width="14" height="14" rx="3" /></svg>
              </button>
              <span class="cap">{t("Завершить")}</span>
            </div>
            <div class="ctl">
              <button type="button" class="yc-press round" onclick={addBookmark} aria-label={t("Поставить закладку")} title="{t('Отметить важное место')} · {prettyAccel(app.settings?.hotkeyBookmark ?? 'Alt+Shift+M')}">
                <Icon name="bookmark" size={16} />
              </button>
              <span class="cap">{t("Закладка")}</span>
            </div>
          </div>

          {#if app.recorder.bookmarks.length}
            <div class="marks">
              {#each app.recorder.bookmarks as ms, i (i)}
                <span class="yc-popin mk"><Icon name="bookmark" size={10} /><span class="mg-mono">{fmtMark(ms)}</span></span>
              {/each}
            </div>
          {/if}

          {#if app.silentTrack === "system"}
            <div class="yc-in warn"><span class="yc-rec" style="width: 8px; height: 8px; flex: none; border-radius: 50%; background: var(--warning)"></span><span style="font-size: 12px; line-height: 1.35"><b>{t("Собеседника не слышно.")}</b> {t("Проверь, что звонок идёт через этот компьютер.")}</span></div>
          {/if}

          <section aria-label={t("Расшифровка на лету")} class="live">
            {#if live.length}
              {#each live as s, i (s.startMs + s.speaker + i)}
                {@const last = i === live.length - 1}
                <p class:yc-caret={last} style="margin: 0; font-size: 13px; line-height: 1.5; text-align: center; color: {last ? 'var(--text)' : 'var(--muted)'}; opacity: {last ? 1 : i === 0 && live.length === 3 ? 0.6 : 1}"><span style="font-weight: 700">{speakerWord(s.speaker)}</span> — {s.text}</p>
              {/each}
            {:else}
              <p class="yc-caret" style="margin: 0; font-size: 13px; line-height: 1.5; text-align: center; color: var(--muted)">{t("Текст появится, как только кто-то заговорит")}</p>
            {/if}
          </section>
        </div>
      {/if}

      {#if phase === "done" && finished}
        <div class="yc-in done">
          <div style="display: flex; flex-direction: column; gap: 2px; min-width: 0">
            <span style="font-size: 14px; font-weight: 800">{t("Расшифровка и итоги готовы")}</span>
            <span class="yc-ellipsis" style="font-size: 12px; color: var(--muted)">{finished.title}</span>
          </div>
          <div style="display: flex; gap: 6px">
            <button type="button" class="yc-press open" onclick={() => finished && go({ name: "meeting", id: finished.id, tab: "summary" })}>{t("Открыть")}</button>
            <button type="button" onclick={copyResult} class="yc-press copy">{copied ? t("Скопировано") : t("Скопировать")}</button>
          </div>
        </div>
      {/if}

      {#if phase === "idle"}
        <div class="yc-in" style="display: flex; flex-direction: column; gap: 8px">
          <label style="display: flex"><span class="mg-sr">{t("Название встречи")}</span>
            <input class="name-in" type="text" bind:value={title} placeholder={t(placeholder)} /></label>
          <div class="form">
            <div class="frow">
              <span style="flex: 1; min-width: 0; display: flex; flex-direction: column"><span style="font-size: 13px; font-weight: 600">{t("Собеседники")}</span><span style="font-size: 11px; color: var(--muted)">{t("для разметки по голосам")}</span></span>
              <div style="flex: none; width: 128px">
                <Stepper decLabel={t("Меньше")} incLabel={t("Больше")} decDisabled={spk === 0} incDisabled={spk === SPK.length - 1} ondec={() => (spk = Math.max(0, spk - 1))} oninc={() => (spk = Math.min(SPK.length - 1, spk + 1))}>{spk === 0 ? t("Авто") : SPK[spk]}</Stepper>
              </div>
            </div>
            <div class="frow" style="border-top: 1px solid var(--border)">
              <span style="flex: 1; min-width: 0; font-size: 13px; font-weight: 600">{t("Микрофон")}</span>
              <div style="flex: none; width: 176px">
                <Select compact label={t("Микрофон")} value={mic} onchange={(v) => (mic = v)} options={devices.map((d) => ({ value: d.id, label: d.name, hint: d.isDefault ? t("по умолчанию") : undefined }))} />
              </div>
            </div>
          </div>
        </div>
      {/if}

      {#if phase === "idle" && tip}
        <div class="yc-in tip">
          <span aria-hidden="true" class="yc-float tip-ic"><svg class="yc-ic" width="18" height="18" viewBox="0 0 24 24"><path d="M3 18v-6a9 9 0 0 1 18 0v6M21 19a2 2 0 0 1-2 2h-1a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2h3zM3 19a2 2 0 0 0 2 2h1a2 2 0 0 0 2-2v-3a2 2 0 0 0-2-2H3z" /></svg></span>
          <span style="flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 1px">
            <span style="font-size: 13px; font-weight: 800">{t("Лучше в наушниках")}</span>
            <span style="font-size: 12px; line-height: 1.35; color: var(--muted)">{t("Так твой голос и собеседник не смешаются")}</span>
          </span>
          <IconButton variant="close" aria-label={t("Скрыть подсказку")} onclick={hideTip}><Icon name="x" size={12} /></IconButton>
        </div>
      {/if}
    </div>
  </div>
</main>

<style>
  main { position: relative; flex: 1 1 520px; min-width: 0; display: flex; flex-direction: column; min-height: 0; overflow: hidden; }
  .glow { position: absolute; left: 50%; top: -170px; width: 720px; height: 520px; margin-left: -360px; pointer-events: none; }
  .bg { position: absolute; left: 0; top: 0; width: 100%; height: 520px; pointer-events: none; -webkit-mask-image: linear-gradient(#000 45%, transparent 80%); mask-image: linear-gradient(#000 45%, transparent 80%); }
  .col { max-width: 440px; box-sizing: border-box; margin: 0 auto; padding: 12px 24px 20px; display: flex; flex-direction: column; align-items: stretch; gap: 12px; }
  .orb { position: relative; width: 250px; height: 250px; align-self: center; flex: none; margin: -25px 0; transform: scale(0.8); }
  .orb-btn { position: absolute; left: 50%; top: 50%; width: 132px; height: 132px; margin: -66px 0 0 -66px; padding: 0; border: 0; border-radius: 50%; background: transparent; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 6px; color: var(--ink-on-accent); }
  .cat { position: absolute; inset: 0; display: block; transition: transform 0.7s cubic-bezier(0.65, 0, 0.35, 1), opacity 0.55s ease, filter 0.55s ease; }
  .rec-btn { position: absolute; left: 14px; top: 14px; width: 100px; height: 100px; border-radius: 50%; background: radial-gradient(circle at 35% 30%, color-mix(in oklch, var(--accent) 70%, #fff), var(--accent) 70%); display: flex; align-items: center; justify-content: center; transition: transform 0.7s cubic-bezier(0.34, 1.45, 0.5, 1) 0.1s, opacity 0.45s ease 0.1s, box-shadow 0.8s ease 0.25s; }
  .timer-wrap { display: flex; flex-direction: column; align-items: center; gap: 2px; margin-top: -2px; }
  .timer { font-size: 32px; font-weight: 600; letter-spacing: -0.04em; transition: color 0.4s; }
  .status { display: inline-flex; align-items: center; gap: 7px; font-size: 12px; color: var(--muted); text-align: center; }
  .rec-col { display: flex; flex-direction: column; align-items: center; gap: 12px; }
  .live-title { flex: 1; min-width: 0; margin: 0; padding: 4px 0; border: 0; background: transparent; color: var(--text); font: inherit; font-size: 16px; font-weight: 700; letter-spacing: -0.02em; text-align: center; outline-offset: 4px; }
  .chips { display: flex; flex-wrap: wrap; justify-content: center; gap: 6px; margin-top: -8px; }
  .chip { display: inline-flex; align-items: center; gap: 7px; height: 28px; padding: 0 11px; border: 1px solid var(--border); border-radius: 14px; background: var(--surface); font-size: 12px; }
  .lv { display: inline-flex; align-items: flex-end; gap: 2px; height: 10px; }
  .lv span { display: block; width: 2px; border-radius: 1px; transition: background-color 0.15s; }
  .controls { display: flex; align-items: flex-start; justify-content: center; gap: 28px; }
  .ctl { display: flex; flex-direction: column; align-items: center; gap: 6px; }
  .cap { font-size: 11px; color: var(--muted); }
  .round { width: 44px; height: 44px; border: 1px solid var(--border); border-radius: 50%; background: var(--surface); color: var(--text); display: grid; place-items: center; }
  .stop { width: 56px; height: 56px; border: 0; border-radius: 50%; background: var(--accent); color: var(--ink-on-accent); display: grid; place-items: center; box-shadow: 0 10px 26px -12px var(--accent); }
  .marks { display: flex; flex-wrap: wrap; justify-content: center; gap: 6px; margin-top: -6px; }
  .mk { display: inline-flex; align-items: center; gap: 5px; height: 24px; padding: 0 10px; border-radius: 12px; background: var(--tone-sand-bg); color: var(--tone-sand); font-size: 12px; }
  .warn { width: 100%; box-sizing: border-box; display: flex; align-items: center; gap: 10px; padding: 10px 12px; border-radius: 14px; background: var(--warning-soft); border: 1px solid var(--warning-line); }
  .live { width: 100%; height: 84px; box-sizing: border-box; overflow: hidden; display: flex; flex-direction: column; justify-content: flex-end; gap: 6px; padding-top: 4px; -webkit-mask-image: linear-gradient(transparent, #000 55%); mask-image: linear-gradient(transparent, #000 55%); }
  .done { display: flex; flex-direction: column; gap: 12px; padding: 14px; border-radius: 16px; background: var(--accent-soft); border: 1px solid var(--accent-line); }
  .open { flex: 1; height: 36px; display: grid; place-items: center; border: 0; border-radius: 11px; background: var(--accent); color: var(--ink-on-accent); font-size: 13px; font-weight: 800; }
  .copy { flex: 1.3; height: 36px; border: 1px solid var(--border); border-radius: 11px; background: var(--surface); color: var(--text); font-size: 13px; font-weight: 700; }
  .name-in { width: 100%; height: 44px; box-sizing: border-box; margin: 0; padding: 0 14px; border: 1px solid var(--border); border-radius: 14px; background: var(--surface); color: var(--text); font: inherit; font-size: 14px; font-weight: 500; }
  .form { border: 1px solid var(--border); border-radius: 14px; background: var(--surface); }
  .frow { display: flex; align-items: center; gap: 12px; min-height: 52px; padding: 8px 10px 8px 14px; box-sizing: border-box; }
  .tip { position: relative; overflow: hidden; display: flex; align-items: center; gap: 12px; padding: 10px 8px 10px 10px; border-radius: 16px; background: var(--accent-soft); border: 1px solid var(--accent-line); }
  .tip-ic { width: 36px; height: 36px; flex: none; border-radius: 11px; background: var(--accent); color: var(--ink-on-accent); display: grid; place-items: center; }
</style>
