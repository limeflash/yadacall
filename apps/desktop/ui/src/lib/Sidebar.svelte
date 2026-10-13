<script lang="ts">
  import Icon from "./kit/Icon.svelte";
  import Mascot from "./kit/Mascot.svelte";
  import { t } from "./i18n/index.svelte";
  import { visibleInterval } from "./visibility";
  import { nav, go } from "./router.svelte";
  import { app, theme, elapsedMs, stopRecording, levels } from "./state.svelte";
  import { clock, pct } from "./format";
  import { api } from "./api";
  import { overlayTitleBar } from "./platform";

  const route = $derived(nav.route.name);
  const recording = $derived(app.recorder.phase !== "idle");
  const paused = $derived(app.recorder.phase === "paused");
  const timer = $derived(clock(elapsedMs()));
  const active = $derived(Object.entries(app.downloads).find(([, d]) => d.state === "running"));
  const dlPct = $derived(active ? Math.min(100, Math.floor((active[1].done / Math.max(1, active[1].total)) * 100)) : 0);

  // Five bars that breathe with the input level (design: `islandBars`).
  let bars = $state([3, 3, 3, 3, 3]);
  $effect(() => {
    if (!recording) return;
    return visibleInterval(() => {
      const lvl = Math.max(levels.mic.peak, levels.system.peak);
      bars = bars.map((_, i) => paused ? 3 : 3 + Math.round(Math.min(1, lvl * (0.6 + 0.4 * Math.abs(Math.sin(performance.now() / 180 + i * 1.3)))) * 9));
    }, 100);
  });

  function recClick() {
    if (route !== "record") go({ name: "record" });
    else if (recording) stopRecording();
  }
</script>

