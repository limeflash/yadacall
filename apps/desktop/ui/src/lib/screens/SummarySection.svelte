<script lang="ts">
  import Button from "../kit/Button.svelte";
  import Segmented from "../kit/Segmented.svelte";
  import Select from "../kit/Select.svelte";
  import SectionLabel from "../kit/SectionLabel.svelte";
  import StatusDot from "../kit/StatusDot.svelte";
  import ModelPicker from "../components/ModelPicker.svelte";
  import { api } from "../api";
  import type { ApiProvider, KeyAccount, OllamaBase, OllamaStatus, ProviderTest, SummaryMode } from "../api";
  import { app, saveSettings } from "../state.svelte";
  import { t, i18n } from "../i18n/index.svelte";
  import { bytes } from "../format";
  import { toast } from "../toast.svelte";
  import { isCloudName } from "../api/summaries";

  const s = $derived(app.settings!.summaries);
  const keys = $derived(s.keysSet);
  type Pick = "off" | "local" | "ollama" | "api";
  const pick = $derived<Pick>(s.mode);

  // ---- state ---------------------------------------------------------------------------------------
  let status = $state<OllamaStatus | null>(null);
  let probing = $state(false);
  let test = $state<{ busy: boolean; result?: ProviderTest }>({ busy: false });
  let draft = $state<Record<string, string>>({});

  const save = (mode: SummaryMode) => { test = { busy: false }; return saveSettings({ summaries: { mode } }); };
  const saveMode = (patch: Partial<SummaryMode>) => save({ ...s, ...patch } as SummaryMode);

  // ---- Ollama detection ------------------------------------------------------------------------------
  const ollamaBase = $derived<OllamaBase>(s.mode === "ollama" ? s.base : "local");
  const ollamaOn = $derived(s.mode === "ollama");
  let seq = 0;
  async function refresh(base: OllamaBase) {
    const mine = ++seq;
    probing = true;
    try { const st = await api.ollamaStatus(base); if (mine === seq) status = st; }
    catch { if (mine === seq) status = { running: false, signedIn: false, models: [], error: "" }; }
    if (mine === seq) probing = false;
  }
  $effect(() => {
    if (!ollamaOn) return;
    void keys.ollamaCloud;
    refresh(ollamaBase);
  });
  /** The first usable model: the account's cloud models first, then anything installed (never embedders). */
  function firstModel(st: OllamaStatus | null): string {
    const chat = (st?.models ?? []).filter((m) => !/embed/i.test(m.name));
    return (chat.find((m) => m.cloud) ?? chat[0])?.name ?? "";
  }
  $effect(() => { if (s.mode === "ollama" && !s.model && status?.models.length) saveMode({ model: firstModel(status) } as Partial<SummaryMode>); });

  const ollamaOptions = $derived.by(() => {
    const chat = (status?.models ?? []).filter((m) => !/embed/i.test(m.name));
    const cloudGroup = ollamaBase === "cloud" ? t("Ollama Cloud") : t("Облако · по подписке Ollama");
    const opts: { value: string; label: string; hint?: string; group?: string }[] = [
      ...chat.filter((m) => m.cloud).map((m) => ({ value: m.name, label: m.name, group: cloudGroup })),
      ...chat.filter((m) => !m.cloud).map((m) => ({ value: m.name, label: m.name, hint: m.sizeBytes ? bytes(m.sizeBytes) : undefined, group: t("На компьютере") })),
    ];
    const cur = s.mode === "ollama" ? s.model : "";
    if (cur && !opts.some((o) => o.value === cur)) opts.unshift({ value: cur, label: cur });
    return opts;
  });
  const ollamaModelIsCloud = $derived(s.mode === "ollama" && (s.base === "cloud" || isCloudName(s.model) || !!status?.models.find((m) => m.name === s.model)?.cloud));

  // ---- local --------------------------------------------------------------------------------------------

  // ---- API ----------------------------------------------------------------------------------------------
  const providerOptions = $derived<{ value: ApiProvider; label: string; hint?: string }[]>([
    { value: "anthropic", label: "Claude", hint: t("облако, только ключ API") },
    { value: "openAiCompatible", label: t("OpenAI-совместимый"), hint: t("OpenAI, свой сервер") },
    { value: "openRouter", label: "OpenRouter", hint: t("облако, только текст") },
    { value: "lmStudio", label: "LM Studio", hint: t("на этом компьютере") },
  ]);
  const claudeModels = [
    { value: "claude-sonnet-5-5", label: "Claude Sonnet 5.5", hint: t("по умолчанию") },
    { value: "claude-haiku-5-5", label: "Claude Haiku 5.5", hint: t("быстрее и дешевле") },
    { value: "claude-opus-5-5", label: "Claude Opus 5.5", hint: t("для длинных встреч") },
  ];
  const apiDefaults: Record<ApiProvider, { model: string; baseUrl?: string }> = {
    anthropic: { model: "claude-sonnet-5-5" },
    openAiCompatible: { model: "", baseUrl: "https://api.openai.com/v1" },
    openRouter: { model: "" },
    lmStudio: { model: "", baseUrl: "http://127.0.0.1:1234/v1" },
  };
  const keyOf = (p: ApiProvider): KeyAccount | null => (p === "anthropic" ? "anthropic" : p === "openRouter" ? "openRouter" : p === "openAiCompatible" ? "openAi" : null);
  const loopback = (u?: string) => !u || /^https?:\/\/(localhost|127\.0\.0\.1|\[::1\])([:/]|$)/i.test(u);

  // ---- actions ---------------------------------------------------------------------------------------------
  function choose(v: Pick) {
    if (v === "off") save({ mode: "off" });
    else if (v === "local") save({ mode: "local", model: "" });
    else if (v === "ollama") save({ mode: "ollama", base: "local", model: firstModel(status) });
    else save({ mode: "api", provider: "anthropic", model: apiDefaults.anthropic.model });
  }
  async function saveKey(account: KeyAccount) {
    const v = (draft[account] ?? "").trim();
    if (!v) return;
    draft[account] = "";
    await saveSettings({ summaries: { key: { account, value: v } } });
    toast(t("Ключ сохранён в связке ключей системы"));
  }
  async function dropKey(account: KeyAccount) {
    await saveSettings({ summaries: { key: { account, value: "" } } });
    toast(t("Ключ удалён"));
  }
  async function runTest() {
    test = { busy: true };
    try { test = { busy: false, result: await api.testSummaryProvider() }; }
    catch (e) { test = { busy: false, result: { ok: false, latencyMs: 0, error: e instanceof Error ? e.message : String((e as { message?: string })?.message ?? e) } }; }
  }
  const secs = (ms: number) => (ms / 1000).toFixed(1).replace(".", i18n.lang === "ru" ? "," : ".");

  // ---- text ------------------------------------------------------------------------------------------------
  const statusLine = $derived.by((): { dot: "ok" | "warn" | "bad" | "off" | "busy"; text: string; help: boolean } => {
    if (probing && !status) return { dot: "busy", text: t("Ищу Ollama…"), help: false };
    if (!status) return { dot: "off", text: "", help: false };
    if (ollamaBase === "cloud") {
      if (status.running) return { dot: "ok", text: t("Ключ принят · моделей: {n}", { n: status.models.length }), help: false };
      if (status.error === "no key") return { dot: "off", text: t("Вставь ключ Ollama Cloud"), help: false };
      return { dot: "bad", text: t("Ollama Cloud не отвечает или ключ не подошёл"), help: false };
    }
    if (!status.running) return { dot: "off", text: t("Ollama не найдена на этом компьютере"), help: true };
    if (!status.signedIn) return { dot: "warn", text: t("Ollama запущена, но вход в аккаунт не выполнен — облачные модели недоступны"), help: true };
    return { dot: "ok", text: `${t("Ollama запущена")}${status.version ? ` · v${status.version}` : ""} · ${t("вход выполнен")}`, help: false };
  });
  const egress = $derived.by(() => {
    switch (s.mode) {
      case "off": return t("Итоги не собираются. В «Спросить» будут только найденные фрагменты, без сгенерированного ответа");
      case "local": return t("Всё считается на этом компьютере, ничего не уходит в сеть");
      case "ollama": return ollamaModelIsCloud ? t("Текст расшифровки уходит в Ollama Cloud, звук — никогда") : t("Всё считается на этом компьютере, ничего не уходит в сеть");
      case "api": {
        if (s.provider === "lmStudio" && loopback(s.baseUrl)) return t("Всё считается на этом компьютере, ничего не уходит в сеть");
        const name = s.provider === "anthropic" ? "Claude" : s.provider === "openRouter" ? "OpenRouter" : (s.baseUrl && !loopback(s.baseUrl) ? new URL(s.baseUrl).host : "API");
        return t("Текст расшифровки уходит в {name}, звук — никогда", { name });
      }
    }
  });
  const canTest = $derived(s.mode !== "off");
