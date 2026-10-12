<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import Button from "../kit/Button.svelte";
  import StatusDot from "../kit/StatusDot.svelte";
  import FilterChips from "../kit/FilterChips.svelte";
  import { api, integrations } from "../api";
  import type { AgendaEvent, CalendarAccount, MeetingListItem } from "../api";
  import { app, onIntegrationEvent } from "../state.svelte";
  import { t, plural } from "../i18n/index.svelte";
  import { durLabel, hhmm, longDate, until } from "../format";
  import { go, href } from "../router.svelte";
  import { toast } from "../toast.svelte";

  const PEOPLE_BG = ["#e6a6b0", "#dcc68c", "#8ec8ea", "#b0cc90", "#b8acf2"];

  let accounts = $state<CalendarAccount[]>([]);
  let events = $state<AgendaEvent[]>([]);
  let meetings = $state<MeetingListItem[]>([]);
  let loaded = $state(false);
  let filter = $state("all");
  let showPast = $state(false);

  async function load() {
    const day = new Date(); day.setHours(0, 0, 0, 0);
    const end = new Date(day); end.setDate(end.getDate() + 2);
    [accounts, events, meetings] = await Promise.all([
      integrations.accounts(),
      integrations.agenda({ from: day.toISOString(), to: end.toISOString() }),
      api.listMeetings(),
    ]);
    loaded = true;
  }
  onMount(() => {
    load();
    return onIntegrationEvent((e) => { if (e.type === "agendaChanged") load(); });
  });
  let lastRev = app.rev;
  $effect(() => { const r = app.rev; if (r !== lastRev) { lastRev = r; load(); } });

  const live = $derived(accounts.filter((a) => a.status === "ok" && a.visible));
  const chips = $derived([{ value: "all", label: t("Все") }, ...live.map((a) => ({ value: a.id, label: a.label }))]);
  // An account that disappeared (hidden / removed) must not leave the list filtered to nothing.
  $effect(() => { if (filter !== "all" && !live.some((a) => a.id === filter)) filter = "all"; });
  const acc = (id: string) => accounts.find((a) => a.id === id);

  const shown = $derived(events.filter((e) => filter === "all" || e.accountId === filter));
  const today = new Date().toDateString();
  const tomorrowKey = (() => { const d = new Date(); d.setDate(d.getDate() + 1); return d.toDateString(); })();
  const dayOf = (e: AgendaEvent) => new Date(e.start).toDateString();
  const past = $derived(shown.filter((e) => Date.parse(e.end) <= app.now && dayOf(e) === today));
  const upcoming = $derived(shown.filter((e) => Date.parse(e.end) > app.now && dayOf(e) === today));
  const tomorrow = $derived(shown.filter((e) => dayOf(e) === tomorrowKey));
  const next = $derived(upcoming[0]);
  const later = $derived(upcoming.slice(1));
  const upIds = $derived(new Set(upcoming.map((e) => e.id)));
  const clashes = (e: AgendaEvent) => e.overlaps.some((id) => upIds.has(id));
  const clashAt = $derived.by(() => {
    const e = later.find(clashes);
    if (!e) return null;
    const other = upcoming.find((o) => e.overlaps.includes(o.id));
    return hhmm(Math.max(Date.parse(e.start), other ? Date.parse(other.start) : 0));
  });

  const recorded = $derived(past.filter((e) => e.willRecord || e.meetingId).length);
  const pastLabel = $derived(
    `${t("Ранее")} · ${past.length} ${plural(past.length, ["встреча", "встречи", "встреч"], ["meeting", "meetings"])}` +
    (recorded === past.length ? `, ${t("записаны")}` : recorded ? `, ${t("записано")} ${recorded}` : ""),
  );
  function pastNote(e: AgendaEvent): { text: string; busy: boolean } | null {
    const m = meetings.find((x) => x.id === e.meetingId);
    if (!m) return null;
    return m.status.state === "processing" ? { text: t("расшифровываю"), busy: true } : { text: t("итоги"), busy: false };
  }

  const nowShort = $derived(hhmm(app.now));
  const left = $derived(next ? (Date.parse(next.start) - app.now) / 1000 : 0);
  const mark = (name: string) => (Array.from(name.trim())[0] ?? "?").toLocaleUpperCase();

  function people(e: AgendaEvent) {
    const shownP = e.attendees.slice(0, 2).map((n, i) => ({ l: mark(n), bg: PEOPLE_BG[i % PEOPLE_BG.length] }));
    return e.attendees.length > 2 ? [...shownP, { l: `+${e.attendees.length - 2}`, bg: "var(--surface-2)" }] : shownP;
  }
  function who(e: AgendaEvent) {
    const a = e.attendees;
    if (a.length === 0) return t("только ты");
    if (a.length <= 2) return t("{names} и ты", { names: a.join(", ") });
    return t("{names} и ещё {n}", { names: a.slice(0, 2).join(", "), n: a.length - 2 });
  }

  async function toggleRec(e: AgendaEvent) {
    const v = !e.willRecord;
    events = events.map((x) => (x.id === e.id ? { ...x, willRecord: v } : x));
    try { await integrations.setEventRecording(e.id, v); } catch (er) { toast.error(er); load(); }
  }
  async function joinRec(e: AgendaEvent) {
    try { await integrations.joinAndRecord(e.id); go({ name: "record" }); } catch (er) { toast.error(er); }
  }
  async function joinOnly(e: AgendaEvent) {
    try {
      const o = await integrations.joinWithoutRecording(e.id);
      toast(t("Открыл Meet в {email} — без записи", { email: o.accountEmail }));
    } catch (er) { toast.error(er); }
  }
