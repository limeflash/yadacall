<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import { api } from "../api";
  import type { MeetingListItem, Stage } from "../api";
  import { app, elapsedMs } from "../state.svelte";
  import { t } from "../i18n/index.svelte";
  import { clock, dayHeading, dayKey, timeRange } from "../format";
  import { go, href } from "../router.svelte";

  let items = $state<MeetingListItem[]>([]);
  let loaded = $state(false);
  let q = $state("");

  async function load() { items = await api.listMeetings(); loaded = true; }
  onMount(load);
  // Refetch when something changed (status events, recorder start/stop).
  let lastRev = app.rev;
  $effect(() => { const r = app.rev; if (r !== lastRev) { lastRev = r; load(); } });

  const stageLabel: Record<Stage, string> = { transcribe: "Распознаю локально", diarize: "Размечаю голоса", index: "Индексирую", summarize: "Собираю итоги" };

  const groups = $derived.by(() => {
    const out: { key: string; heading: string; items: MeetingListItem[] }[] = [];
    for (const m of items) {
      const k = dayKey(m.startedAt);
      let g = out.find((x) => x.key === k);
      if (!g) { g = { key: k, heading: dayHeading(m.startedAt), items: [] }; out.push(g); }
      g.items.push(m);
    }
    return out;
  });

  function ask(e: SubmitEvent) {
    e.preventDefault();
    go({ name: "ask", q: q.trim() || undefined });
  }
</script>

<main>
  <div class="yc-scroll" style="flex: 1; min-height: 0">
    <div class="yc-in col">
      <h1>{t("Встречи")}</h1>

      {#if loaded && items.length === 0}
        <p class="empty">{t("Пока пусто. Запиши первую встречу или загрузи готовую запись.")}</p>
      {/if}

      {#each groups as g (g.key)}
        <div class="day">{g.heading}</div>
        {#each g.items as m (m.id)}
          {@const st = m.status}
          <a class="yc-lift card" href={m.status.state === "recording" ? href({ name: "record" }) : href({ name: "meeting", id: m.id, tab: "summary" })}>
            <span class="top">
              <span class="yc-ellipsis title">{m.title}</span>
              {#if st.state === "recording"}
                <span class="rt"><span class="yc-rec dot"></span><span class="mg-mono yc-num">{clock(elapsedMs())}</span></span>
              {:else}
                <span class="mg-mono range">{timeRange(m.startedAt, m.durationMs)}</span>
              {/if}
            </span>
            {#if st.state === "processing"}
              <span class="sub flex"><span style="flex: 1">{t(stageLabel[st.stage])}</span><span class="mg-mono yc-num pct">{Math.round(st.progress * 100)}%</span></span>
              <span aria-hidden="true" class="bar"><span style="width: {Math.round(st.progress * 100)}%"></span></span>
            {:else if st.state === "failed"}
              <span class="sub warn">{st.message}</span>
            {:else}
              <span class="yc-ellipsis sub">{m.status.state === "recording" ? t("Идёт запись — текст пишется на лету") : m.snippet ?? ""}</span>
            {/if}
            {#if m.audioNote}
              <span class="note"><Icon name="clock" size={12} />{m.audioNote}</span>
            {/if}
          </a>
        {/each}
      {/each}
    </div>
  </div>

  <div aria-hidden="true" class="fade"></div>
  <form class="askbar" onsubmit={ask}>
    <label style="flex: 1; min-width: 0; display: flex">
      <span class="mg-sr">{t("Вопрос ко всем встречам")}</span>
      <input type="text" bind:value={q} placeholder={t("Какие задачи на мне на этой неделе?")} />
    </label>
    <button type="submit" aria-label={t("Спросить по всем встречам")} class="yc-press send"><Icon name="arrowUp" size={16} /></button>
  </form>
</main>

<style>
  main { flex: 1 1 520px; min-width: 0; display: flex; flex-direction: column; min-height: 0; position: relative; }
  .col { max-width: 680px; box-sizing: border-box; margin: 0 auto; padding: 40px 28px 120px; display: flex; flex-direction: column; gap: 8px; }
  h1 { margin: 0 0 14px 4px; font-size: 30px; line-height: 1.15; font-weight: 800; letter-spacing: -0.04em; }
  .day { padding: 14px 4px 2px; font-size: 12px; font-weight: 600; color: var(--muted); }
  .day:first-of-type { padding-top: 4px; }
  .empty { margin: 24px 4px; color: var(--muted); }
  .card { display: flex; flex-direction: column; gap: 3px; padding: 13px 16px; border: 1px solid var(--border); border-radius: 16px; background: var(--surface); color: var(--text); text-decoration: none; }
  .top { display: flex; align-items: baseline; gap: 12px; }
  .title { flex: 1; font-size: 15px; font-weight: 700; }
  .range { flex: none; font-size: 12px; color: var(--muted); }
  .rt { flex: none; display: inline-flex; align-items: center; gap: 6px; font-size: 12px; font-weight: 700; }
  .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--danger); }
  .sub { font-size: 13px; color: var(--muted); }
  .sub.flex { display: flex; align-items: center; gap: 10px; }
  .sub.warn { color: var(--warning-text); }
  .pct { font-size: 12px; font-weight: 700; color: var(--accent-text); }
  .bar { position: relative; display: block; height: 12px; margin-top: 4px; border-radius: 4px; overflow: hidden; background: var(--surface-2); }
  .bar span { position: absolute; left: 0; top: 0; bottom: 0; background: linear-gradient(90deg, var(--accent), var(--accent-soft), var(--accent)); background-size: 200% 100%; animation: ycShimmer 1.6s linear infinite; transition: width 0.2s; }
  .note { display: inline-flex; align-items: center; gap: 6px; margin-top: 4px; font-size: 12px; color: var(--muted); }
  .fade { position: absolute; left: 0; right: 10px; bottom: 0; height: 110px; pointer-events: none; background: linear-gradient(to bottom, transparent, var(--bg) 60%); }
  .askbar { position: absolute; left: 28px; right: 28px; bottom: 18px; max-width: 624px; margin: 0 auto; display: flex; align-items: center; gap: 10px; height: 48px; box-sizing: border-box; padding: 0 5px 0 18px; border: 1px solid var(--border); border-radius: 24px; background: var(--dock); backdrop-filter: blur(16px); -webkit-backdrop-filter: blur(16px); box-shadow: var(--shadow-dock); }
  input { flex: 1; min-width: 0; height: 36px; margin: 0; padding: 0; border: 0; background: transparent; color: var(--text); font: inherit; font-size: 14px; outline-offset: 6px; }
  .send { flex: none; width: 36px; height: 36px; border: 0; border-radius: 50%; background: var(--surface-2); color: var(--muted); display: grid; place-items: center; }
</style>
