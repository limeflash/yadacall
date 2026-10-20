<script lang="ts">
  /**
   * First-run wizard: hello → call language → speech model → summaries → permissions → calendar → done.
   * Nothing is downloaded until the last step's «Начать»; the progress (step and choices) survives closing the window.
   */
  import { onMount } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import Button from "../kit/Button.svelte";
  import Segmented from "../kit/Segmented.svelte";
  import ThemeToggle from "../kit/ThemeToggle.svelte";
  import StatusPill from "../kit/StatusPill.svelte";
  import Mascot from "../kit/Mascot.svelte";
  import Sheet from "../kit/Sheet.svelte";
  import VendorMark from "../kit/VendorMark.svelte";
  import ModelCatalog from "../components/ModelCatalog.svelte";
  import { api, integrations } from "../api";
  import type { CalendarAccount, ModelEntry, ModelKind, OllamaStatus, Permissions, SummaryMode, Verdict } from "../api";
  import { saveSettings, theme } from "../state.svelte";
  import { t, i18n } from "../i18n/index.svelte";
  import { go } from "../router.svelte";
  import { isMac, overlayTitleBar } from "../platform";
  import { bytes } from "../format";
  import { toast } from "../toast.svelte";
  import { activateModel, attachModels, modelAct, modelById, mstore, setPending } from "../modelsStore.svelte";
  import { displayName, langText, recommendFor, type SpokenLang } from "../models";

  type Mode = "local" | "ollama" | "api" | "off";
  const STEPS = [
    { id: "hello", title: "Привет" },
    { id: "lang", title: "Язык созвонов" },
    { id: "asr", title: "Распознавание речи" },
    { id: "llm", title: "Итоги" },
    { id: "perm", title: "Разрешения" },
    { id: "cal", title: "Календарь" },
    { id: "done", title: "Готово" },
  ] as const;
  const LAST = STEPS.length - 1;

  // ---- progress, remembered between launches -----------------------------------------------------------------
  const KEY = "yc.onboarding";
  function restore(): { step: number; lang: SpokenLang; asrId: string | null; mode: Mode; llmId: string | null } {
    const d = { step: 0, lang: "ru" as SpokenLang, asrId: null, mode: "local" as Mode, llmId: null };
    try {
      const v = JSON.parse(localStorage.getItem(KEY) ?? "{}");
      return {
        step: Number.isInteger(v.step) && v.step >= 0 && v.step <= LAST ? v.step : d.step,
        lang: ["ru", "multi", "en"].includes(v.lang) ? v.lang : d.lang,
        asrId: typeof v.asrId === "string" ? v.asrId : null,
        mode: ["local", "ollama", "api", "off"].includes(v.mode) ? v.mode : d.mode,
        llmId: typeof v.llmId === "string" ? v.llmId : null,
      };
    } catch { return d; }
  }
  const saved = restore();
  let step = $state(saved.step);
  let back = $state(false);
  let lang = $state<SpokenLang>(saved.lang);
  let asrId = $state<string | null>(saved.asrId);
  let mode = $state<Mode>(saved.mode);
  let llmId = $state<string | null>(saved.llmId);
  function persist() { try { localStorage.setItem(KEY, JSON.stringify({ step, lang, asrId, mode, llmId })); } catch { /* storage may be blocked */ } }

  let perms = $state<Permissions>({ microphone: "notDetermined", systemAudio: "notDetermined", screenRecording: "notDetermined" });
  let ollama = $state<OllamaStatus | null>(null);
  let accounts = $state<CalendarAccount[]>([]);
  let connecting = $state(false);
  let sheet = $state<null | ModelKind>(null);
  let finishing = $state(false);

  onMount(() => {
    const detach = attachModels();
    api.permissions().then((p) => (perms = p));
    api.ollamaStatus("local").then((st) => (ollama = st)).catch(() => (ollama = null));
    integrations.accounts().then((a) => (accounts = a)).catch(() => {});
    return detach;
  });

  // ---- choices ---------------------------------------------------------------------------------------------------
  const recAsr = $derived(recommendFor(mstore.models, "asr", lang));
  const recLlm = $derived(recommendFor(mstore.models, "llm", lang));
  const asr = $derived(modelById(asrId) ?? recAsr);
  const llm = $derived(modelById(llmId) ?? recLlm);
  const found = $derived.by(() => {
    if (!ollama?.running) return null;
    const chat = ollama.models.filter((m) => !/embed/i.test(m.name));
    return (ollama.signedIn ? chat.find((m) => m.cloud) : undefined) ?? chat.find((m) => !m.cloud) ?? null;
  });
  $effect(() => { if (mode === "ollama" && !found && ollama !== null) mode = "local"; });
  const langName = $derived(lang === "ru" ? t("Русский") : lang === "en" ? t("Английский") : t("Разные языки"));
  const toDownload = $derived([asr, mode === "local" ? llm : undefined].filter((m): m is ModelEntry => !!m && !m.installed));
  const total = $derived(toDownload.reduce((s, m) => s + m.sizeBytes, 0));

  const pill: Record<Verdict, { kind: "ok" | "blip" | "warn" | "bad"; label: string }> = {
    great: { kind: "ok", label: "Отлично" }, ok: { kind: "blip", label: "Нормально" }, slow: { kind: "warn", label: "Медленно" }, wontRun: { kind: "bad", label: "Не потянет" },
  };

  // ---- navigation ------------------------------------------------------------------------------------------------
  const canNext = $derived(step === 2 ? !!asr : step === 3 ? mode !== "local" || !!llm : true);
  function goTo(n: number) { if (n < 0 || n > LAST) return; back = n < step; step = n; persist(); }
  const next = () => { if (canNext && !finishing) goTo(step + 1); };
  const prev = () => goTo(step - 1);
  function setLang(v: SpokenLang) { if (v !== lang) { lang = v; asrId = null; llmId = null; } persist(); }
  function onKey(e: KeyboardEvent) {
    if (e.defaultPrevented) return;
    if (e.key === "Escape" && step > 0 && !sheet) { e.preventDefault(); prev(); return; }
    const el = e.target as HTMLElement | null;
    const interactive = !!el && (/^(BUTTON|A|INPUT|TEXTAREA|SELECT)$/.test(el.tagName) || !!el.closest("[role=dialog]"));
    if (e.key === "Enter" && !interactive && !e.metaKey && !e.ctrlKey) { e.preventDefault(); if (step === LAST) void finish(); else next(); }
  }

  async function grant(kind: "microphone" | "systemAudio") { perms = await api.requestPermission(kind); }
  async function connectGoogle() {
    connecting = true;
    try { const a = await integrations.addGoogleAccount(); accounts = [...accounts.filter((x) => x.id !== a.id), a]; }
    catch (e) { if (!/cancelled/i.test(String((e as Error)?.message ?? e))) toast.error(e); }
    connecting = false;
  }
  function cancelConnect() { void integrations.cancelAddAccount(); }

  function choose(kind: ModelKind, id: string) {
    if (kind === "asr") asrId = id; else llmId = id;
    sheet = null; persist();
  }

  /** The last step: save the choices, then (and only then) start the downloads; the finished ones become primary. */
  async function finish() {
    if (finishing) return;
    finishing = true;
    let sm: SummaryMode = { mode: "off" };
    if (mode === "local") sm = { mode: "local", model: llm?.id ?? "" };
    else if (mode === "ollama" && found) sm = { mode: "ollama", base: "local", model: found.name };
    else if (mode === "api") sm = { mode: "api", provider: "anthropic", model: "claude-sonnet-5-5" };
    await saveSettings({ summaries: { mode: sm }, meetingLanguage: lang === "multi" ? "auto" : lang, onboarded: true });
    const picks: ModelEntry[] = [asr, mode === "local" ? llm : undefined].filter((m): m is ModelEntry => !!m);
    for (const m of picks) {
      if (m.installed) { try { await activateModel(m.id); } catch (e) { toast.error(e); } }
      else { setPending(m.kind, m.id); await modelAct(() => api.modelDownload(m.id)); }
    }
    try { localStorage.removeItem(KEY); } catch { /* ignore */ }
    go({ name: "meetings" });
  }

  const modeOptions = $derived<{ id: Mode; title: string; sub: string; off?: boolean }[]>([
    { id: "local", title: t("На компьютере"), sub: t("Приватно и без интернета") },
    { id: "ollama", title: t("Через Ollama"), sub: found ? t("найдено: {name}", { name: found.name }) : t("Ollama не найдена на этом компьютере"), off: !found },
    { id: "api", title: t("Через API"), sub: t("Claude и другие — ключ добавишь позже в настройках") },
    { id: "off", title: t("Без итогов"), sub: t("Только расшифровка и поиск") },
  ]);
  const langOptions = $derived<{ id: SpokenLang; title: string; sub: string }[]>([
    { id: "ru", title: t("Русский"), sub: t("Почти все звонки на русском") },
    { id: "multi", title: t("Разные языки"), sub: t("Русский, английский и другие вперемешку") },
    { id: "en", title: "English", sub: t("Почти все звонки на английском") },
  ]);