</script>

<main>
  <div class="yc-scroll" style="flex: 1; min-height: 0">
    <div class="yc-in col">
      <a class="back" href={href({ name: "meetings" })}><Icon name="back" size={14} />{t("Встречи")}</a>

      <header>
        <div class="ttl">
          <h1>{t("Сегодня")}</h1>
          <span class="date">{longDate(new Date(app.now))}</span>
        </div>
        {#if live.length}
          <a href={href({ name: "google" })} class="yc-press accs" aria-label={t("Аккаунты Google: {names}", { names: live.map((a) => a.label.toLowerCase()).join(", ") })}>
            <span aria-hidden="true" class="stack">
              {#each live as a, i (a.id)}<span class="av" style="background: {a.color}; margin-left: {i ? -3 : 0}px">{mark(a.label)}</span>{/each}
            </span>
            <Icon name="chevronRight" size={12} />
          </a>
        {/if}
      </header>

      {#if loaded && live.length === 0}
        <p class="empty">{t("Календарь пока не подключён. Подключи Google — и звонки появятся здесь.")} <a href={href({ name: "connections" })}>{t("Подключения")}</a></p>
      {/if}

      {#if live.length > 1}
        <FilterChips label={t("Чей календарь")} value={filter} onchange={(v) => (filter = v)} options={chips} />
      {/if}

      {#if past.length}
        <button type="button" class="yc-row pastbtn" onclick={() => (showPast = !showPast)} aria-expanded={showPast}>
          <span class="chev" style="transform: rotate({showPast ? 90 : 0}deg)"><Icon name="chevronRight" size={12} /></span>
          <span style="flex: 1">{pastLabel}</span>
        </button>
        {#if showPast}
          <div class="yc-in" style="display: flex; flex-direction: column">
            {#each past as e (e.id)}
              {@const note = pastNote(e)}
              <svelte:element this={e.meetingId ? "a" : "div"} href={e.meetingId ? href({ name: "meeting", id: e.meetingId, tab: "summary" }) : undefined} class="yc-row prow">
                <span class="mg-mono yc-num ptime">{hhmm(e.start)}</span>
                <span aria-hidden="true" class="pbar" style="background: {acc(e.accountId)?.color}"></span>
                <span class="yc-ellipsis ptitle">{e.title}</span>
                {#if note}
                  <span class="pnote">
                    {#if note.busy}<svg class="yc-ic yc-spin" width="11" height="11" viewBox="0 0 24 24" style="color: var(--busy); stroke-width: 3.2"><path d="M12 3a9 9 0 1 0 9 9" /></svg>{:else}<StatusDot kind="ok" />{/if}
                    {note.text}
                  </span>
                {/if}
              </svelte:element>
            {/each}
          </div>
        {/if}
      {/if}

      {#if live.length}
        <div aria-hidden="true" class="nowline">
          <span class="mg-mono yc-num nl-time">{nowShort}</span>
          <span class="nl-dot"></span>
          <span class="nl-line"></span>
        </div>
      {/if}

      {#if next}
        {@const a = acc(next.accountId)}
        <article class="yc-in next">
          <div class="nrow">
            <span class="ntime">
              <span class="mg-mono nt">{hhmm(next.start)}</span>
              <span class="nd">{durLabel(Math.round((Date.parse(next.end) - Date.parse(next.start)) / 60000))}</span>
            </span>
            <span aria-hidden="true" class="nbar" style="background: {a?.color}"></span>
            <span class="nbody">
              <span style="display: flex; align-items: center; gap: 10px">
                <span class="yc-ellipsis ntitle">{next.title}</span>
                <span class="mg-mono yc-num ncd" style="color: {left <= 0 ? 'var(--danger)' : 'var(--accent-text)'}">{until(left)}</span>
              </span>
              <span class="yc-ellipsis nsub">{a?.label} · <span class="mg-mono" style="font-size: 12px">{a?.email}</span></span>
            </span>
          </div>
          <div class="nact">
            <span class="who">
              <span style="display: inline-flex; padding-right: 4px">{#each people(next) as p (p.l)}<span class="pav" style="background: {p.bg}">{p.l}</span>{/each}</span>
              <span class="yc-ellipsis">{who(next)}</span>
            </span>
            <Button variant="ghost" onclick={() => joinOnly(next)}>{t("Без записи")}</Button>
            <button type="button" class="yc-press join" onclick={() => joinRec(next)}><Icon name="video" size={14} />{t("Войти и записать")}</button>
          </div>
        </article>
      {/if}

      {#if later.length}
        <div style="display: flex; flex-direction: column; padding-top: 6px">
          {#each later as e (e.id)}
            <div class="yc-row lrow">
              <span class="mg-mono ltime">{hhmm(e.start)}</span>
              <span aria-hidden="true" class="lbar" style="background: {acc(e.accountId)?.color}"></span>
              <span class="lbody">
                <span class="yc-ellipsis ltitle">{e.title}</span>
                <span class="lsub">
                  <span class="yc-ellipsis">{acc(e.accountId)?.label} · {durLabel(Math.round((Date.parse(e.end) - Date.parse(e.start)) / 60000))}</span>
                  {#if clashes(e)}<span class="clash">· {t("накладка")}</span>{/if}
                </span>
              </span>
              <button type="button" class="yc-press chip" class:on={e.willRecord} aria-pressed={e.willRecord}
                aria-label={(e.willRecord ? t("Не записывать «{title}»", { title: e.title }) : t("Записать «{title}»", { title: e.title }))} onclick={() => toggleRec(e)}>
                <span aria-hidden="true" class="cdot"></span>{e.willRecord ? t("запишу") : t("без записи")}
              </button>
            </div>
          {/each}
        </div>
      {/if}

      {#if clashAt}
        <span class="yc-in yc-pretty clashnote">
          <span style="display: inline-flex; padding-top: 2px; color: var(--warning-text)"><Icon name="warn" size={12} /></span>
          {t("В {time} звонки из двух аккаунтов идут одновременно — запишу тот, в который зайдёшь.", { time: clashAt })}
        </span>
      {/if}

      {#if loaded && live.length && !next}
        <p class="empty" style="margin-top: 8px">{t("На сегодня звонков больше нет.")}</p>
      {/if}

      {#if tomorrow.length}
        <div class="tmh"><span style="flex: none; width: 50px"></span><span style="font-size: 12px; font-weight: 600; color: var(--muted)">{t("Завтра")}</span></div>
        <div style="display: flex; flex-direction: column">
          {#each tomorrow as e (e.id)}
            <div class="trow">
              <span class="mg-mono ttime">{hhmm(e.start)}</span>
              <span aria-hidden="true" class="tbar" style="background: {acc(e.accountId)?.color}"></span>
              <span class="yc-ellipsis ttitle">{e.title}<span class="mg-sr"> · {acc(e.accountId)?.label}</span></span>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  </div>
</main>

<style>
  main { position: relative; flex: 1 1 520px; min-width: 0; display: flex; flex-direction: column; min-height: 0; }
  .col { max-width: 600px; box-sizing: border-box; margin: 0 auto; padding: 22px 28px 48px; display: flex; flex-direction: column; gap: 6px; }
  .back { align-self: flex-start; display: inline-flex; align-items: center; gap: 6px; font-size: 13px; color: var(--muted); text-decoration: none; }
  header { display: flex; align-items: flex-end; gap: 16px; padding: 4px 0 10px; }
  .ttl { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  h1 { margin: 0; font-size: 28px; line-height: 1.15; font-weight: 800; letter-spacing: -0.04em; }
  .date { font-size: 13px; color: var(--muted); }
  .accs { flex: none; display: inline-flex; align-items: center; gap: 4px; height: 30px; padding: 0 6px 0 4px; border-radius: 15px; color: var(--muted); text-decoration: none; }
  .stack { display: inline-flex; }
  .av { width: 22px; height: 22px; border-radius: 50%; color: #0c0c0e; font-size: 11px; font-weight: 800; display: grid; place-items: center; box-shadow: 0 0 0 2px var(--bg); }
  .empty { margin: 8px 4px; color: var(--muted); font-size: 13px; }
  .empty a { color: var(--accent-text); }
  .pastbtn { display: flex; align-items: center; gap: 8px; margin-top: 10px; padding: 7px 10px 7px 8px; border: 0; border-radius: 10px; background: transparent; color: var(--muted); font: inherit; font-size: 12.5px; text-align: left; }
  .chev { display: inline-flex; transition: transform 0.25s var(--ease-out-soft); }
  .prow { display: flex; align-items: center; gap: 12px; padding: 8px 10px 8px 0; border-radius: 12px; color: var(--muted); text-decoration: none; }
  .ptime { flex: none; width: 50px; text-align: right; font-size: 12px; }
  .pbar { flex: none; width: 3px; height: 18px; border-radius: 2px; opacity: 0.45; }
  .ptitle { flex: 1; min-width: 0; font-size: 13.5px; }
  .pnote { flex: none; display: inline-flex; align-items: center; gap: 6px; font-size: 12px; }
  .nowline { display: flex; align-items: center; gap: 10px; padding: 6px 0 4px; }
  .nl-time { flex: none; width: 50px; text-align: right; font-size: 11px; font-weight: 700; color: var(--accent-text); }
  .nl-dot { width: 7px; height: 7px; flex: none; border-radius: 50%; background: var(--accent); box-shadow: 0 0 0 4px var(--accent-soft); }
  .nl-line { flex: 1; height: 1px; background: linear-gradient(90deg, var(--accent-line), transparent); }
  .next { display: flex; flex-direction: column; gap: 16px; padding: 16px 18px 16px 0; border: 1px solid var(--accent-line); border-radius: 20px; background: var(--surface); }
  .nrow { display: flex; align-items: flex-start; gap: 12px; }
  .ntime { flex: none; width: 62px; display: flex; flex-direction: column; align-items: flex-end; gap: 2px; }
  .nt { font-size: 17px; line-height: 1.3; font-weight: 600; letter-spacing: -0.02em; }
  .nd { font-size: 11.5px; color: var(--muted); }
  .nbar { flex: none; align-self: stretch; width: 3px; border-radius: 2px; }
  .nbody { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 4px; }
  .ntitle { flex: 1; min-width: 0; font-size: 18px; line-height: 1.3; font-weight: 800; letter-spacing: -0.025em; }
  .ncd { flex: none; font-size: 12.5px; font-weight: 700; }
  .nsub { font-size: 12.5px; color: var(--muted); }
  .nact { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; padding-left: 77px; }
  .who { flex: 1 1 140px; min-width: 0; display: inline-flex; align-items: center; gap: 8px; font-size: 12.5px; color: var(--muted); }
  .pav { width: 22px; height: 22px; margin-right: -5px; border-radius: 50%; color: #0c0c0e; font-size: 10.5px; font-weight: 800; display: grid; place-items: center; box-shadow: 0 0 0 2px var(--surface); }
  .join { display: inline-flex; align-items: center; gap: 8px; height: 36px; padding: 0 16px; border: 0; border-radius: 11px; background: var(--accent); color: var(--ink-on-accent); font-size: 13.5px; font-weight: 800; }
  .lrow { display: flex; align-items: center; gap: 12px; padding: 10px 6px 10px 0; border-radius: 14px; }
  .ltime { flex: none; width: 50px; text-align: right; font-size: 13px; font-weight: 600; }
  .lbar { flex: none; width: 3px; height: 30px; border-radius: 2px; }
  .lbody { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 1px; }
  .ltitle { font-size: 14px; font-weight: 600; }
  .lsub { display: flex; align-items: center; gap: 6px; min-width: 0; font-size: 12px; color: var(--muted); }
  .clash { flex: none; color: var(--warning-text); }
  .chip { flex: none; display: inline-flex; align-items: center; gap: 6px; height: 26px; padding: 0 10px; border: 1px solid var(--border); border-radius: 13px; background: transparent; color: var(--muted); font: inherit; font-size: 12px; font-weight: 600; transition: background-color 0.2s, border-color 0.2s, color 0.2s; }
  .chip.on { background: var(--accent-soft); border-color: transparent; color: var(--accent-text); }
  .cdot { width: 7px; height: 7px; border-radius: 50%; background: transparent; box-shadow: inset 0 0 0 1.5px var(--faint); }
  .chip.on .cdot { background: var(--accent); box-shadow: inset 0 0 0 1.5px var(--accent); }
  .clashnote { display: flex; align-items: flex-start; gap: 7px; padding: 0 6px 0 62px; font-size: 12px; color: var(--muted); }
  .tmh { display: flex; align-items: center; gap: 12px; padding: 22px 0 6px; }
  .trow { display: flex; align-items: center; gap: 12px; padding: 6px 6px 6px 0; color: var(--muted); }
  .ttime { flex: none; width: 50px; text-align: right; font-size: 12px; }
  .tbar { flex: none; width: 3px; height: 16px; border-radius: 2px; opacity: 0.45; }
  .ttitle { flex: 1; min-width: 0; font-size: 13.5px; }
</style>
