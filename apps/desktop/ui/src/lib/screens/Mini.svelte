<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import { api } from "../api";
  import { app, elapsedMs, levels, togglePause, addBookmark, stopRecording } from "../state.svelte";
  import { t } from "../i18n/index.svelte";
  import { clock } from "../format";
  import { visibleInterval } from "../visibility";

  const paused = $derived(app.recorder.phase === "paused");
  // The level meter is moved straight on the DOM (transform only): no reactive state, no layout, no re-render.
  let meter: HTMLElement;
  onMount(() => {
    api.popoverFit("mini", 64);
    return visibleInterval(() => {
      const lvl = Math.max(levels.mic.peak, levels.system.peak);
      const now = performance.now();
      Array.from(meter.children).forEach((el, j) => {
        const h = paused ? 2 : 3 + lvl * (6 + 7 * Math.abs(Math.sin(now / 150 + j * 1.7)));
        (el as HTMLElement).style.transform = `scaleY(${(h / 18).toFixed(2)})`;
      });
    }, 100);
  });
  async function stop() { const id = await stopRecording(); await api.openInMain(id ? `#/meeting/${id}/summary` : "#/"); }
</script>

<div class="stage">
  <div class="pill" data-tauri-drag-region>
    <span class={paused ? "" : "yc-rec"} style="width: 7px; height: 7px; border-radius: 50%; background: {paused ? 'var(--warning)' : 'var(--danger)'}; pointer-events: none"></span>
    <span class="mg-mono yc-num" style="font-size: 13px; font-weight: 600; pointer-events: none">{clock(elapsedMs())}</span>
    <span aria-hidden="true" class="mw" bind:this={meter}>{#each Array(14) as _, i (i)}<span style="transform: scaleY(0.11)"></span>{/each}</span>
    <button type="button" class="mg-ib mg-ib--flat no-drag" aria-label={paused ? t("Дальше") : t("Пауза")} onclick={togglePause}>
      {#if paused}<svg class="yc-icf" width="12" height="12" viewBox="0 0 24 24"><path d="M7 4.5v15l12.5-7.5z" /></svg>{:else}<svg class="yc-ic" width="13" height="13" viewBox="0 0 24 24" style="stroke-width: 3"><path d="M8 5v14M16 5v14" /></svg>{/if}
    </button>
    <button type="button" class="mg-ib mg-ib--flat no-drag" aria-label={t("Поставить закладку")} onclick={addBookmark}><Icon name="bookmark" size={14} /></button>
    <button type="button" class="yc-press stopb no-drag" aria-label={t("Завершить запись")} onclick={stop}><svg class="yc-icf" width="11" height="11" viewBox="0 0 24 24"><rect x="5" y="5" width="14" height="14" rx="3" /></svg></button>
  </div>
</div>

<style>
  .stage { padding: 10px; }
  .pill { display: flex; align-items: center; gap: 10px; height: 44px; box-sizing: border-box; padding: 0 6px 0 14px; border: 1px solid var(--border); border-radius: 22px; background: var(--surface); box-shadow: var(--shadow-dock); }
  .mw { width: 60px; height: 18px; display: flex; align-items: center; gap: 2px; pointer-events: none; }
  .mw span { display: block; flex: 1; height: 18px; border-radius: 1px; background: var(--accent); transition: transform 0.1s linear; will-change: transform; }
  .stopb { width: 30px; height: 30px; border: 0; border-radius: 50%; background: var(--accent); color: var(--ink-on-accent); display: grid; place-items: center; }
</style>