</script>

<svelte:window onkeydown={onKey} />

{#snippet modelCard(m: ModelEntry, rec: ModelEntry | undefined)}
  <div class="mcard">
    <VendorMark vendor={m.vendor} size={40} />
    <span class="two">
      <span class="mn"><span class="nm">{displayName(m)}</span>{#if rec && m.id === rec.id}<span class="tg">{t("Рекомендуем")}</span>{/if}</span>
      <span class="s">{[m.vendor, bytes(m.sizeBytes), langText(m), m.installed ? t("уже установлена") : ""].filter(Boolean).join(" · ")}</span>
      <span class="s forecast">{t(m.estimate)}</span>
    </span>
    <StatusPill kind={pill[m.verdict].kind} label={t(pill[m.verdict].label)} sm />
  </div>
{/snippet}

<div class="wrap" class:mac={overlayTitleBar}>
  <header class="head">
    <nav aria-label={t("Шаги")}>
      <ol class="dots">
        {#each STEPS as st, i (st.id)}
          <li class="dot" class:done={i < step} class:cur={i === step} aria-current={i === step ? "step" : undefined}>
            <button type="button" class="seg" disabled={i > step} aria-label={t("Шаг {n} из {m}: {title}", { n: i + 1, m: STEPS.length, title: t(st.title) })} onclick={() => goTo(i)}></button>
          </li>
        {/each}
      </ol>
      <p class="count mg-mono">{step + 1} / {STEPS.length}{step > 0 ? ` · ${t(STEPS[step].title)}` : ""}</p>
    </nav>
    <div class="top">
      <Segmented variant="lang" label={t("Язык")} value={i18n.lang} onchange={(v) => saveSettings({ uiLanguage: v })} options={[{ value: "ru", label: "RU" }, { value: "en", label: "EN" }]} />
      <ThemeToggle dark={theme.resolved === "dark"} onchange={(v) => saveSettings({ theme: v })} />
    </div>
  </header>

  <main class="stage">
    {#key step}
      <section class="step" class:back aria-labelledby="ob-h">
        {#if step === 0}
          <div class="hero">
            <div class="yc-float mascot"><Mascot size="100%" accent={theme.accent} /></div>
            <h1 id="ob-h" class="yc-balance">{t("Привет! Я YadaCall")}</h1>
            <p class="lead yc-pretty">{t("Записываю звонки, расшифровываю их и собираю итоги. Настроим всё за пару минут.")}</p>
          </div>
          <ul class="facts">
            <li><span class="ic"><Icon name="mic" size={15} /></span><span>{t("Пишу две дорожки: твой микрофон и звук собеседников")}</span></li>
            <li><span class="ic"><Icon name="cpu" size={15} /></span><span>{t("Расшифровка и итоги считаются прямо на этом компьютере")}</span></li>
            <li><span class="ic"><Icon name="lock" size={15} /></span><span>{t("Записи, тексты и голоса остаются у тебя — в сеть уходит только то, что ты включишь сам")}</span></li>
          </ul>
        {:else if step === 1}
          <h1 id="ob-h">{t("На каком языке твои созвоны?")}</h1>
          <p class="lead">{t("От этого зависит, какую модель распознавания мы предложим. Поменять можно в любой момент.")}</p>
          <div role="radiogroup" aria-label={t("Язык созвонов")} class="opts">
            {#each langOptions as o (o.id)}
              <button type="button" role="radio" aria-checked={lang === o.id} class="opt yc-press" class:on={lang === o.id} onclick={() => setLang(o.id)}>
                <span aria-hidden="true" class="radio"></span>
                <span class="two"><span class="ot">{o.title}</span><span class="s">{o.sub}</span></span>
              </button>
            {/each}
          </div>
        {:else if step === 2}
          <h1 id="ob-h">{t("Распознавание речи")}</h1>
          <p class="lead">{t("Модель превращает речь в текст. Она работает на этом компьютере и скачивается один раз.")}</p>
          {#if asr}
            {@render modelCard(asr, recAsr)}
            <p class="note"><Icon name="arrowDown" size={13} />{asr.installed ? t("Уже установлена — скачивать ничего не нужно.") : t("Скачается {size} — но только в конце настройки, после твоего «Начать».", { size: bytes(asr.sizeBytes) })}</p>
          {:else}
            <p class="note warn">{t("Для этого компьютера подходящей модели не нашлось — выбери из каталога.")}</p>
          {/if}
          <button type="button" class="other yc-press" onclick={() => (sheet = "asr")}>{t("Выбрать другую")}<Icon name="chevronRight" size={13} /></button>
        {:else if step === 3}
          <h1 id="ob-h">{t("Итоги встреч")}</h1>
          <p class="lead">{t("После встречи — короткое резюме, решения и задачи. Можно не собирать: останется расшифровка и поиск.")}</p>
          <div role="radiogroup" aria-label={t("Чем собирать итоги")} class="opts grid">
            {#each modeOptions as o (o.id)}
              <button type="button" role="radio" aria-checked={mode === o.id} aria-disabled={o.off} class="opt yc-press" class:on={mode === o.id} class:off={o.off} onclick={() => { if (!o.off) { mode = o.id; persist(); } }}>
                <span aria-hidden="true" class="radio"></span>
                <span class="two"><span class="ot">{o.title}</span><span class="s">{o.sub}</span></span>
              </button>
            {/each}
          </div>
          {#if mode === "local"}
            <div class="sub yc-in">
              {#if llm}
                {@render modelCard(llm, recLlm)}
                <p class="note"><Icon name="arrowDown" size={13} />{llm.installed ? t("Уже установлена.") : t("Скачается {size} — после «Начать». Запись не ждёт загрузку.", { size: bytes(llm.sizeBytes) })}</p>
              {:else}
                <p class="note warn">{t("Для этого компьютера подходящей модели не нашлось — выбери из каталога.")}</p>
              {/if}
              <button type="button" class="other yc-press" onclick={() => (sheet = "llm")}>{t("Выбрать другую")}<Icon name="chevronRight" size={13} /></button>
            </div>
          {:else if mode === "ollama" && found}
            <p class="note yc-in"><Icon name="info" size={13} />{found.cloud ? t("Текст расшифровки уходит в Ollama Cloud, звук — никогда") : t("Всё считается на этом компьютере, ничего не уходит в сеть")}</p>
          {:else if mode === "api"}
            <p class="note yc-in"><Icon name="info" size={13} />{t("Ключ добавишь в Настройках → Итоги. Текст расшифровки уходит в выбранный сервис, звук — никогда.")}</p>
          {:else if mode === "off"}
            <p class="note yc-in"><Icon name="info" size={13} />{t("Позже можно включить в настройках — в том числе Claude или Ollama Cloud по ключу")}</p>
          {/if}
        {:else if step === 4}
          <h1 id="ob-h">{t("Разрешения")}</h1>
          <p class="lead">{t("Чтобы записывать звонок, нужны микрофон и звук собеседников. Звук не покидает компьютер.")}</p>
          <div class="card">
            <div class="row">
              <span aria-hidden="true" class="ic"><Icon name="mic" size={15} /></span>
              <span class="two"><span class="ot">{t("Микрофон")}</span><span class="s">{t("Твой голос")}</span></span>
              {#if perms.microphone === "granted"}<StatusPill kind="ok" sm label={t("Разрешено")} />{:else}<Button size="sm" onclick={() => grant("microphone")}>{t("Разрешить")}</Button>{/if}
            </div>
            <div class="row bt">
              <span aria-hidden="true" class="ic"><Icon name="monitor" size={15} /></span>
              <span class="two"><span class="ot">{t("Звук собеседников")}</span><span class="s">{isMac ? t("macOS отдаёт его через разрешение «Запись экрана»") : t("Системный звук записывается без дополнительных разрешений")}</span></span>
              {#if perms.systemAudio === "granted"}<StatusPill kind="ok" sm label={t("Разрешено")} />{:else}<Button size="sm" onclick={() => grant("systemAudio")}>{t("Разрешить")}</Button>{/if}
            </div>
          </div>
          <p class="note"><Icon name="info" size={13} />{t("Можно пропустить и разрешить позже — приложение подскажет, когда понадобится.")}</p>
        {:else if step === 5}
          <h1 id="ob-h">{t("Календарь")}</h1>
          <p class="lead">{t("Необязательно. Подключи Google Календарь — YadaCall напомнит о встрече и сам зайдёт в звонок. Расписание не покидает компьютер.")}</p>
          <div class="card">
            {#if accounts.length}
              {#each accounts as a, i (a.id)}
                <div class="row" class:bt={i > 0}>
                  <span aria-hidden="true" class="ic" style="color: {a.color}"><Icon name="calendar" size={15} /></span>
                  <span class="two"><span class="ot">{a.label}</span><span class="s">{a.email}</span></span>
                  <StatusPill kind="ok" sm label={t("Подключён")} />
                </div>
              {/each}
            {:else}
              <div class="row">
                <span aria-hidden="true" class="ic"><Icon name="calendar" size={15} /></span>
                <span class="two"><span class="ot">Google Calendar</span><span class="s">{connecting ? t("Жду подтверждения в браузере…") : t("Откроется браузер для входа")}</span></span>
                {#if connecting}<Button size="sm" variant="ghost" onclick={cancelConnect}>{t("Отмена")}</Button>{:else}<Button size="sm" onclick={connectGoogle}>{t("Подключить")}</Button>{/if}
              </div>
            {/if}
          </div>
          <p class="note"><Icon name="info" size={13} />{t("Можно пропустить: потом это в Настройки → Подключения.")}</p>
        {:else}
          <div class="hero tight">
            <div class="yc-float mascot sm"><Mascot size="100%" accent={theme.accent} /></div>
            <h1 id="ob-h" class="yc-balance">{t("Всё готово")}</h1>
          </div>
          <div class="card">
            <div class="row"><span class="k">{t("Язык созвонов")}</span><span class="v">{langName}</span></div>
            <div class="row bt"><span class="k">{t("Распознавание")}</span><span class="v mg-mono">{asr ? displayName(asr) : "—"}{asr ? ` · ${bytes(asr.sizeBytes)}` : ""}</span></div>
            <div class="row bt"><span class="k">{t("Итоги")}</span><span class="v mg-mono">{mode === "local" ? `${llm ? displayName(llm) : "—"}${llm ? ` · ${bytes(llm.sizeBytes)}` : ""}` : mode === "ollama" ? `Ollama · ${found?.name ?? ""}` : mode === "api" ? "API" : t("Без итогов")}</span></div>
            <div class="row bt"><span class="k">{t("Разрешения")}</span><span class="v">{perms.microphone === "granted" && perms.systemAudio === "granted" ? t("выданы") : t("выдашь позже")}</span></div>
            <div class="row bt"><span class="k">{t("Календарь")}</span><span class="v">{accounts.length ? accounts.map((a) => a.email).join(", ") : t("пропущен")}</span></div>
          </div>
          <p class="note big"><Icon name="arrowDown" size={14} />{total > 0 ? t("Сейчас начнётся загрузка: {size}. Идёт в фоне — записывать можно сразу.", { size: bytes(total) }) : t("Скачивать ничего не нужно.")}</p>
        {/if}
      </section>
    {/key}
  </main>

  <footer class="foot">
      <Button variant="ghost" onclick={prev} disabled={step === 0 || finishing} style={step === 0 ? "visibility: hidden" : ""}>{t("Назад")}</Button>
      <span class="hintk"><kbd class="mg-kbd">Enter</kbd> {t("далее")} · <kbd class="mg-kbd">Esc</kbd> {t("назад")}</span>
      {#if step === LAST}
        <Button variant="primary" onclick={finish} disabled={finishing}>{finishing ? t("Запускаю…") : t("Начать")}</Button>
      {:else}
        <Button variant="primary" onclick={next} disabled={!canNext}>{step === 0 ? t("Поехали") : step === 5 && !accounts.length ? t("Пропустить") : t("Далее")}</Button>
      {/if}
  </footer>
</div>

<Sheet width={640} open={sheet !== null} title={sheet === "llm" ? t("Модель для итогов") : t("Модель расшифровки")} onclose={() => (sheet = null)}>
  {#if sheet}
    <ModelCatalog kinds={[sheet]} mode="pick" selectedId={sheet === "asr" ? asr?.id ?? null : llm?.id ?? null} preselectId={(sheet === "asr" ? recAsr : recLlm)?.id ?? null} onPick={(id) => choose(sheet!, id)} autofocusSearch />
  {/if}
  {#snippet footer()}<span class="hintf">{t("Ничего не скачивается, пока ты не нажмёшь «Начать» в конце.")}</span>{/snippet}
</Sheet>

<style>
  /* Full-height page that never scrolls: step bar on top, content centred in the remaining height, footer pinned.
     Vertical rhythm (--g) and the mascot scale with the viewport height. */
  .wrap {
    --g: clamp(8px, 1.9vh, 16px);
    --inset-top: 20px;
    height: 100%; box-sizing: border-box; display: grid; grid-template-rows: auto minmax(0, 1fr) auto; justify-items: center;
    padding: var(--inset-top) 20px clamp(14px, 3vh, 28px); background: var(--bg); overflow: hidden;
  }
  /* macOS overlay title bar: keep clear of the traffic lights (y 20–32) and the 28px drag strip. */
  .wrap.mac { --inset-top: 38px; }
  .head { width: 100%; display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 480px) minmax(0, 1fr); align-items: center; column-gap: 16px; }
  .head nav { grid-column: 2; }
  .top { grid-column: 3; justify-self: end; display: flex; align-items: center; gap: 10px; margin-right: -2px; }
  .stage { width: 100%; max-width: 480px; min-height: 0; display: flex; align-items: center; justify-content: center; padding: var(--g) 0; }
  nav { display: flex; flex-direction: column; gap: 6px; }
  .dots { list-style: none; margin: 0; padding: 0; display: flex; gap: 6px; }
  .dot { flex: 1; }
  .seg { display: block; width: 100%; height: 16px; padding: 0; border: 0; background: none; cursor: pointer; position: relative; }
  .seg::after { content: ""; position: absolute; left: 0; right: 0; top: 6px; height: 4px; border-radius: 2px; background: var(--surface-2); transition: background-color 0.3s; }
  .dot.done .seg::after { background: var(--accent-line); }
  .dot.cur .seg::after { background: var(--accent); }
  .seg:disabled { cursor: default; }
  .seg:focus-visible { outline: 2px solid var(--accent-line); outline-offset: 1px; border-radius: 4px; }
  .count { margin: 0; font-size: 11.5px; color: var(--muted); }

  .step { width: 100%; display: flex; flex-direction: column; gap: var(--g); animation: stepIn 0.4s var(--ease-out-soft, ease-out) both; }
  .step.back { animation-name: stepBack; }
  @keyframes stepIn { from { opacity: 0; transform: translateX(22px); } to { opacity: 1; transform: none; } }
  @keyframes stepBack { from { opacity: 0; transform: translateX(-22px); } to { opacity: 1; transform: none; } }
  @media (prefers-reduced-motion: reduce) { .step { animation: none; } }

  h1 { margin: 0; font-size: clamp(21px, 3.5vh, 24px); line-height: 1.2; font-weight: 800; letter-spacing: -0.035em; }
  .lead { margin: 0; font-size: 14px; line-height: 1.45; color: var(--muted); }
  .hero { display: flex; flex-direction: column; align-items: center; gap: calc(var(--g) * 0.8); text-align: center; }
  .mascot { width: clamp(72px, 17vh, 150px); height: clamp(72px, 17vh, 150px); }
  .mascot.sm { width: clamp(64px, 12vh, 96px); height: clamp(64px, 12vh, 96px); }
  .facts { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: calc(var(--g) * 0.6); }
  .facts li { display: flex; align-items: center; gap: 12px; padding: clamp(7px, 1.4vh, 11px) 14px; border: 1px solid var(--border); border-radius: 14px; background: var(--surface); font-size: 13.5px; line-height: 1.4; }
  .ic { width: 30px; height: 30px; flex: none; border-radius: 9px; background: var(--surface-2); color: var(--muted); display: grid; place-items: center; }
  .facts .ic { background: var(--accent-soft); color: var(--accent-text); }

  .opts { display: flex; flex-direction: column; gap: calc(var(--g) * 0.5); }
  .opts.grid { display: grid; grid-template-columns: 1fr 1fr; gap: calc(var(--g) * 0.6); }
  .opts.grid .opt { align-items: flex-start; }
  .opts.grid .radio { margin-top: 3px; }
  .opt { display: flex; align-items: center; gap: 12px; width: 100%; box-sizing: border-box; padding: clamp(7px, 1.3vh, 11px) 14px; border: 1px solid var(--border); border-radius: 14px; background: var(--surface); color: var(--text); text-align: left; font: inherit; cursor: pointer; transition: border-color 0.2s, background-color 0.2s; }
  .opt:hover:not(.off) { border-color: var(--accent-line); }
  .opt.on { border-color: var(--accent); background: var(--accent-soft); }
  .opt.off { opacity: 0.5; cursor: not-allowed; }
  .opt:focus-visible, .other:focus-visible { outline: 2px solid var(--accent-line); outline-offset: 2px; }
  .radio { flex: none; width: 16px; height: 16px; box-sizing: border-box; border: 1.5px solid var(--faint); border-radius: 50%; }
  .opt.on .radio { border: 5px solid var(--accent); }
  .two { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .ot { font-size: 14px; font-weight: 700; }
  .s { font-size: 12px; color: var(--muted); }
  .sub { display: flex; flex-direction: column; gap: calc(var(--g) * 0.6); }

  .mcard { display: flex; align-items: center; gap: 12px; padding: clamp(8px, 1.5vh, 12px) 14px; border: 1px solid var(--accent-line); border-radius: 14px; background: var(--surface); }
  .mn { display: flex; align-items: center; gap: 8px; min-width: 0; flex-wrap: wrap; }
  .nm { font-size: 13.5px; font-weight: 700; }
  .tg { display: inline-flex; align-items: center; height: 18px; padding: 0 7px; border-radius: 9px; background: var(--accent-soft); color: var(--accent-text); border: 1px solid var(--accent-line); font-size: 10.5px; font-weight: 700; }
  .forecast { color: var(--text); opacity: 0.8; }
  .other { align-self: flex-start; display: inline-flex; align-items: center; gap: 4px; padding: 5px 4px; border: 0; background: none; color: var(--accent-text); font: inherit; font-size: 13px; font-weight: 700; cursor: pointer; border-radius: 8px; }
  .note { display: flex; align-items: flex-start; gap: 8px; margin: 0; font-size: 12.5px; line-height: 1.45; color: var(--muted); }
  .note :global(svg) { flex: none; margin-top: 2px; }
  .note.warn { color: var(--warning-text); }
  .note.big { font-size: 13px; color: var(--text); }

  .card { border: 1px solid var(--border); border-radius: 16px; background: var(--surface); }
  .row { display: flex; align-items: center; gap: 12px; padding: clamp(8px, 1.6vh, 13px) 16px; }
  .bt { border-top: 1px solid var(--border); }
  .k { flex: none; width: 120px; font-size: 12.5px; color: var(--muted); }
  .v { flex: 1; min-width: 0; font-size: 13px; font-weight: 600; text-align: right; overflow-wrap: anywhere; }

  .foot { width: 100%; max-width: 480px; display: flex; align-items: center; justify-content: space-between; gap: 12px; }
  .hintk { flex: 1; text-align: center; font-size: 11px; color: var(--muted); }
  .hintk :global(.mg-kbd) { display: inline-flex; height: 18px; min-width: 18px; font-size: 10px; vertical-align: middle; }
  .hintf { font-size: 12px; color: var(--muted); }
  @media (max-width: 480px) { .hintk { display: none; } }
</style>
