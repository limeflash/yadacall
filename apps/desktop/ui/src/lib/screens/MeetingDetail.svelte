<script lang="ts">
  import { onMount, onDestroy, tick } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import Avatar from "../kit/Avatar.svelte";
  import Button from "../kit/Button.svelte";
  import IconButton from "../kit/IconButton.svelte";
  import Segmented from "../kit/Segmented.svelte";
  import SectionLabel from "../kit/SectionLabel.svelte";
  import PlayerBar from "./PlayerBar.svelte";
  import { api } from "../api";
  import type { MeetingDetail, Turn } from "../api";
  import { t, plural } from "../i18n/index.svelte";
  import { app } from "../state.svelte";
  import { clock, minutes, timeRange } from "../format";
  import { go, replace } from "../router.svelte";
  import { Player } from "../player.svelte";
  import { isNamed, speakerIndex, speakerName } from "../speakers";
  import { toast } from "../toast.svelte";
  import { isMac } from "../platform";

  let { id, tab, at }: { id: string; tab: "summary" | "transcript"; at?: number } = $props();

  let detail = $state<MeetingDetail | null>(null);
  let player = $state<Player | null>(null);
  let menuOpen = $state(false);
  let peopleOpen = $state(false);
  let renaming = $state(false);
  let titleDraft = $state("");
  let confirmDelete = $state(false);
  let namesOpen = $state(false);
  let nameDrafts = $state<Record<string, string>>({});
  let searchOpen = $state(false);
  let query = $state("");
  let hitIndex = $state(0);
  let busy = $state(false);
  let question = $state("");
  let scroller: HTMLElement | undefined = $state();

  async function load() {
    const d = await api.getMeeting(id);
    detail = d;
    if (!player) {
      player = new Player(Math.max(d.meeting.durationMs, d.turns.at(-1)?.endMs ?? 0), d.audioUrl);
      if (at != null) { player.seek(at, false); }
    }
  }
  onMount(load);
  onDestroy(() => player?.destroy());
  let lastRev = app.rev;
  $effect(() => { const r = app.rev; if (r !== lastRev) { lastRev = r; load(); } });
  $effect(() => { if (at != null && player) { player.seek(at, tab === "transcript"); scrollToActive(); } });

  const m = $derived(detail?.meeting);
  const turns = $derived(detail?.turns ?? []);
  const summary = $derived(detail?.summary);
  const unnamed = $derived.by(() => {
    if (!m) return [] as string[];
    return [...new Set(turns.map((x) => x.speaker))].filter((k) => !isNamed(m, k));
  });
  const speakers = $derived.by(() => {
    if (!m) return [];
    const total = turns.reduce((s, x) => s + (x.endMs - x.startMs), 0) || 1;
    const keys = [...new Set(turns.map((x) => x.speaker))].sort((a, b) => speakerIndex(a) - speakerIndex(b));
    return keys.map((k) => ({
      key: k, name: speakerName(m, k), named: isNamed(m, k), idx: speakerIndex(k),
      pct: Math.round((turns.filter((x) => x.speaker === k).reduce((s, x) => s + (x.endMs - x.startMs), 0) / total) * 100),
    }));
  });
  const namedSpeakers = $derived(speakers.filter((s) => s.named));
  const modelLabel = $derived.by(() => {
    const raw = summary?.model ?? "";
    const name = raw.split(":")[1] ?? raw;
    if (!name) return "—";
    return name.replace(/^claude-/, "").replace(/-/g, " ").replace(/\b\w/g, (c) => c.toUpperCase()).replace(/ (\d) (\d)/, " $1.$2");
  });

  // ---- transcript ---------------------------------------------------------------------------------
  type Item = { kind: "turn"; turn: Turn; i: number } | { kind: "mark"; at: number; label?: string };
  const items = $derived.by(() => {
    const out: Item[] = [];
    const marks = [...(m?.bookmarks ?? [])].sort((a, b) => a.atMs - b.atMs);
    let mi = 0;
    turns.forEach((tr, i) => {
      while (mi < marks.length && marks[mi].atMs <= tr.startMs) { out.push({ kind: "mark", at: marks[mi].atMs, label: marks[mi].label }); mi++; }
      out.push({ kind: "turn", turn: tr, i });
    });
    while (mi < marks.length) { out.push({ kind: "mark", at: marks[mi].atMs, label: marks[mi].label }); mi++; }
    return out;
  });
  const activeTurn = $derived(player && player.touched ? turns.findIndex((x) => player!.pos >= x.startMs && player!.pos < x.endMs) : -1);

  const needle = $derived(query.trim().toLowerCase());
  const hits = $derived(needle ? turns.map((x, i) => (x.text.toLowerCase().includes(needle) ? i : -1)).filter((i) => i >= 0) : []);
  $effect(() => { needle; hitIndex = 0; });
  $effect(() => { if (hits.length && searchOpen) scrollToTurn(hits[Math.min(hitIndex, hits.length - 1)]); });

  function parts(text: string, q: string): { s: string; hit: boolean }[] {
    if (!q) return [{ s: text, hit: false }];
    const out: { s: string; hit: boolean }[] = [];
    const low = text.toLowerCase();
    let p = 0, i: number;
    while ((i = low.indexOf(q, p)) >= 0) { if (i > p) out.push({ s: text.slice(p, i), hit: false }); out.push({ s: text.slice(i, i + q.length), hit: true }); p = i + q.length; }
    if (p < text.length) out.push({ s: text.slice(p), hit: false });
    return out;
  }
  function words(tr: Turn): { w: string; c: string }[] {
    const active = player?.playing && player.pos >= tr.startMs && player.pos < tr.endMs;
    const said = Math.floor((((player?.pos ?? 0) - tr.startMs) / 1000) / 0.42);
    return tr.text.split(" ").map((w, k) => ({ w, c: active && k >= said ? "var(--muted)" : "var(--text)" }));
  }

  async function scrollToTurn(i: number) {
    await tick();
    scroller?.querySelector(`[data-turn="${i}"]`)?.scrollIntoView({ block: "center", behavior: "smooth" });
  }
  async function scrollToActive() { await tick(); if (activeTurn >= 0) scrollToTurn(activeTurn); }

  function jump(ms: number | undefined) {
    if (ms == null || !player) return;
    player.seek(ms, true);
    if (tab !== "transcript") replace({ name: "meeting", id, tab: "transcript" });
    tick().then(() => { const i = turns.findIndex((x) => ms >= x.startMs && ms < x.endMs); if (i >= 0) scrollToTurn(i); });
  }

  function setTab(v: "summary" | "transcript") { menuOpen = false; replace({ name: "meeting", id, tab: v }); }

  // ---- actions ------------------------------------------------------------------------------------
  async function toggleTask(i: number) {
    if (!detail?.summary) return;
    const tk = detail.summary.tasks[i];
    tk.done = !tk.done;
    await api.setTaskDone(id, i, tk.done);
    app.openTasks += tk.done ? -1 : 1;
  }
  function startRename() { menuOpen = false; titleDraft = m?.title ?? ""; renaming = true; tick().then(() => document.getElementById("title-edit")?.focus()); }
  async function commitRename() {
    renaming = false;
    const v = titleDraft.trim();
    if (v && m && v !== m.title) { await api.renameMeeting(id, v); m.title = v; app.rev++; }
  }
  function openSearch() { menuOpen = false; if (tab !== "transcript") setTab("transcript"); searchOpen = true; tick().then(() => document.getElementById("find")?.focus()); }
  function closeSearch() { searchOpen = false; query = ""; }
  async function regenerate() {
    menuOpen = false; busy = true;
    try { const s = await api.regenerateSummary(id); if (detail) detail.summary = s; toast(t("Итоги пересобраны")); } catch (e) { toast.error(String(e)); }
    busy = false;
  }
  async function doExport(fmt: "md" | "json") {
    menuOpen = false;
    try { const p = await api.exportMeeting(id, fmt); toast(t("Сохранено: {path}", { path: p })); } catch (e) { toast.error(String(e)); }
  }
  async function copyClaude() {
    menuOpen = false;
    try { await navigator.clipboard.writeText(await api.copyForClaude(id)); toast(t("Расшифровка и итоги в буфере — в Markdown")); } catch { toast.error(t("Не удалось скопировать — скопируй вручную")); }
  }
  async function del() {
    if (!confirmDelete) { confirmDelete = true; return; }
    menuOpen = false; confirmDelete = false;
    await api.deleteMeeting(id); app.rev++; go({ name: "meetings" });
    toast(t("Встреча удалена"));
  }
  function openNames() { menuOpen = false; nameDrafts = Object.fromEntries(speakers.filter((s) => s.key !== "me").map((s) => [s.key, s.named ? s.name : ""])); namesOpen = true; }
  async function saveNames() {
    for (const [k, v] of Object.entries(nameDrafts)) if (v.trim() && v.trim() !== (m ? speakerName(m, k) : "")) await api.renameSpeaker(id, k, v.trim());
    namesOpen = false; await load();
  }
  function askAbout(e: SubmitEvent) { e.preventDefault(); go({ name: "ask", q: question.trim() || undefined }); }

  function onKey(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "f") { e.preventDefault(); openSearch(); }
    else if (e.key === "Escape") { if (searchOpen) closeSearch(); else if (menuOpen) menuOpen = false; else if (namesOpen) namesOpen = false; }
  }
  function onDocClick(e: MouseEvent) {
    if (menuOpen && !(e.target as HTMLElement).closest(".menu, .more")) { menuOpen = false; confirmDelete = false; }
  }

  const tasksDone = (n: string | undefined) => (n && n !== "me" ? (m ? speakerName(m, n) : n) : t("вы"));
  function dueLabel(due?: string) {
    if (!due) return "";
    return new Intl.DateTimeFormat(app.settings?.uiLanguage === "en" ? "en-GB" : "ru-RU", { day: "numeric", month: "short" }).format(new Date(due + "T12:00:00")).replace(".", "");
  }
