<script lang="ts">
  /**
   * The model browser: search, tabs by purpose, filter chips, grouped cards.
   *  - `manage`: the Models screen — download / pause / remove / measure / make primary.
   *  - `pick`: a single-select chooser — rows are options, picking only reports the id (`onPick`), nothing downloads.
   * Data comes from the shared `mstore`; the component attaches to the engine stream while mounted.
   */
  import { onMount } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import Button from "../kit/Button.svelte";
  import IconButton from "../kit/IconButton.svelte";
  import StatusPill from "../kit/StatusPill.svelte";
  import SectionLabel from "../kit/SectionLabel.svelte";
  import Mascot from "../kit/Mascot.svelte";
  import Tabs from "../kit/Tabs.svelte";
  import ToggleChips from "../kit/ToggleChips.svelte";
  import SearchField from "../kit/SearchField.svelte";
  import VendorMark from "../kit/VendorMark.svelte";
  import { api } from "../api";
  import type { ModelEntry, ModelKind, Verdict } from "../api";
  import { i18n, locale, t } from "../i18n/index.svelte";
  import { bytes, mbps } from "../format";
  import { toast } from "../toast.svelte";
  import { attachModels, diskBytes, isRequired, modelAct, mstore, onModelDone, removeModel } from "../modelsStore.svelte";
  import {
    TAB_ORDER, KIND_ORDER, FILTER_ORDER, applyFilters, chipMatches, groupsFor, haystack, highlight, matchesTerms, paramsLabel, sorted, terms,
    type FilterKey, type Group, type Tab,
  } from "../models";

  let { kinds = KIND_ORDER, mode = "manage", selectedId = null, preselectId = null, onPick, persist = mode === "manage", autofocusSearch = false }: {
    /** Which kinds to list; a single kind hides the tab bar. */
    kinds?: ModelKind[];
    mode?: "manage" | "pick";
    /** Pick mode: the chosen model. */
    selectedId?: string | null;
    /** Pick mode: shown as chosen while nothing is (the recommendation). */
    preselectId?: string | null;
    onPick?: (id: string) => void;
    /** Remember tab + filters in localStorage. */
    persist?: boolean;
    autofocusSearch?: boolean;
  } = $props();

  const uid = "mc" + Math.random().toString(36).slice(2, 7);
  const pick = $derived(mode === "pick");
  const tabList = $derived<Tab[]>(kinds.length > 1 ? (["all", ...KIND_ORDER.filter((k) => kinds.includes(k))] as Tab[]) : [kinds[0]]);

  let benching = $state<string | null>(null);
  let measured = $state<Record<string, number>>({});

  onMount(() => {
    const detach = attachModels();
    const offDone = onModelDone((m) => { if (mode === "manage" && kinds.includes(m.kind)) toast(t("{name} скачана и проверена", { name: m.name })); });
    const io = typeof IntersectionObserver !== "undefined" && sentinel ? new IntersectionObserver(([en]) => (stuck = !en.isIntersecting)) : null;
    if (io && sentinel) io.observe(sentinel);
    if (autofocusSearch) searchEl?.focus();
    return () => { detach(); offDone(); io?.disconnect(); };
  });

  // ---- browsing state: tab + filters are remembered (manage), the search is not ---------------------------------
  const VIEW_KEY = "yc.models.view";
  function restore(): { tab: Tab; filters: Set<FilterKey> } {
    if (!persist) return { tab: tabList[0], filters: new Set() };
    try {
      const v = JSON.parse(localStorage.getItem(VIEW_KEY) ?? "{}");
      return {
        tab: tabList.includes(v.tab) ? v.tab : tabList[0],
        filters: new Set<FilterKey>(Array.isArray(v.filters) ? v.filters.filter((x: unknown): x is FilterKey => FILTER_ORDER.includes(x as FilterKey)) : []),
      };
    } catch { return { tab: tabList[0], filters: new Set() }; }
  }
  const saved = restore();
  let tab = $state<Tab>(saved.tab);
  let filters = $state<Set<FilterKey>>(saved.filters);
  let q = $state("");
  let searchEl = $state<HTMLInputElement | null>(null);
  let sentinel = $state<HTMLElement | null>(null);
  let stuck = $state(false);
  /** Kinds opened past their first three cards in the "all" tab. */
  let more = $state<Set<ModelKind>>(new Set());
  let wontOpen = $state(false);
  function save() { if (!persist) return; try { localStorage.setItem(VIEW_KEY, JSON.stringify({ tab, filters: [...filters] })); } catch { /* storage may be blocked */ } }
  function setTab(v: Tab) { tab = v; save(); }
  function toggleFilter(k: FilterKey) { const n = new Set(filters); if (n.has(k)) n.delete(k); else n.add(k); filters = n; save(); }
  function resetAll() { q = ""; filters = new Set(); save(); }
  function toggleMore(k: ModelKind) { const n = new Set(more); if (n.has(k)) n.delete(k); else n.add(k); more = n; }

  /** `/` or ⌘F / Ctrl+F jump to the search box. */
  function onKey(e: KeyboardEvent) {
    const el = e.target as HTMLElement | null;
    const typing = !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
    const find = (e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === "f";
    if (find || (e.key === "/" && !typing && !e.metaKey && !e.ctrlKey && !e.altKey)) { e.preventDefault(); searchEl?.focus(); searchEl?.select(); }
  }

  const kindInfo: Record<ModelKind, { title: string; sub: string }> = {
    asr: { title: "Расшифровка", sub: "Речь → текст на компьютере" },
    llm: { title: "Итоги (LLM)", sub: "Итоги, задачи и ответы на вопросы без облака" },
    voice: { title: "Голоса", sub: "Разметка по голосам и узнавание знакомых" },
    embed: { title: "Поиск", sub: "Смысловой поиск по встречам" },
  };
  /** The tab says "Итоги" in Russian; the shared English string for that word means the meeting summary itself. */
  const tabLabel = (v: Tab) => v === "all" ? t("Все") : v === "llm" ? (i18n.lang === "en" ? "Summaries" : "Итоги") : t(kindInfo[v].title);

  // ---- search → tab → filters ------------------------------------------------------------------------------
  const mine = $derived(mstore.models.filter((m) => kinds.includes(m.kind)));
  const ts = $derived(terms(q));
  const searched = $derived(mine.filter((m) => matchesTerms(haystack(m, t(m.blurb)), ts)));
  const filtered = $derived(applyFilters(searched, filters));
  const inTab = $derived(tab === "all" ? filtered : filtered.filter((m) => m.kind === tab));
  const tabs = $derived(tabList.map((v) => ({ value: v, label: tabLabel(v), count: v === "all" ? filtered.length : filtered.filter((m) => m.kind === v).length })));
  const filterLabel: Record<FilterKey, string> = { ru: "Русский", multi: "Мультиязычные", en: "English", fits: "Потянет этот компьютер", installed: "Установленные" };
  /** Each chip counts what you would get by switching it on, given the search, the tab and the other chips. */
  const chipOptions = $derived.by(() => {
    const base = tab === "all" ? searched : searched.filter((m) => m.kind === tab);
    const lang = new Set<FilterKey>(["ru", "multi", "en"]);
    return FILTER_ORDER.map((k) => ({
      value: k, label: t(filterLabel[k]),
      count: applyFilters(base, filters, lang.has(k) ? lang : new Set([k])).filter((m) => chipMatches(k, m)).length,
    }));
  });
  const narrowed = $derived(ts.length > 0 || filters.size > 0);
  const kindList = (kind: ModelKind) => sorted(inTab.filter((m) => m.kind === kind));
  const tabGroups = $derived<Group[]>(tab === "all" ? [] : groupsFor(tab, kindList(tab)));
  const FOLD = 3;
  const effectiveId = $derived(selectedId ?? preselectId ?? null);

  const verdictPill: Record<Verdict, { kind: "ok" | "blip" | "warn" | "bad"; label: string }> = {
    great: { kind: "ok", label: "Отлично" }, ok: { kind: "blip", label: "Нормально" }, slow: { kind: "warn", label: "Медленно" }, wontRun: { kind: "bad", label: "Не потянет" },
  };
  const verdictColor: Record<Verdict, string> = { great: "ok", ok: "blip", slow: "warning", wontRun: "danger" };
  const pctOf = (m: ModelEntry) => (m.download ? Math.floor((m.download.doneBytes / Math.max(1, m.download.totalBytes)) * 100) : 0);
  function etaLabel(s?: number) { if (!s) return ""; return s > 60 ? "~" + Math.ceil(s / 60) + " " + t("мин") : "~" + Math.max(1, s) + " " + t("с"); }
  const dec = (x: number) => x.toLocaleString(locale(), { maximumFractionDigits: 1 });
  function paramsText(m: ModelEntry) {
    if (!m.params) return "";
    if (m.activeParams) return t("{total}, активных {active}", { total: paramsLabel(m.params, dec), active: paramsLabel(m.activeParams, dec) });
    return paramsLabel(m.params, dec);
  }
  const metaLine = (m: ModelEntry) => [bytes(m.sizeBytes), paramsText(m), m.vendor, m.license].filter(Boolean).join(" · ");
  const langChips = (m: ModelEntry) => m.tags.includes("english-only") ? [] : m.languages.map((l) => (l === "ru" ? "RU" : l === "en" ? "EN" : t("Мульти")));

  async function bench(m: ModelEntry) {
    benching = m.id;
    try { const r = await api.modelBenchmark(m.id); measured[m.id] = r.realtimeFactor; toast(`${m.name}: ${measuredLabel(m, r.realtimeFactor)}`); } catch (e) { toast.error(e); }
    benching = null;
  }
  /** "измерено: ≈ 3.1× быстрее реального времени" — the unit depends on what the model does. */
  function measuredLabel(m: ModelEntry, v: number) {
    const x = Number(v.toFixed(1));
    if (m.kind === "embed") return t("измерено: ≈ {x} абзацев в секунду", { x });
    if (m.kind === "llm") return t("измерено: ≈ {x} токенов в секунду", { x });
    return t("измерено: ≈ {x}× быстрее реального времени", { x });
  }

  // ---- removal: confirmed inline on the row, never a modal ---------------------------------------------------
  let confirmId = $state<string | null>(null);
  let removing = $state(false);
  /** Another installed model of the same kind that can take over, best first. */
  const successor = (m: ModelEntry) => sorted(mstore.models.filter((x) => x.kind === m.kind && x.installed && x.id !== m.id && x.verdict !== "wontRun"))[0];
  /** The primary model of a kind that has a "primary" (speech, summaries). */
  const isPrimary = (m: ModelEntry) => m.installed && m.active && (m.kind === "asr" || m.kind === "llm");
  /** Speech recognition cannot be left empty: the only one has to stay until another is installed. */
  const lastAsr = (m: ModelEntry) => m.kind === "asr" && isPrimary(m) && !successor(m);
  const hasPartial = (m: ModelEntry) => !m.installed && !!m.download;
  async function confirmRemove(m: ModelEntry) {
    removing = true;
    const next = isPrimary(m) ? successor(m)?.id : undefined;
    const ok = await removeModel(m, next);
    removing = false;
    if (ok) { confirmId = null; toast(t("{name} удалена — освободилось {size}", { name: m.name, size: bytes(diskBytes(m)) })); }
  }
  const consequence = (m: ModelEntry) =>
    m.kind === "llm" ? t("Пока не выберешь другую модель или провайдера, итоги собираться не будут.")
    : m.kind === "voice" ? t("Без неё встречи не будут делиться по голосам.")
    : m.kind === "embed" ? t("Без неё перестанет работать смысловой поиск.") : "";

  /** Listbox navigation in pick mode: arrows move focus between options (Enter / Space chooses). */
  function optionKeys(e: KeyboardEvent) {
    if (!pick || (e.key !== "ArrowDown" && e.key !== "ArrowUp" && e.key !== "Home" && e.key !== "End")) return;
    const opts = [...(e.currentTarget as HTMLElement).querySelectorAll<HTMLElement>('[role="radio"]:not([aria-disabled="true"])')];
    const at = opts.indexOf(document.activeElement as HTMLElement);
    if (at < 0) return;
    e.preventDefault();
    const next = e.key === "Home" ? 0 : e.key === "End" ? opts.length - 1 : Math.min(opts.length - 1, Math.max(0, at + (e.key === "ArrowDown" ? 1 : -1)));
    opts[next]?.focus();
  }
</script>

<svelte:window onkeydown={onKey} />

{#snippet name(m: ModelEntry)}
  {#each highlight(m.name, ts) as p, i (i)}{#if p.hit}<mark>{p.s}</mark>{:else}{p.s}{/if}{/each}
{/snippet}

<!-- Who it is, what it is like and how it will run here: shared by both modes. -->
{#snippet identity(m: ModelEntry)}
  {@const lc = langChips(m)}
  <VendorMark vendor={m.vendor} />
  <span class="body" title={t(m.blurb)}>
    <span class="l1">
      <span class="nm mg-mono yc-ellipsis">{@render name(m)}</span>
      {#if m.recommended}<span class="tg tg-rec">{t("Рекомендуем")}</span>{/if}
      {#if m.active && m.installed && (m.kind === "asr" || m.kind === "llm")}<span class="yc-popin tag-main">{t("основная")}</span>{/if}
      {#each lc as c (c)}<span class="tg mg-mono">{c}</span>{/each}
    </span>
    <span class="meta">{metaLine(m)}</span>
    <span class="est"><span class="edot" style="background: var(--{verdictColor[m.verdict]})"></span>{measured[m.id] ? measuredLabel(m, measured[m.id]) : t(m.estimate)}</span>
    {#if m.tags.includes("ru-tuned") || m.tags.includes("moe") || m.tags.includes("english-only") || m.licenseCustom}
      <span class="traits">
        {#if m.tags.includes("ru-tuned")}<span class="tg tg-soft">{t("Русское дообучение")}</span>{/if}
        {#if m.tags.includes("moe")}<span class="tg tg-soft">{t("MoE · быстрая для размера")}</span>{/if}
        {#if m.tags.includes("english-only")}<span class="tg tg-soft">{t("Только английский")}</span>{/if}
        {#if m.licenseCustom}
          {#if m.licenseUrl && !pick}<a class="tg tg-warn" href={m.licenseUrl} target="_blank" rel="noopener noreferrer" title={t("Условия лицензии — откроются в браузере")}>{t("Особая лицензия")}<Icon name="externalLink" size={10} /></a>
          {:else}<span class="tg tg-warn">{t("Особая лицензия")}</span>{/if}
        {/if}
      </span>
    {/if}
  </span>
  <span class="mg-sr">{t(m.blurb)}</span>
{/snippet}

<!-- Inline confirmation under a row: what is freed, what changes, and that there is no undo. -->
{#snippet removePanel(m: ModelEntry)}
  {@const next = isPrimary(m) ? successor(m) : undefined}
  {@const blocked = lastAsr(m)}
  <div class="confirm yc-in" role="alert">
    {#if hasPartial(m)}
      <p><strong>{t("Отменить загрузку и удалить скачанное?")}</strong> {t("Освободится {size}.", { size: bytes(diskBytes(m)) })}</p>
    {:else if blocked}
      <p><strong>{t("Это единственная модель расшифровки.")}</strong> {t("Сначала скачай другую — без модели запись не расшифруется.")}</p>
    {:else}
      <p>
        <strong>{t("Удалить {name}?", { name: m.name })}</strong> {t("Освободится {size}.", { size: bytes(diskBytes(m)) })}
        {#if next}{t("Это основная модель — основной станет {name}.", { name: next.name })}{:else if isPrimary(m) || m.kind === "voice" || m.kind === "embed"}{consequence(m)}{/if}
        {t("Вернуть можно только скачав заново — отменить удаление нельзя.")}
      </p>
    {/if}
    <span class="acts">
      {#if !blocked}<Button size="sm" variant="danger" disabled={removing} onclick={() => confirmRemove(m)}>{hasPartial(m) ? t("Отменить и удалить") : next ? t("Переключить и удалить") : t("Удалить")}</Button>{/if}
      <Button size="sm" variant="ghost" disabled={removing} onclick={() => (confirmId = null)}>{blocked ? t("Понятно") : t("Отмена")}</Button>
    </span>
  </div>
{/snippet}

{#snippet trash(m: ModelEntry)}
  {#if isRequired(m)}
    <span class="tg tg-req" title={t("Без неё запись не работает")}>{t("нужна для записи")}</span>
  {:else}
    <IconButton variant="flat" aria-label={hasPartial(m) ? t("Отменить и удалить") : t("Удалить модель {name}", { name: m.name })} aria-expanded={confirmId === m.id} onclick={() => (confirmId = confirmId === m.id ? null : m.id)}><Icon name="trash" size={14} /></IconButton>
  {/if}
{/snippet}

{#snippet manageCard(m: ModelEntry)}
  {@const d = m.download}
  {@const ds = d?.state.state}
  {@const vp = verdictPill[m.verdict]}
  {@const canPrimary = m.kind === "asr" || m.kind === "llm"}
  <div class="card" style="border-color: {m.active && m.installed ? 'var(--accent-line)' : ds === 'failed' ? 'var(--warning-line)' : 'var(--border)'}">
    <div class="row">
      {@render identity(m)}

      {#if !m.installed || ds === "failed"}<span class="vp"><StatusPill kind={vp.kind} label={t(vp.label)} sm /></span>{/if}

      {#if m.installed && !m.active && canPrimary}
        <Button size="sm" onclick={() => modelAct(() => api.modelSetActive(m.id))}>{t("Сделать основной")}</Button>
        {@render trash(m)}
      {:else if m.installed && !d}
        {#if m.benchmarkable !== false}<Button size="sm" variant="ghost" disabled={benching === m.id} onclick={() => bench(m)}>{benching === m.id ? t("Меряю…") : t("Измерить")}</Button>{/if}
        {@render trash(m)}
      {:else if !m.installed && !d}
        <Button size="sm" disabled={m.verdict === "wontRun"} onclick={() => modelAct(() => api.modelDownload(m.id))}>
          <svg class="yc-ic" width="12" height="12" viewBox="0 0 24 24"><path d="M12 4v11M7 10l5 5 5-5M5 20h14" /></svg>{t("Скачать")}
        </Button>
      {:else if ds === "running" || ds === "queued"}
        <span class="mg-mono yc-num pct">{pctOf(m)}%</span>
        <IconButton variant="flat" aria-label={t("Пауза")} onclick={() => modelAct(() => api.modelPause(m.id))}><svg class="yc-ic" width="13" height="13" viewBox="0 0 24 24" style="stroke-width: 3"><path d="M8 5v14M16 5v14" /></svg></IconButton>
        <IconButton variant="flat" aria-label={t("Отменить загрузку")} onclick={() => modelAct(() => api.modelCancel(m.id))}><Icon name="x" size={14} /></IconButton>
      {:else if ds === "paused"}
        <Button size="sm" onclick={() => modelAct(() => api.modelDownload(m.id))}>{t("Продолжить")}</Button>
        {@render trash(m)}
      {:else if ds === "verifying"}
        <span class="verifying"><svg class="yc-ic yc-spin" width="13" height="13" viewBox="0 0 24 24" style="color: var(--accent-text); stroke-width: 3"><path d="M12 3a9 9 0 1 0 9 9" /></svg>{t("проверяю")}</span>
      {:else if ds === "failed"}
        <Button size="sm" onclick={() => modelAct(() => api.modelDownload(m.id))}><Icon name="refresh" size={12} />{t("Повторить")}</Button>
        {@render trash(m)}
      {/if}
    </div>

    {#if d && ds !== "done"}
      <div class="yc-in dl">
        <span aria-hidden="true" class="pbar">
          <span style="width: {pctOf(m)}%; background: {ds === 'running' || ds === 'queued' ? 'linear-gradient(90deg, var(--accent), var(--accent-soft), var(--accent))' : ds === 'failed' ? 'var(--warning)' : ds === 'verifying' ? 'var(--accent)' : 'var(--faint)'}; background-size: 200% 100%; animation: {ds === 'running' ? 'ycShimmer 1.6s linear infinite' : 'none'}"></span>
        </span>
        <span class="dlline" style="color: {ds === 'failed' ? 'var(--warning-text)' : 'var(--muted)'}">
          <span class="mg-mono yc-num" style="flex: 1">
            {#if ds === "running" || ds === "queued"}{bytes(d.doneBytes)} {t("из")} {bytes(d.totalBytes)} · {mbps(d.bytesPerSec)}
            {:else if ds === "paused"}{t("На паузе")} · {bytes(d.doneBytes)} {t("из")} {bytes(d.totalBytes)}
            {:else if ds === "failed"}{t("Связь прервалась на {p}% — докачаю с этого места", { p: pctOf(m) })}
            {:else}{t("Скачано · сверяю контрольную сумму")}{/if}
          </span>
          <span class="mg-mono yc-num">{ds === "running" ? etaLabel(d.etaSecs) : ""}</span>
        </span>
      </div>
    {/if}
    {#if confirmId === m.id}{@render removePanel(m)}{/if}
  </div>
{/snippet}

{#snippet pickCard(m: ModelEntry)}
  {@const on = effectiveId === m.id}
  {@const dead = m.verdict === "wontRun"}
  {@const vp = verdictPill[m.verdict]}
  <div class="pickwrap">
    <div class="pickline">
    <button type="button" role="radio" aria-checked={on} aria-disabled={dead} class="card pickrow" class:on class:dead
      onclick={() => { if (!dead) onPick?.(m.id); }}>
      <span class="radio" aria-hidden="true">{#if on}<span></span>{/if}</span>
      {@render identity(m)}
      <span class="pickside">
        <StatusPill kind={vp.kind} label={t(vp.label)} sm />
        <span class="state">{#if m.installed}<Icon name="check" size={11} />{t("установлена")}{:else if m.download}{pctOf(m)}%{/if}</span>
      </span>
      {#if m.installed || hasPartial(m)}<span class="rmgap" aria-hidden="true"></span>{/if}
    </button>
    {#if m.installed || hasPartial(m)}<span class="rmbtn">{@render trash(m)}</span>{/if}
    </div>
    {#if confirmId === m.id}{@render removePanel(m)}{/if}
  </div>
{/snippet}

{#snippet card(m: ModelEntry)}
  {#if pick}{@render pickCard(m)}{:else}{@render manageCard(m)}{/if}
{/snippet}

{#snippet cards(items: ModelEntry[], label: string)}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="list" role={pick ? "radiogroup" : undefined} aria-label={pick ? label : undefined} onkeydown={optionKeys}>
    {#each items as m (m.id)}{@render card(m)}{/each}
  </div>
{/snippet}

{#snippet runtimeRow()}
  {#if !pick && mstore.runtime}
    {@const runtime = mstore.runtime}
    <div class="rt">
      <span class="yc-ellipsis" style="flex: 1; min-width: 0">
        {#if mstore.rtPct !== null}{t("Ставлю рантайм {name} · {size}… {p}%", { name: runtime.name, size: bytes(runtime.sizeBytes), p: mstore.rtPct })}
        {:else if runtime.installed}{t("Рантайм")} {runtime.name} · {bytes(runtime.sizeBytes)} · {t("установлен")}
        {:else}{t("Рантайм")} {runtime.name} · {bytes(runtime.sizeBytes)} · {t("поставится при первом запуске")}{/if}
      </span>
      <button type="button" class="rt-btn" disabled={mstore.rtPct !== null} onclick={() => modelAct(() => api.modelDownload(runtime.id))}>{t("Переустановить")}</button>
    </div>
  {/if}
{/snippet}

<div class="cat" class:stuck>
  <div bind:this={sentinel} aria-hidden="true" class="sentinel"></div>
  <div class="bar">
    <SearchField bind:value={q} bind:el={searchEl} label={t("Поиск по моделям")} placeholder={t("Поиск: название, язык, производитель")} hint="/" />
    {#if tabList.length > 1}<Tabs {tabs} value={tab} onchange={setTab} label={t("Разделы моделей")} idPrefix={uid} />{/if}
  </div>

  <ToggleChips options={chipOptions} selected={filters} ontoggle={toggleFilter} label={t("Фильтры")} />

  <p class="mg-sr" aria-live="polite">{t("Найдено: {n}", { n: inTab.length })}</p>

  <div role={tabList.length > 1 ? "tabpanel" : undefined} id="{uid}-panel" aria-labelledby={tabList.length > 1 ? `${uid}-${tab}` : undefined} class="panel">
    {#if mstore.loaded && inTab.length === 0}
      <div class="empty">
        <Mascot size={56} />
        <strong>{t("Ничего не нашлось")}</strong>
        <span>{t("Попробуй другое слово или сними фильтры.")}</span>
        {#if narrowed}<Button size="sm" onclick={resetAll}>{t("Сбросить фильтры")}</Button>{/if}
      </div>
    {:else if tab === "all"}
      {#each KIND_ORDER.filter((k) => kinds.includes(k)) as k (k)}
        {@const items = kindList(k)}
        {#if items.length}
          {@const capped = !narrowed && !more.has(k) && items.length > FOLD}
          <section class="sect">
            <div class="sh"><SectionLabel as="h2">{t(kindInfo[k].title)}</SectionLabel><span class="gcount mg-mono">{items.length}</span></div>
            {@render cards(capped ? items.slice(0, FOLD) : items, t(kindInfo[k].title))}
            {#if !narrowed && items.length > FOLD}
              <button type="button" class="more yc-press" aria-expanded={!capped} onclick={() => toggleMore(k)}>
                {capped ? t("Показать ещё {n}", { n: items.length - FOLD }) : t("Свернуть")}
                <span class="chev" class:up={!capped}><Icon name="chevronRight" size={12} /></span>
              </button>
            {/if}
            {#if k === "llm"}{@render runtimeRow()}{/if}
          </section>
        {/if}
      {/each}
    {:else}
      <p class="tabsub">{t(kindInfo[tab].sub)}</p>
      {#each tabGroups as g (g.id)}
        {@const open = !g.foldable || wontOpen || ts.length > 0}
        <section class="sect">
          {#if g.title}
            <div class="sh">
              {#if g.foldable && ts.length === 0}
                <button type="button" class="fold yc-press" aria-expanded={open} onclick={() => (wontOpen = !wontOpen)}>
                  <span class="gdot {g.tone}"></span><span class="mg-label">{t(g.title)}</span><span class="gcount mg-mono">{g.items.length}</span>
                  <span class="chev" class:up={open}><Icon name="chevronRight" size={12} /></span>
                </button>
              {:else}
                <span class="gdot {g.tone ?? 'none'}"></span><SectionLabel as="h3">{t(g.title)}</SectionLabel><span class="gcount mg-mono">{g.items.length}</span>
              {/if}
            </div>
          {/if}
          {#if open}{@render cards(g.items, g.title ? t(g.title) : t(kindInfo[tab].title))}{/if}
        </section>
      {/each}
      {#if tab === "llm"}{@render runtimeRow()}{/if}
    {/if}
  </div>
</div>

<style>
  .cat { display: flex; flex-direction: column; gap: 12px; min-width: 0; }
  .sentinel { height: 1px; margin-bottom: -13px; pointer-events: none; }
  .bar { position: sticky; top: 0; z-index: 5; display: flex; flex-direction: column; gap: 8px; margin: 0 calc(-1 * var(--cat-pad, 28px)); padding: 8px var(--cat-pad, 28px) 10px; background: var(--cat-bg, var(--bg)); border-bottom: 1px solid transparent; transition: border-color 0.2s; }
  .stuck .bar { border-bottom-color: var(--border); }
  .panel { display: flex; flex-direction: column; gap: 14px; min-width: 0; }
  .sect { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
  .sh { display: flex; align-items: center; gap: 8px; padding: 6px 4px 0; }
  .gcount { font-size: 11px; font-weight: 700; color: var(--muted); font-variant-numeric: tabular-nums; }
  .gdot { width: 7px; height: 7px; border-radius: 50%; flex: none; background: var(--faint); }
  .gdot.ok { background: var(--ok); } .gdot.warn { background: var(--warning); } .gdot.bad { background: var(--danger); } .gdot.none { display: none; }
  .tabsub { margin: 2px 4px 0; font-size: 12px; color: var(--muted); }
  .fold { display: inline-flex; align-items: center; gap: 8px; padding: 0; border: 0; background: none; color: inherit; cursor: pointer; border-radius: 6px; }
  .fold:focus-visible, .more:focus-visible { outline: 2px solid var(--accent-line); outline-offset: 3px; }
  .chev { display: inline-grid; color: var(--muted); transform: rotate(90deg); transition: transform 0.2s; }
  .chev.up { transform: rotate(-90deg); }
  .more { align-self: flex-start; display: inline-flex; align-items: center; gap: 6px; padding: 5px 8px; margin-left: 2px; border: 0; border-radius: 9px; background: none; color: var(--accent-text); font: inherit; font-size: 12.5px; font-weight: 700; cursor: pointer; }
  .more:hover { background: var(--surface-2); }
  .more .chev { color: inherit; }

  .list { display: flex; flex-direction: column; gap: 6px; }
  .card { position: relative; display: flex; flex-direction: column; gap: 9px; padding: 11px 12px 11px 12px; border: 1px solid var(--border); border-radius: 14px; background: var(--surface); transition: border-color 0.3s; text-align: left; color: inherit; font: inherit; }
  .row { display: flex; align-items: center; gap: 12px; min-width: 0; }
  .body { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .l1 { display: flex; align-items: center; gap: 6px; min-width: 0; flex-wrap: wrap; }
  .nm { font-size: 13px; font-weight: 700; min-width: 0; max-width: 100%; }
  .nm :global(mark) { background: color-mix(in oklch, var(--accent) 28%, transparent); color: inherit; border-radius: 3px; padding: 0 1px; }
  .meta { font-size: 12px; color: var(--muted); }
  .est { display: flex; align-items: center; gap: 7px; font-size: 12px; color: var(--muted); }
  .edot { width: 6px; height: 6px; border-radius: 50%; flex: none; }
  .traits { display: flex; flex-wrap: wrap; gap: 4px; margin-top: 3px; }
  .tg { display: inline-flex; align-items: center; gap: 4px; height: 18px; padding: 0 7px; border-radius: 9px; background: var(--surface-2); color: var(--muted); font-size: 10.5px; font-weight: 700; white-space: nowrap; text-decoration: none; }
  .tg.mg-mono { font-size: 10px; letter-spacing: 0.02em; }
  .tg-rec { background: var(--accent-soft); color: var(--accent-text); border: 1px solid var(--accent-line); }
  .tg-soft { background: transparent; border: 1px solid var(--border); }
  .tg-warn { background: color-mix(in oklch, var(--warning) 14%, var(--surface)); color: var(--warning-text); border: 1px solid var(--warning-line); }
  a.tg-warn:hover { text-decoration: underline; }
  .tag-main { flex: none; height: 18px; padding: 0 8px; border-radius: 9px; background: var(--accent); color: var(--ink-on-accent); font-size: 10.5px; font-weight: 800; display: inline-flex; align-items: center; }
  .tg-req { background: var(--accent-soft); color: var(--accent-text); border: 1px solid var(--accent-line); }
  .confirm { display: flex; flex-direction: column; gap: 9px; padding: 10px 12px; border: 1px solid var(--warning-line); border-radius: 11px; background: color-mix(in oklch, var(--warning) 9%, var(--surface)); font-size: 12.5px; line-height: 1.45; }
  .confirm p { margin: 0; color: var(--text); }
  .confirm .acts { display: flex; gap: 8px; }
  .vp { flex: none; }
  .pct { font-size: 13px; font-weight: 700; color: var(--accent-text); }
  .verifying { display: inline-flex; align-items: center; gap: 7px; font-size: 12px; color: var(--muted); }
  .dl { display: flex; flex-direction: column; gap: 6px; }
  .dlline { display: flex; align-items: center; gap: 8px; font-size: 11px; }
  .pbar { position: relative; display: block; height: 6px; border-radius: 3px; overflow: hidden; background: var(--surface-2); }
  .pbar span { position: absolute; left: 0; top: 0; bottom: 0; border-radius: 3px; transition: width 0.2s; }

  .pickwrap { display: flex; flex-direction: column; gap: 6px; }
  .pickline { position: relative; }
  .rmbtn { position: absolute; right: 10px; top: 50%; transform: translateY(-50%); display: inline-flex; }
  .rmgap { flex: none; width: 30px; }
  .pickrow { flex-direction: row; align-items: center; gap: 12px; width: 100%; box-sizing: border-box; cursor: pointer; }
  .pickrow:hover:not(.dead) { border-color: var(--accent-line); }
  .pickrow.on { border-color: var(--accent-line); background: var(--accent-soft); }
  .pickrow.dead { opacity: 0.55; cursor: not-allowed; }
  .pickrow:focus-visible { outline: 2px solid var(--accent-line); outline-offset: 2px; }
  .radio { flex: none; width: 18px; height: 18px; border-radius: 50%; border: 2px solid var(--faint); display: grid; place-items: center; }
  .pickrow.on .radio { border-color: var(--accent); }
  .radio span { width: 8px; height: 8px; border-radius: 50%; background: var(--accent); }
  .pickside { flex: none; display: flex; flex-direction: column; align-items: flex-end; gap: 5px; }
  .state { display: inline-flex; align-items: center; gap: 4px; font-size: 11px; color: var(--muted); min-height: 14px; }

  .rt { display: flex; align-items: center; gap: 10px; padding: 2px 6px 0; font-size: 11px; color: var(--muted); }
  .rt-btn { flex: none; padding: 2px 6px; border: 0; background: none; font-size: 11px; color: var(--muted); text-decoration: underline; cursor: pointer; }
  .rt-btn:hover:not(:disabled) { color: var(--text); }
  .rt-btn:disabled { opacity: 0.5; }

  .empty { display: flex; flex-direction: column; align-items: center; gap: 6px; padding: 28px 16px 20px; text-align: center; border: 1px dashed var(--border); border-radius: 16px; }
  .empty strong { font-size: 14px; margin-top: 6px; }
  .empty span { font-size: 12.5px; color: var(--muted); margin-bottom: 8px; }
</style>
