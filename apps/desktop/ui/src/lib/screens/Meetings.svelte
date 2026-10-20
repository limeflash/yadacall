<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import FolderBar from "../components/FolderBar.svelte";
  import FolderScope from "../components/FolderScope.svelte";
  import MeetingMenu from "../components/MeetingMenu.svelte";
  import { folders, filters, loadFolders, matchesFilter, setFavorite, moveToFolder, folderById, folderColor } from "../folders.svelte";
  import { drag, startMeetingDrag, wasDragged } from "../dragMeeting.svelte";
  import { api, integrations } from "../api";
  import type { AgendaEvent, CalendarAccount, MeetingListItem, Stage } from "../api";
  import { app, elapsedMs, onIntegrationEvent } from "../state.svelte";
  import { t, plural } from "../i18n/index.svelte";
  import { clock, dayHeading, dayKey, hhmm, timeRange, until } from "../format";
  import { go, href } from "../router.svelte";

  let items = $state<MeetingListItem[]>([]);
  let loaded = $state(false);
  let q = $state("");

  // "Скоро": the next call from the connected calendars (today only).
  let agenda = $state<AgendaEvent[]>([]);
  let accounts = $state<CalendarAccount[]>([]);
  async function loadSoon() {
    const end = new Date(); end.setHours(24, 0, 0, 0);
    try {
      [agenda, accounts] = await Promise.all([integrations.agenda({ from: new Date().toISOString(), to: end.toISOString() }), integrations.accounts()]);
    } catch { agenda = []; }
  }
  const upcoming = $derived(agenda.filter((e) => Date.parse(e.end) > app.now));
  const soon = $derived(upcoming[0]);
  const soonAcc = $derived(accounts.find((a) => a.id === soon?.accountId));
  const soonLeft = $derived(soon ? (Date.parse(soon.start) - app.now) / 1000 : 0);

  async function load() { items = await api.listMeetings(); loaded = true; }
  onMount(() => {
    load(); loadSoon(); loadFolders();
    const off = onIntegrationEvent((e) => { if (e.type === "agendaChanged") loadSoon(); });
    const iv = setInterval(loadSoon, 30000);
    return () => { off(); clearInterval(iv); };
  });
  // Refetch when something changed (status events, recorder start/stop).
  let lastRev = app.rev;
  $effect(() => { const r = app.rev; if (r !== lastRev) { lastRev = r; load(); } });

  const stageLabel: Record<Stage, string> = { transcribe: "Распознаю локально", diarize: "Размечаю голоса", index: "Индексирую", summarize: "Собираю итоги" };

  const filter = $derived(filters.meetings);
  const shown = $derived(items.filter((m) => matchesFilter(filter, m)));
  const groups = $derived.by(() => {
    const out: { key: string; heading: string; items: MeetingListItem[] }[] = [];
    for (const m of shown) {
      const k = dayKey(m.startedAt);
      let g = out.find((x) => x.key === k);
      if (!g) { g = { key: k, heading: dayHeading(m.startedAt), items: [] }; out.push(g); }
      g.items.push(m);
    }
    return out;
  });

  // The ask bar searches the folder being browsed unless the user picks another scope (null = everything).
  let scope = $state<string | null | undefined>(undefined);
  const askFolder = $derived(scope === undefined ? (folderById(filter) ? filter : undefined) : (scope ?? undefined));
  function ask(e: SubmitEvent) {
    e.preventDefault();
    go({ name: "ask", q: q.trim() || undefined, folder: askFolder });
  }

  // ---- favourites / folders ---------------------------------------------------------------------
  let menu = $state<{ m: MeetingListItem; x: number; y: number; above?: number; trigger?: HTMLElement } | null>(null);
  function openMenu(m: MeetingListItem, e: MouseEvent) {
    const btn = e.currentTarget as HTMLElement;
    const r = btn.getBoundingClientRect();
    menu = menu?.m.id === m.id ? null : { m, x: r.right - 236, y: r.bottom + 4, above: r.top - 0, trigger: btn };
  }
  function ctxMenu(m: MeetingListItem, e: MouseEvent) {
    if (m.status.state === "recording") return;
    e.preventDefault();
    menu = { m, x: e.clientX, y: e.clientY };
  }
  function closeMenu(escaped?: boolean) { const tr = menu?.trigger; menu = null; if (escaped) tr?.focus(); }
  function dropOn(m: MeetingListItem, target: string) {
    if (target === "fav") setFavorite(m, true);
    else moveToFolder(m, target === "none" ? null : target);
  }
  const emptyText = $derived(filter === "fav" ? "Здесь будут важные встречи: нажми ★ на карточке — звук такой встречи не удалится."
    : filter === "none" ? "Все встречи разложены по папкам."
    : "В этой папке пока пусто. Перетащи сюда встречу или выбери папку в меню ⋯ на карточке.");