<aside class="yc-side" aria-label={t("Боковое меню")} class:mac={overlayTitleBar}>
  <div class="brand">
    <div class="mark"><Mascot size={30} accent={theme.accent} /></div>
    <span class="word">yada<span>call</span></span>
  </div>

  <nav aria-label={t("Разделы")}>
    <button type="button" class="yc-nav item" class:on={route === "meetings" || route === "meeting" || route === "ask" || route === "calendar"} aria-current={route === "meetings" ? "page" : undefined} onclick={() => go({ name: "meetings" })}>
      <span class="ic"><Icon name="calendar" size={16} /></span><span class="lbl">{t("Встречи")}</span>
    </button>
    <button type="button" class="yc-nav item" class:on={route === "tasks"} aria-current={route === "tasks" ? "page" : undefined} onclick={() => go({ name: "tasks" })}>
      <span class="ic"><Icon name="list" size={16} /></span><span class="lbl">{t("Задачи")}</span>
      {#if app.openTasks > 0}<span class="mg-mono badge">{app.openTasks}</span>{/if}
    </button>
    <button type="button" class="yc-nav item" class:on={route === "upload"} aria-current={route === "upload" ? "page" : undefined} onclick={() => go({ name: "upload" })}>
      <span class="ic"><Icon name="upload" size={16} /></span><span class="lbl">{t("Загрузить запись")}</span>
    </button>
    <button type="button" class="yc-nav item" class:on={route === "settings" || route === "models" || route === "connections" || route === "google"} aria-current={route === "settings" ? "page" : undefined} onclick={() => go({ name: "settings" })}>
      <span class="ic"><Icon name="gear" size={16} /></span><span class="lbl">{t("Настройки")}</span>
    </button>
  </nav>

  <div class="bottom">
    {#if active}
      <button type="button" class="yc-in yc-nav dl" onclick={() => go({ name: "models" })}>
        <span class="dl-top">
          <span class="dl-ic"><svg class="yc-ic" width="13" height="13" viewBox="0 0 24 24"><path d="M12 4v11M7 10l5 5 5-5M5 20h14" /></svg></span>
          <span class="yc-ellipsis dl-name">{t("Качаю {name}", { name: active[0] })}</span>
          <span class="mg-mono yc-num dl-pct">{dlPct}%</span>
        </span>
        <span aria-hidden="true" class="dl-bar"><span style="width: {dlPct}%"></span></span>
      </button>
    {/if}
    {#if recording}
      <button type="button" class="yc-popin yc-press stop" aria-label={route === "record" ? t("Остановить запись") : t("Идёт запись, {time} — вернуться к записи", { time: timer })} onclick={recClick}>
        <span class={paused ? "" : "yc-rec"} style="width: 7px; height: 7px; border-radius: 50%; background: var(--ink-on-accent)"></span>
        <span class="mg-mono yc-num" style="font-size: 13px; font-weight: 700">{timer}</span>
        <span aria-hidden="true" class="eq">{#each bars as h, i (i)}<span style="height: {h}px"></span>{/each}</span>
        <span style="font-size: 13px; font-weight: 800">{route === "record" ? t("Стоп") : t("Запись")}</span>
      </button>
    {:else}
      <button type="button" class="yc-press rec" onclick={recClick}>
        <Icon name="mic" size={15} />{t("Записать встречу")}
      </button>
    {/if}
  </div>
</aside>

<style>
  .yc-side { position: relative; z-index: 1; flex: 0 0 208px; max-width: 208px; box-sizing: border-box; display: flex; flex-direction: column; gap: 18px; padding: 16px 10px 14px; border-right: 1px solid var(--border); background: var(--bg); min-height: 0; }
  .yc-side.mac { padding-top: 40px; }
  .brand { display: flex; align-items: center; gap: 8px; height: 32px; padding: 0 4px; }
  .mark { width: 30px; height: 30px; flex: none; }
  .word { font-size: 15px; font-weight: 800; letter-spacing: -0.03em; }
  .word span { color: var(--accent-text); }
  nav { display: flex; flex-direction: column; gap: 2px; }
  .item { display: flex; align-items: center; gap: 10px; height: 34px; padding: 0 10px; border: 0; border-radius: 11px; background: transparent; color: var(--text); font-size: 13px; text-align: left; text-decoration: none; }
  .item.on { background: var(--surface-2); font-weight: 700; }
  .ic { display: inline-flex; color: var(--muted); }
  .lbl { flex: 1; }
  .badge { min-width: 18px; height: 18px; padding: 0 5px; box-sizing: border-box; border-radius: 9px; background: var(--accent-soft); color: var(--accent-text); font-size: 11px; font-weight: 700; display: grid; place-items: center; }
  .bottom { margin-top: auto; display: flex; flex-direction: column; gap: 8px; }
  .rec, .stop { width: 100%; height: 44px; display: flex; align-items: center; justify-content: center; gap: 8px; border: 0; border-radius: 14px; background: var(--accent); color: var(--ink-on-accent); font-size: 14px; font-weight: 800; }
  .stop { gap: 10px; }
  .eq { display: flex; align-items: center; gap: 2px; height: 12px; }
  .eq span { display: block; width: 2px; border-radius: 1px; background: var(--ink-on-accent); transition: height 0.1s; }
  .dl { display: flex; flex-direction: column; gap: 7px; padding: 10px 12px; border: 1px solid var(--border); border-radius: 14px; background: var(--surface); color: var(--text); text-align: left; }
  .dl-top { display: flex; align-items: center; gap: 8px; font-size: 12px; }
  .dl-ic { display: inline-flex; color: var(--accent-text); }
  .dl-name { flex: 1; font-weight: 700; }
  .dl-pct { font-size: 11px; color: var(--muted); }
  .dl-bar { position: relative; display: block; height: 4px; border-radius: 2px; overflow: hidden; background: var(--surface-2); }
  .dl-bar span { position: absolute; left: 0; top: 0; bottom: 0; border-radius: 2px; background: linear-gradient(90deg, var(--accent), var(--accent-soft), var(--accent)); background-size: 200% 100%; animation: ycShimmer 1.6s linear infinite; transition: width 0.2s; }
</style>