</script>

<svelte:window onkeydown={onKey} onclick={onDocClick} />

<main>
  <section class="yc-sheet sheet" aria-label={t("Встреча")}>
    <button type="button" class="grab" aria-label={t("Закрыть встречу")} onclick={() => go({ name: "meetings" })}></button>

    {#if menuOpen && m}
      <div class="yc-pop menu" role="menu" aria-label={t("Действия со встречей")}>
        <button type="button" role="menuitem" class="yc-mi mi" onclick={startRename}><span class="mi-ic"><Icon name="text" size={16} /></span><span class="grow">{t("Переименовать")}</span></button>
        <button type="button" role="menuitem" class="yc-mi mi" onclick={openSearch}><span class="mi-ic"><Icon name="search" size={16} /></span><span class="grow">{t("Найти в расшифровке")}</span><span class="mg-mono dim">{isMac ? "⌘F" : "Ctrl+F"}</span></button>
        <button type="button" role="menuitem" class="yc-mi mi" onclick={regenerate}><span class="mi-ic"><Icon name="refresh" size={16} /></span><span class="grow">{t("Пересобрать итоги")}</span></button>
        <span aria-hidden="true" class="sep"></span>
        <button type="button" role="menuitem" class="yc-mi mi" onclick={openNames}><span class="mi-ic"><Icon name="people" size={16} /></span><span class="grow">{t("Назвать спикеров")}</span>{#if unnamed.length}<span class="new">{t("{n} новый", { n: unnamed.length })}</span>{/if}</button>
        <button type="button" role="menuitem" class="yc-mi mi" onclick={() => { menuOpen = false; toast(t("Скоро: разделение реплик вручную")); }}><span class="mi-ic"><Icon name="layers" size={16} /></span><span class="grow">{t("Разделить реплики")}</span></button>
        <span aria-hidden="true" class="sep"></span>
        <button type="button" role="menuitem" class="yc-mi mi" onclick={() => { menuOpen = false; go({ name: "settings" }); }}><span class="mi-ic"><Icon name="globe" size={16} /></span><span class="grow">{t("Язык итогов")}</span><span class="dim2">{summary?.language === "ru" ? t("Авто") : (summary?.language ?? t("Авто"))}</span></button>
        <button type="button" role="menuitem" class="yc-mi mi" onclick={() => { menuOpen = false; go({ name: "settings" }); }}><span class="mi-ic"><Icon name="sparkle" size={16} /></span><span class="grow">{t("Модель")}</span><span class="mg-mono dim">{modelLabel}</span></button>
        <span aria-hidden="true" class="sep"></span>
        <button type="button" role="menuitem" class="yc-mi mi" onclick={() => doExport("md")}><span class="mi-ic"><Icon name="share" size={16} /></span><span class="grow">{t("Экспорт")}</span><span class="dim2">MD</span></button>
        <button type="button" role="menuitem" class="yc-mi mi" onclick={() => doExport("json")}><span class="mi-ic"><Icon name="code" size={16} /></span><span class="grow">{t("Экспорт")} JSON</span></button>
        <button type="button" role="menuitem" class="yc-mi mi" onclick={copyClaude}><span class="mi-ic"><Icon name="copy" size={16} /></span><span class="grow">{t("Скопировать для Claude")}</span></button>
        <button type="button" role="menuitem" class="yc-mi mi" onclick={() => { menuOpen = false; api.revealMeeting(id); }}><span class="mi-ic"><Icon name="externalLink" size={16} /></span><span class="grow">{t("Показать в папке")}</span></button>
        <span aria-hidden="true" class="sep"></span>
        <button type="button" role="menuitem" class="yc-mi mi danger" onclick={del}><Icon name="trash" size={16} /><span class="grow">{confirmDelete ? t("Точно удалить?") : t("Удалить")}</span></button>
      </div>
    {/if}

    {#if namesOpen}
      <div class="veil" role="presentation" onclick={() => (namesOpen = false)}></div>
      <div class="yc-pop names" role="dialog" aria-label={t("Назвать спикеров")}>
        <h2>{t("Назвать спикеров")}</h2>
        <p>{t("Имена сохранятся во встрече и подставятся в итоги и задачи.")}</p>
        {#each Object.keys(nameDrafts) as k (k)}
          <label class="nrow"><Avatar name={nameDrafts[k] || "?"} index={speakerIndex(k)} size={26} />
            <input placeholder={t("Гость")} bind:value={nameDrafts[k]} /></label>
        {/each}
        <div class="nbtns"><Button size="md" variant="ghost" onclick={() => (namesOpen = false)}>{t("Отмена")}</Button><Button size="md" variant="primary" onclick={saveNames}>{t("Сохранить")}</Button></div>
      </div>
    {/if}

    <div class="yc-scroll" style="flex: 1; min-height: 0" bind:this={scroller}>
      {#if m}
        <div class="col">
          <header>
            <div class="head">
              {#if renaming}
                <input id="title-edit" class="title-edit" bind:value={titleDraft} onblur={commitRename} onkeydown={(e) => { if (e.key === "Enter") commitRename(); if (e.key === "Escape") renaming = false; }} />
              {:else}
                <h1 class="yc-balance">{m.title}</h1>
              {/if}
              <div class="mg-mono meta">{timeRange(m.startedAt, m.durationMs)} · {minutes(m.durationMs)}</div>
            </div>
            <IconButton variant="square" class="more" aria-label={t("Ещё")} onclick={() => (menuOpen = !menuOpen)}>
              <svg class="yc-icf" width="16" height="16" viewBox="0 0 24 24" style="stroke-width: 0"><circle cx="5" cy="12" r="1.9" /><circle cx="12" cy="12" r="1.9" /><circle cx="19" cy="12" r="1.9" /></svg>
            </IconButton>
          </header>

          <Segmented variant="thumb" label={t("Раздел встречи")} value={tab} onchange={setTab} options={[{ value: "summary", label: t("Итоги") }, { value: "transcript", label: t("Расшифровка") }]} />

          {#if m.status.state === "processing"}
            <p class="note">{t("Идёт обработка… {p}%", { p: Math.round(m.status.progress * 100) })}</p>
          {/if}

          {#if tab === "summary"}
            <div class="yc-in stack">
              {#if speakers.length}
                <div class="people">
                  <button type="button" class="people-btn" aria-expanded={peopleOpen} onclick={() => (peopleOpen = !peopleOpen)}>
                    <span class="avs">{#each namedSpeakers.filter((s) => s.key !== "me").slice(0, 3) as s (s.key)}<span class="avwrap"><Avatar name={s.name} index={s.idx} size={22} /></span>{/each}</span>
                    <span class="names-line">{namedSpeakers.filter((s) => s.key !== "me").map((s) => s.name).join(", ") || t("Вы")}{#if unnamed.length}{" "}<span style="color: var(--muted)">{t("и гость")}</span>{/if}</span>
                    <span class="chev" style="transform: rotate({peopleOpen ? 90 : 0}deg)"><Icon name="chevronRight" size={14} /></span>
                  </button>
                  {#if peopleOpen}
                    <div class="yc-in talk">
                      <div style="font-size: 12px; color: var(--muted)">{t("Кто сколько говорил")}</div>
                      {#each speakers as s (s.key)}
                        <div class="trow">
                          <span class="tname" style:color={s.named ? undefined : "var(--muted)"}>{s.name}</span>
                          {#if s.named}
                            <span class="tbar"><span style="width: {s.pct}%; background: {['var(--accent-lavender)','var(--accent-sky)','var(--accent-sand)','var(--accent-rose)','var(--muted)'][s.idx]}"></span></span>
                            <span class="mg-mono yc-num tpct">{s.pct}%</span>
                          {:else}
                            <span style="flex: 1; font-size: 12px; color: var(--muted)">{t("голос не узнан")}</span>
                            <Button size="xs" onclick={openNames}>{t("назвать")}</Button>
                          {/if}
                        </div>
                      {/each}
                    </div>
                  {/if}
                </div>
              {/if}

              {#if summary}
                <section class="sec">
                  <SectionLabel as="h2">{t("О чём встреча")}</SectionLabel>
                  {#each summary.about as p, i (i)}<p class="yc-pretty prose">{p}</p>{/each}
                </section>
                {#if summary.decisions.length}
                  <section class="sec">
                    <SectionLabel as="h2">{t("Договорённости")}</SectionLabel>
                    <ul class="bul">
                      {#each summary.decisions as d, i (i)}
                        <li><span aria-hidden="true" class="dot"></span><button type="button" class="yc-dash grow" onclick={() => jump(d.atMs)}>{d.text}</button>{#if d.atMs != null}<span class="mg-mono ts">{clock(d.atMs)}</span>{/if}</li>
                      {/each}
                    </ul>
                  </section>
                {/if}
                {#if summary.tasks.length}
                  <section class="sec" style="gap: 6px">
                    <SectionLabel as="h2">{t("Задачи")}</SectionLabel>
                    {#each summary.tasks as tk, i (i)}
                      <label class="task">
                        <input type="checkbox" class="yc-check" checked={tk.done} onchange={() => toggleTask(i)} />
                        <span class={tk.done ? "yc-task-done" : ""} style="flex: 1; min-width: 0; font-size: 15px">{tk.text}</span>
                        <span style="flex: none; font-size: 12px; color: var(--muted)">{tasksDone(tk.owner)}{tk.due ? " · " + dueLabel(tk.due) : ""}</span>
                      </label>
                    {/each}
                  </section>
                {/if}
                {#if summary.openQuestions.length}
                  <section class="sec">
                    <SectionLabel as="h2">{t("Осталось открытым")}</SectionLabel>
                    <ul class="bul">
                      {#each summary.openQuestions as q, i (i)}
                        <li><span aria-hidden="true" class="dot"></span><button type="button" class="yc-dash grow" onclick={() => jump(undefined)}>{q}</button></li>
                      {/each}
                    </ul>
                  </section>
                {/if}
              {:else if m.status.state === "ready"}
                <p class="note">{busy ? t("Собираю итоги…") : t("Итогов пока нет. Их можно собрать из меню ⋯ → «Пересобрать итоги».")}</p>
              {/if}
              {#if busy}<p class="note">{t("Собираю итоги…")}</p>{/if}
            </div>
          {:else}
            <div class="yc-in stack" style="gap: 12px">
              {#if searchOpen}
                <div class="find yc-in">
                  <Icon name="search" size={14} />
                  <input id="find" bind:value={query} placeholder={t("Найти в расшифровке")} onkeydown={(e) => { if (e.key === "Enter") hitIndex = hits.length ? (hitIndex + (e.shiftKey ? hits.length - 1 : 1)) % hits.length : 0; }} />
                  <span class="mg-mono cnt">{needle ? (hits.length ? `${Math.min(hitIndex, hits.length - 1) + 1} / ${hits.length}` : t("нет")) : ""}</span>
                  <IconButton variant="flat" aria-label={t("Назад")} disabled={!hits.length} onclick={() => (hitIndex = (hitIndex + hits.length - 1) % hits.length)}><Icon name="arrowUp" size={14} /></IconButton>
                  <IconButton variant="flat" aria-label={t("Дальше")} disabled={!hits.length} onclick={() => (hitIndex = (hitIndex + 1) % hits.length)}><Icon name="arrowDown" size={14} /></IconButton>
                  <IconButton variant="flat" aria-label={t("Закрыть")} onclick={closeSearch}><Icon name="x" size={14} /></IconButton>
                </div>
              {/if}
              {#if turns.length === 0}
                <p class="note">{m.status.state === "processing" ? t("Расшифровка ещё собирается.") : t("В этой встрече не нашлось речи.")}</p>
              {/if}
              {#each items as it, k (k)}
                {#if it.kind === "mark"}
                  <div style="display: flex; justify-content: center"><span class="mark"><svg class="yc-ic" width="12" height="12" viewBox="0 0 24 24"><path d="M7 4h10v16l-5-4-5 4z" /></svg>{clock(it.at)} · {t("твоя закладка")}{it.label ? ": " + it.label : ""}</span></div>
                {:else}
                  {@const tr = it.turn}
                  {@const mine = tr.speaker === "me"}
                  {@const named = isNamed(m, tr.speaker)}
                  {@const on = activeTurn === it.i}
                  {@const ws = !needle ? words(tr) : null}
                  <div data-turn={it.i} class="trow2" class:mine style:padding-left={mine ? "64px" : undefined} style:padding-right={mine ? undefined : "64px"}>
                    {#if !mine}
                      {#if named}<Avatar name={speakerName(m, tr.speaker)} index={speakerIndex(tr.speaker)} size={28} />
                      {:else}<span aria-hidden="true" class="qa">?</span>{/if}
                    {/if}
                    <div class="bub {mine ? 'b-mine' : named ? 'b-them' : 'b-guest'} {on ? 'yc-bub-on' : ''}" class:hit-on={hits[Math.min(hitIndex, hits.length - 1)] === it.i && searchOpen}>
                      {#if !mine && named}
                        <button type="button" class="yc-dash who" onclick={openNames}>{speakerName(m, tr.speaker)}</button>
                      {:else if !mine}
                        <span class="who2"><span>{t("Гость")}</span><Button size="xs" onclick={openNames}>{t("кто это?")}</Button></span>
                      {/if}
                      <p class="yc-pretty txt">
                        {#if ws}{#each ws as w, wi (wi)}<span style="color: {w.c}; transition: color 0.15s">{w.w}</span>{" "}{/each}
                        {:else}{#each parts(tr.text, needle) as p, pi (pi)}{#if p.hit}<mark class="yc-hit">{p.s}</mark>{:else}{p.s}{/if}{/each}{/if}
                      </p>
                      <span class="tc">
                        {#if on && player?.playing}<span aria-hidden="true" class="eq"><span></span><span style="animation-delay: 0.2s"></span><span style="animation-delay: 0.4s"></span></span>{/if}
                        <button type="button" class="yc-tcx mg-mono tcb" onclick={() => player?.seek(tr.startMs, true)}>{clock(tr.startMs)}</button>
                      </span>
                    </div>
                  </div>
                {/if}
              {/each}
            </div>
          {/if}
        </div>
      {/if}
    </div>

    <div aria-hidden="true" class="fade"></div>
    <div class="dock">
      {#if player && (tab === "transcript" || player.touched)}
        <PlayerBar {player} {turns} />
      {/if}
      <form class="askbar" onsubmit={askAbout}>
        <label style="flex: 1; min-width: 0; display: flex"><span class="mg-sr">{t("Вопрос о встрече")}</span>
          <input type="text" bind:value={question} placeholder={t("Что обещал Борис?")} /></label>
        <button type="submit" aria-label={t("Спросить о встрече")} class="yc-press send"><Icon name="arrowUp" size={16} /></button>
      </form>
    </div>
  </section>
</main>

<style>
  main { flex: 1 1 520px; min-width: 0; display: flex; flex-direction: column; min-height: 0; padding: 10px 10px 0; }
  .sheet { flex: 1; min-height: 0; position: relative; display: flex; flex-direction: column; border: 1px solid var(--border); border-bottom: 0; border-radius: 22px 22px 0 0; background: var(--surface); overflow: hidden; }
  .grab { flex: none; display: block; width: 36px; height: 4px; margin: 9px auto 0; padding: 0; border: 0; border-radius: 2px; background: var(--faint); cursor: pointer; }
  .menu { position: absolute; z-index: 5; top: 64px; right: max(16px, calc(50% - 304px)); width: 280px; box-sizing: border-box; padding: 6px; border: 1px solid var(--border); border-radius: 16px; background: var(--surface); box-shadow: var(--shadow-pop); display: flex; flex-direction: column; max-height: calc(100% - 80px); overflow-y: auto; }
  .mi { display: flex; align-items: center; gap: 10px; height: 36px; flex: none; padding: 0 10px; border: 0; border-radius: 10px; background: transparent; color: var(--text); font-size: 13px; text-align: left; }
  .mi.hl { background: var(--surface-2); }
  .mi.danger { color: var(--danger-text); }
  .mi-ic { display: inline-flex; color: var(--muted); }
  .grow { flex: 1; }
  .dim { font-size: 11px; color: var(--muted); }
  .dim2 { font-size: 12px; color: var(--muted); }
  .new { font-size: 12px; color: var(--accent-text); }
  .sep { flex: none; height: 1px; margin: 5px 4px; background: var(--border); }
  .veil { position: absolute; inset: 0; z-index: 6; background: var(--backdrop); }
  .names { position: absolute; z-index: 7; left: 50%; top: 120px; transform: translateX(-50%); width: min(380px, calc(100% - 32px)); box-sizing: border-box; padding: 18px; border: 1px solid var(--border); border-radius: 18px; background: var(--surface); box-shadow: var(--shadow-pop); display: flex; flex-direction: column; gap: 10px; transform-origin: top center; }
  .names h2 { margin: 0; font-size: 16px; font-weight: 800; letter-spacing: -0.02em; }
  .names p { margin: 0 0 4px; font-size: 12px; color: var(--muted); }
  .nrow { display: flex; align-items: center; gap: 10px; }
  .nrow input, .title-edit, .find input { flex: 1; min-width: 0; height: 36px; box-sizing: border-box; padding: 0 12px; border: 1px solid var(--border); border-radius: 11px; background: var(--bg); color: var(--text); font: inherit; font-size: 14px; }
  .nbtns { display: flex; justify-content: flex-end; gap: 8px; margin-top: 6px; }
  .col { max-width: 640px; box-sizing: border-box; margin: 0 auto; padding: 12px 28px 180px; display: flex; flex-direction: column; gap: 18px; }
  header { display: flex; align-items: flex-start; gap: 16px; }
  .head { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 4px; }
  h1 { margin: 0; font-size: 26px; line-height: 1.2; font-weight: 800; letter-spacing: -0.035em; }
  .title-edit { height: 40px; font-size: 22px; font-weight: 800; letter-spacing: -0.03em; }
  .meta { font-size: 12px; color: var(--muted); }
  .note { margin: 0; font-size: 13px; color: var(--muted); }
  .stack { display: flex; flex-direction: column; gap: 24px; padding-top: 4px; }
  .people { border-radius: 16px; background: var(--surface-2); }
  .people-btn { display: flex; align-items: center; gap: 12px; width: 100%; padding: 9px 14px; border: 0; border-radius: 16px; background: transparent; color: var(--text); text-align: left; }
  .avs { display: inline-flex; align-items: center; padding-left: 6px; }
  .avwrap { display: inline-flex; margin-left: -6px; padding: 2px; border-radius: 50%; background: var(--surface-2); }
  .names-line { flex: 1; min-width: 0; font-size: 14px; }
  .chev { display: inline-flex; color: var(--muted); transition: transform 0.3s var(--ease-spring); }
  .talk { display: flex; flex-direction: column; gap: 9px; padding: 2px 16px 14px; }
  .trow { display: flex; align-items: center; gap: 10px; }
  .tname { width: 56px; font-size: 13px; }
  .tbar { flex: 1; height: 5px; border-radius: 3px; background: var(--bg); overflow: hidden; display: block; }
  .tbar span { display: block; height: 100%; border-radius: 3px; }
  .tpct { width: 32px; text-align: right; font-size: 12px; color: var(--muted); }
  .sec { display: flex; flex-direction: column; gap: 8px; }
  .prose { margin: 0; font-size: 15px; line-height: 1.65; }
  .bul { margin: 0; padding: 0; list-style: none; display: flex; flex-direction: column; gap: 8px; }
  .bul li { display: flex; align-items: baseline; gap: 10px; font-size: 15px; line-height: 1.55; }
  .dot { flex: none; width: 4px; height: 4px; border-radius: 50%; background: var(--muted); transform: translateY(-4px); }
  .ts { flex: none; font-size: 11px; color: var(--muted); }
  .task { display: flex; align-items: center; gap: 12px; padding: 4px 0; cursor: pointer; }
  .find { display: flex; align-items: center; gap: 6px; padding: 4px 6px 4px 12px; border: 1px solid var(--border); border-radius: 14px; background: var(--bg); color: var(--muted); }
  .find input { border: 0; background: transparent; height: 32px; padding: 0 6px; }
  .cnt { font-size: 11px; min-width: 40px; text-align: right; }
  .trow2 { display: flex; align-items: flex-end; gap: 9px; }
  .trow2.mine { justify-content: flex-end; }
  .bub { min-width: 0; display: flex; flex-direction: column; gap: 2px; border: 1px solid transparent; }
  .b-mine { padding: 9px 13px 7px; border-radius: 18px 18px 6px 18px; background: var(--accent-soft); }
  .b-them { padding: 8px 13px 7px; border-radius: 18px 18px 18px 6px; background: var(--surface-2); }
  .b-guest { padding: 8px 13px 7px; gap: 3px; border: 1px dashed var(--faint); border-radius: 18px 18px 18px 6px; background: var(--surface-2); }
  .hit-on { outline: 2px solid var(--accent-line); outline-offset: 2px; }
  .who { align-self: flex-start; font-size: 12px; font-weight: 700; color: var(--muted); }
  .who2 { display: flex; align-items: center; gap: 8px; font-size: 12px; font-weight: 700; color: var(--muted); }
  .qa { width: 28px; height: 28px; box-sizing: border-box; flex: none; border-radius: 50%; border: 1.5px dashed var(--faint); color: var(--muted); display: grid; place-items: center; font-size: 12px; font-weight: 800; }
  .txt { margin: 0; font-size: 14px; line-height: 1.55; }
  .tc { align-self: flex-end; display: inline-flex; align-items: center; gap: 6px; }
  .tcb { padding: 0; border: 0; background: none; font-size: 11px; color: var(--muted); cursor: pointer; }
  .eq { display: flex; align-items: flex-end; gap: 2px; height: 10px; }
  .eq span { display: block; width: 2px; height: 3px; border-radius: 1px; background: var(--accent-text); animation: ycEq 0.7s ease-in-out infinite; }
  .mark { display: inline-flex; align-items: center; gap: 6px; height: 24px; padding: 0 10px; border-radius: 12px; background: var(--tone-sand-bg); color: var(--tone-sand); font-size: 12px; }
  .fade { position: absolute; left: 0; right: 10px; bottom: 0; height: 150px; pointer-events: none; background: linear-gradient(to bottom, transparent, var(--surface) 58%); }
  .dock { position: absolute; left: 24px; right: 24px; bottom: 16px; max-width: 600px; margin: 0 auto; display: flex; flex-direction: column; gap: 8px; }
  .askbar { display: flex; align-items: center; gap: 10px; height: 48px; box-sizing: border-box; padding: 0 5px 0 18px; border: 1px solid var(--border); border-radius: 24px; background: var(--dock); backdrop-filter: blur(16px); -webkit-backdrop-filter: blur(16px); box-shadow: var(--shadow-dock); }
  .askbar input { flex: 1; min-width: 0; height: 36px; margin: 0; padding: 0; border: 0; background: transparent; color: var(--text); font: inherit; font-size: 14px; outline-offset: 6px; }
  .send { flex: none; width: 36px; height: 36px; border: 0; border-radius: 50%; background: var(--surface-2); color: var(--muted); display: grid; place-items: center; }
</style>