</script>

{#snippet keyRow(account: KeyAccount, subtitle: string, ph: string)}
  <div class="grow bt">
    <span class="two"><span style="font-size: 14px">{t("Ключ API")}</span><span class="sub">{keys[account] ? t("Сохранён в связке ключей системы") : subtitle}</span></span>
    <input class="tf mg-mono" style="width: 150px" type="password" bind:value={draft[account]} placeholder={keys[account] ? "••••••••" : ph} autocomplete="off" aria-label={t("Ключ API")}
      onkeydown={(e) => { if (e.key === "Enter") saveKey(account); }} />
    <Button size="sm" onclick={() => saveKey(account)} disabled={!(draft[account] ?? "").trim()}>{t("Сохранить")}</Button>
    {#if keys[account]}<Button size="sm" variant="ghost" onclick={() => dropKey(account)}>{t("Удалить")}</Button>{/if}
  </div>
{/snippet}

{#snippet testRow()}
  <div class="grow bt">
    <span class="two">
      <span style="font-size: 14px">{t("Проверить")}</span>
      {#if test.busy}<span class="sub" role="status">{t("Отправляю короткий запрос…")}</span>
      {:else if test.result?.ok}<span class="sub ok" role="status">{t("Ответила за {s} с", { s: secs(test.result.latencyMs) })}{test.result.reply ? ` · «${test.result.reply}»` : ""}</span>
      {:else if test.result}<span class="sub bad" role="alert">{test.result.error}</span>
      {:else}<span class="sub">{t("Один короткий запрос — покажет, отвечает ли модель и как быстро")}</span>{/if}
    </span>
    <Button size="sm" onclick={runTest} disabled={test.busy || !canTest}>{test.busy ? t("Проверяю…") : t("Проверить")}</Button>
  </div>
{/snippet}

<div style="padding: 14px 4px 0"><SectionLabel as="h2">{t("Итоги")}</SectionLabel></div>
<Segmented variant="thumb" label={t("Чем собирать итоги")} value={pick} onchange={choose}
  options={[{ value: "off", label: t("Выкл") }, { value: "local", label: t("На компьютере") }, { value: "ollama", label: "Ollama" }, { value: "api", label: "API" }]} />

{#if s.mode !== "off"}
  <div class="group">
    {#if s.mode === "local"}
      <div class="grow pick"><div style="flex: 1; min-width: 0"><ModelPicker kind="llm" title={t("Модель итогов")} currentId={s.model} sheetTitle={t("Модель для итогов")} /></div></div>
      {@render testRow()}
    {:else if s.mode === "ollama"}
      <div class="grow">
        <span style="flex: 1; min-width: 0; font-size: 14px">{t("Откуда")}</span>
        <Segmented variant="inset" label={t("Откуда брать модели Ollama")} value={s.base} onchange={(v) => save({ mode: "ollama", base: v, model: "" })}
          options={[{ value: "local", label: t("Приложение Ollama") }, { value: "cloud", label: "Ollama Cloud" }]} />
      </div>
      <div class="grow bt">
        <StatusDot kind={statusLine.dot} />
        <span class="two"><span style="font-size: 13px" role="status">{statusLine.text}</span>
          {#if statusLine.help}<span class="sub"><a href="https://docs.ollama.com/cloud" target="_blank" rel="noopener noreferrer" class="lnk">{t("Как войти в Ollama")}</a></span>{/if}
        </span>
        <Button size="sm" variant="ghost" onclick={() => refresh(ollamaBase)} disabled={probing}>{t("Найти снова")}</Button>
      </div>
      {#if s.base === "cloud"}
        {@render keyRow("ollamaCloud", t("Ключи — на ollama.com/settings/keys"), "…")}
      {/if}
      {#if ollamaOptions.length}
        <div class="grow bt">
          <span style="flex: 1; min-width: 0; font-size: 14px">{t("Модель")}</span>
          <div style="width: 240px; flex: none"><Select compact label={t("Модель итогов")} value={s.model} onchange={(v) => saveMode({ model: v } as Partial<SummaryMode>)} options={ollamaOptions} /></div>
        </div>
      {:else if status && (status.running || s.base === "local")}
        <div class="grow bt"><span class="sub">{status.running ? t("В Ollama нет моделей для текста. Облачные модели появляются после входа в аккаунт.") : t("Моделей пока не видно")}</span></div>
      {/if}
      {@render testRow()}
    {:else if s.mode === "api"}
      <div class="grow">
        <span style="flex: 1; min-width: 0; font-size: 14px">{t("Сервис")}</span>
        <div style="width: 240px; flex: none"><Select compact label={t("Сервис")} value={s.provider} onchange={(v) => save({ mode: "api", provider: v, ...apiDefaults[v] })} options={providerOptions} /></div>
      </div>
      {#if s.provider === "anthropic"}
        <div class="grow bt">
          <span style="flex: 1; min-width: 0; font-size: 14px">{t("Модель")}</span>
          <div style="width: 240px; flex: none"><Select compact label={t("Модель итогов")} value={s.model || "claude-sonnet-5-5"} onchange={(v) => saveMode({ model: v } as Partial<SummaryMode>)} options={claudeModels} /></div>
        </div>
      {:else}
        <label class="grow bt"><span style="flex: 1; min-width: 0; font-size: 14px">{t("Модель (имя)")}</span>
          <input class="tf mg-mono" style="width: 240px" value={s.model} placeholder={s.provider === "openRouter" ? "anthropic/claude-sonnet-5-5" : ""} onchange={(e) => saveMode({ model: e.currentTarget.value } as Partial<SummaryMode>)} /></label>
      {/if}
      {#if s.provider === "openAiCompatible" || s.provider === "lmStudio"}
        <label class="grow bt"><span style="flex: 1; min-width: 0; font-size: 14px">{t("Адрес API")}</span>
          <input class="tf mg-mono" style="width: 240px" value={s.baseUrl ?? ""} placeholder="https://…/v1" onchange={(e) => saveMode({ baseUrl: e.currentTarget.value } as Partial<SummaryMode>)} /></label>
      {/if}
      {#if keyOf(s.provider)}
        {@render keyRow(keyOf(s.provider)!, t("Хранится в связке ключей системы, не в файлах"), "sk-…")}
      {/if}
      {@render testRow()}
    {/if}
  </div>
{/if}
<div class="hint">{egress}</div>
{#if s.mode === "api" && s.provider === "anthropic"}
  <div class="hint">{t("Подписка Claude для сторонних приложений недоступна — нужен ключ API с console.anthropic.com")}</div>
{/if}

<style>
  .group { border: 1px solid var(--border); border-radius: 16px; background: var(--surface); }
  .grow { display: flex; align-items: center; gap: 16px; padding: 12px 16px; }
  .pick { padding: 4px 12px 12px; }
  .pick :global(.head) { padding-top: 6px; }
  .bt { border-top: 1px solid var(--border); }
  .two { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .sub { font-size: 12px; color: var(--muted); }
  .sub.ok { color: var(--accent-text); }
  .sub.bad { color: var(--danger-text); }
  .lnk { color: var(--accent-text); }
  .hint { padding: 2px 4px 0; font-size: 12px; color: var(--muted); }
  .tf { height: 32px; box-sizing: border-box; padding: 0 10px; border: 1px solid var(--border); border-radius: 10px; background: var(--bg); color: var(--text); font-size: 12px; }
</style>
