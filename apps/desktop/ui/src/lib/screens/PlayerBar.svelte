<script lang="ts">
  import Button from "../kit/Button.svelte";
  import { t } from "../i18n/index.svelte";
  import { clock } from "../format";
  import { speakerColors, speakerIndex } from "../speakers";
  import type { Turn } from "../api";
  import type { Player } from "../player.svelte";

  let { player, turns }: { player: Player; turns: Turn[] } = $props();
  const N = 90;
  const speakerAt = (ms: number) => { let k = "me"; for (const tr of turns) { if (ms >= tr.startMs) k = tr.speaker; else break; } return k; };
  // A deterministic pseudo-waveform coloured by who speaks (the real peaks come with the audio index later).
  const bars = $derived(Array.from({ length: N }, (_, i) => {
    const ms = ((i + 0.5) / N) * player.duration;
    const a = Math.abs(Math.sin((i + 1) * 12.9898) * 43758.5453) % 1;
    const b = Math.abs(Math.sin((i + 7) * 78.233) * 12543.21) % 1;
    return { h: Math.round(4 + a * 13 + b * 8), c: speakerColors[speakerIndex(speakerAt(ms))], ms };
  }));
  const head = $derived((player.pos / Math.max(1, player.duration)) * 100);

  function seekBar(e: MouseEvent) {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    player.seek(Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)) * player.duration);
  }
</script>

<div class="yc-in bar">
  <button type="button" class="yc-press play" onclick={() => player.toggle()} aria-label={player.playing ? t("Пауза") : t("Слушать")}>
    {#if !player.playing}
      <svg class="yc-icf" width="12" height="12" viewBox="0 0 24 24" style="margin-left: 2px"><path d="M7 4.5v15l12.5-7.5z" /></svg>
    {:else}
      <svg class="yc-ic" width="12" height="12" viewBox="0 0 24 24" style="stroke-width: 3.4"><path d="M8 5v14M16 5v14" /></svg>
    {/if}
  </button>
  <span class="mg-mono yc-num time"><span style="color: var(--text)">{clock(player.pos)}</span> / {clock(player.duration)}</span>
  <button type="button" onclick={seekBar} aria-label={t("Перемотать запись")} class="wave">
    {#each bars as b, i (i)}
      <span style="height: {b.h}px; background: {b.c}; opacity: {b.ms <= player.pos ? 1 : 0.28}"></span>
    {/each}
    <span aria-hidden="true" class="head" style="left: {head}%"></span>
  </button>
  <Button size="sm" variant="ghost" onclick={() => player.cycleSpeed()} aria-label={t("Скорость")}>{player.speedLabel}</Button>
</div>

<style>
  .bar { display: flex; align-items: center; gap: 10px; height: 44px; box-sizing: border-box; padding: 0 6px; border: 1px solid var(--border); border-radius: 22px; background: var(--dock); backdrop-filter: blur(16px); -webkit-backdrop-filter: blur(16px); box-shadow: var(--shadow-dock); }
  .play { flex: none; width: 32px; height: 32px; border: 0; border-radius: 50%; background: var(--accent); color: var(--ink-on-accent); display: grid; place-items: center; }
  .time { flex: none; font-size: 12px; color: var(--muted); }
  .wave { flex: 1; min-width: 80px; height: 28px; position: relative; display: flex; align-items: center; gap: 2px; padding: 0; border: 0; background: none; cursor: pointer; }
  .wave span:not(.head) { display: block; flex: 1; min-width: 1px; border-radius: 2px; }
  .head { position: absolute; top: 0; bottom: 0; width: 2px; margin-left: -1px; border-radius: 1px; background: var(--text); }
</style>