</script>

<main>
  <div class="yc-scroll" style="flex: 1; min-height: 0">
    <div class="yc-in col">
      <h1>{t("Встречи")}</h1>

      {#if loaded && items.length === 0}
        <p class="empty">{t("Пока пусто. Запиши первую встречу или загрузи готовую запись.")}</p>
      {/if}
      {#if loaded && (items.length > 0 || folders.list.length > 0)}
        <FolderBar value={filter} onchange={(v) => { filters.meetings = v; scope = undefined; }} />
      {/if}
      {#if loaded && items.length > 0 && shown.length === 0}
        <p class="empty">{t(emptyText)}</p>
      {/if}

      {#if soon && filter === "all"}
        <div class="soonh">
          <span class="day" style="padding: 0">{t("Скоро")}</span>
          <a class="yc-press cal" href={href({ name: "calendar" })}>{t("Календарь")} →</a>
        </div>
        <a class="yc-lift soon" href={href({ name: "calendar" })}>
          <span class="mg-mono" style="flex: none; font-size: 13px; font-weight: 600">{hhmm(soon.start)}</span>
          <span aria-hidden="true" class="sbar" style="background: {soonAcc?.color}"></span>
          <span style="flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px">
            <span class="yc-ellipsis" style="font-size: 15px; font-weight: 700">{soon.title}</span>
            <span class="yc-ellipsis" style="font-size: 13px; color: var(--muted)">{soonAcc?.label}{upcoming.length > 1 ? ` · ${t("ещё {n} {w} сегодня", { n: upcoming.length - 1, w: plural(upcoming.length - 1, ["звонок", "звонка", "звонков"], ["call", "calls"]) })}` : ""}</span>
          </span>
          <span style="flex: none; display: flex; flex-direction: column; align-items: flex-end; gap: 4px">
            <span class="mg-mono yc-num" style="font-size: 12px; font-weight: 700; color: {soonLeft <= 0 ? 'var(--danger)' : 'var(--accent-text)'}">{until(soonLeft)}</span>
            <span style="display: inline-flex; align-items: center; gap: 5px; font-size: 11.5px; color: var(--muted)"><span aria-hidden="true" class="sdot" class:off={!soon.willRecord}></span>{soon.willRecord ? t("запишу") : t("без записи")}</span>
          </span>
        </a>
      {/if}

      {#each groups as g (g.key)}
        <div class="day">{g.heading}</div>
        {#each g.items as m (m.id)}
          {@const st = m.status}
          {@const folder = folderById(m.folderId)}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div class="yc-lift cw" class:dragging={drag.active && drag.title === m.title} oncontextmenu={(e) => ctxMenu(m, e)} onpointerdown={(e) => { if (st.state !== "recording" && !(e.target as HTMLElement).closest(".acts")) startMeetingDrag(e, m.title, (tg) => dropOn(m, tg)); }}>
            <a class="card" draggable="false" href={m.status.state === "recording" ? href({ name: "record" }) : href({ name: "meeting", id: m.id, tab: "summary" })} onclick={(e) => { if (wasDragged()) e.preventDefault(); }}>
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
                <span class="sub warn">{t(st.message)}</span>
              {:else}
                <span class="yc-ellipsis sub">{m.status.state === "recording" ? t("Идёт запись — текст пишется на лету") : m.snippet ?? ""}</span>
              {/if}
              {#if (folder && filter !== folder.id) || m.favorite || m.audioNote}
                <span class="meta">
                  {#if folder && filter !== folder.id}<span class="ftag"><span aria-hidden="true" class="cd" style="background: {folderColor(folder)}"></span>{folder.name}</span>{/if}
                  {#if m.favorite}<span class="note fav"><Icon name="star" size={12} style="fill: currentColor" />{t("звук хранится всегда")}</span>
                  {:else if m.audioNote}<span class="note"><Icon name="clock" size={12} />{m.audioNote}</span>{/if}
                </span>
              {/if}
            </a>
            {#if st.state !== "recording"}
              <span class="acts">
                <button type="button" class="act" class:on={m.favorite} aria-pressed={m.favorite} aria-label={m.favorite ? t("Убрать из избранного") : t("В избранное")} title={m.favorite ? t("Убрать из избранного") : t("В избранное — звук хранится всегда")} onclick={() => setFavorite(m, !m.favorite)}><Icon name="star" size={15} style={m.favorite ? "fill: currentColor" : undefined} /></button>
                <button type="button" class="act more" data-menu-trigger aria-haspopup="menu" aria-expanded={menu?.m.id === m.id} aria-label={t("Избранное и папка")} title={t("Избранное и папка")} onclick={(e) => openMenu(m, e)}><Icon name="dots" size={15} style="stroke-width: 3.4" /></button>
              </span>
            {/if}
          </div>
        {/each}
      {/each}
    </div>
  </div>

  <div aria-hidden="true" class="fade"></div>
  <form class="askbar" onsubmit={ask}>
    <FolderScope value={askFolder} onchange={(id) => (scope = id ?? null)} />
    <label style="flex: 1; min-width: 0; display: flex">
      <span class="mg-sr">{t("Вопрос ко всем встречам")}</span>
      <input type="text" bind:value={q} placeholder={t("Какие задачи на мне на этой неделе?")} />
    </label>
    <button type="submit" aria-label={t("Спросить по всем встречам")} class="yc-press send"><Icon name="arrowUp" size={16} /></button>
  </form>
</main>

{#if menu}
  <MeetingMenu x={menu.x} y={menu.y} above={menu.above} favorite={menu.m.favorite} folderId={menu.m.folderId} onfavorite={(v) => setFavorite(menu!.m, v)} onmove={(id) => moveToFolder(menu!.m, id)} onclose={closeMenu} />
{/if}
{#if drag.active}
  <div class="ghost" aria-hidden="true" style="left: {drag.x + 14}px; top: {drag.y + 10}px"><Icon name="folder" size={13} /><span class="yc-ellipsis">{drag.title}</span></div>
{/if}

<style>
  main { flex: 1 1 520px; min-width: 0; display: flex; flex-direction: column; min-height: 0; position: relative; }
  .col { max-width: 680px; box-sizing: border-box; margin: 0 auto; padding: 40px 28px 120px; display: flex; flex-direction: column; gap: 8px; }
  h1 { margin: 0 0 14px 4px; font-size: 30px; line-height: 1.15; font-weight: 800; letter-spacing: -0.04em; }
  .soonh { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; padding: 4px 4px 2px; }
  .cal { font-size: 12.5px; font-weight: 600; color: var(--accent-text); text-decoration: none; white-space: nowrap; }
  .soon { display: flex; align-items: center; gap: 14px; padding: 12px 16px 12px 14px; border: 1px solid var(--border); border-radius: 16px; background: var(--surface); color: var(--text); text-decoration: none; }
  .sbar { flex: none; width: 3px; height: 34px; border-radius: 2px; }
  .sdot { width: 6px; height: 6px; border-radius: 50%; background: var(--accent); }
  .sdot.off { background: transparent; box-shadow: inset 0 0 0 1.5px var(--faint); }
  .day { padding: 14px 4px 2px; font-size: 12px; font-weight: 600; color: var(--muted); }
  .day:first-of-type { padding-top: 4px; }
  .empty { margin: 24px 4px; color: var(--muted); }
  .cw { position: relative; border: 1px solid var(--border); border-radius: 16px; background: var(--surface); }
  .cw.dragging { opacity: 0.55; }
  .card { display: flex; flex-direction: column; gap: 3px; padding: 13px 16px; border-radius: 15px; color: var(--text); text-decoration: none; }
  .card:focus-visible { outline-offset: -2px; }
  .acts { position: absolute; top: 8px; right: 9px; display: flex; gap: 2px; }
  .act { display: grid; place-items: center; width: 26px; height: 26px; padding: 0; border: 0; border-radius: 8px; background: transparent; color: var(--muted); cursor: pointer; opacity: 0; transition: opacity 0.15s, background-color 0.15s, color 0.15s; }
  .cw:hover .act, .cw:focus-within .act, .act[aria-expanded="true"] { opacity: 1; }
  .act.on { opacity: 1; color: var(--accent-text); }
  .act:hover, .act[aria-expanded="true"] { background: var(--surface-2); color: var(--text); }
  .act.on:hover { color: var(--accent-text); }
  .meta { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 12px; margin-top: 4px; }
  .ftag { display: inline-flex; align-items: center; gap: 6px; font-size: 12px; font-weight: 600; color: var(--muted); }
  .cd { flex: none; width: 8px; height: 8px; border-radius: 50%; }
  .ghost { position: fixed; z-index: 70; pointer-events: none; display: flex; align-items: center; gap: 7px; max-width: 240px; height: 30px; padding: 0 12px; border: 1px solid var(--accent-line); border-radius: 15px; background: var(--surface); color: var(--text); font-size: 12px; font-weight: 700; box-shadow: var(--shadow-pop); }
  .top { display: flex; align-items: baseline; gap: 12px; }
  .title { flex: 1; font-size: 15px; font-weight: 700; }
  .range { flex: none; margin-right: 56px; font-size: 12px; color: var(--muted); }
  .rt { flex: none; display: inline-flex; align-items: center; gap: 6px; font-size: 12px; font-weight: 700; }
  .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--danger); }
  .sub { font-size: 13px; color: var(--muted); }
  .sub.flex { display: flex; align-items: center; gap: 10px; }
  .sub.warn { color: var(--warning-text); }
  .pct { font-size: 12px; font-weight: 700; color: var(--accent-text); }
  .bar { position: relative; display: block; height: 12px; margin-top: 4px; border-radius: 4px; overflow: hidden; background: var(--surface-2); }
  .bar span { position: absolute; left: 0; top: 0; bottom: 0; background: linear-gradient(90deg, var(--accent), var(--accent-soft), var(--accent)); background-size: 200% 100%; animation: ycShimmer 1.6s linear infinite; transition: width 0.2s; }
  .note { display: inline-flex; align-items: center; gap: 6px; font-size: 12px; color: var(--muted); }
  .note.fav { color: var(--accent-text); }
  .fade { position: absolute; left: 0; right: 10px; bottom: 0; height: 110px; pointer-events: none; background: linear-gradient(to bottom, transparent, var(--bg) 60%); }
  .askbar { position: absolute; left: 28px; right: 28px; bottom: 18px; max-width: 624px; margin: 0 auto; display: flex; align-items: center; gap: 10px; height: 48px; box-sizing: border-box; padding: 0 5px 0 18px; border: 1px solid var(--border); border-radius: 24px; background: var(--dock); backdrop-filter: blur(16px); -webkit-backdrop-filter: blur(16px); box-shadow: var(--shadow-dock); }
  input { flex: 1; min-width: 0; height: 36px; margin: 0; padding: 0; border: 0; background: transparent; color: var(--text); font: inherit; font-size: 14px; outline-offset: 6px; }
  .send { flex: none; width: 36px; height: 36px; border: 0; border-radius: 50%; background: var(--surface-2); color: var(--muted); display: grid; place-items: center; }
</style>
