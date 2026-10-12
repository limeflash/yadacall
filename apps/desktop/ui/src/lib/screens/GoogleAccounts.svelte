<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import Button from "../kit/Button.svelte";
  import IconButton from "../kit/IconButton.svelte";
  import Chip from "../kit/Chip.svelte";
  import Segmented from "../kit/Segmented.svelte";
  import Switch from "../kit/Switch.svelte";
  import StatusDot from "../kit/StatusDot.svelte";
  import SectionLabel from "../kit/SectionLabel.svelte";
  import { integrations } from "../api";
  import type { CalendarAccount, Connection, RecordPolicy, ReminderSettings } from "../api";
  import { onIntegrationEvent } from "../state.svelte";
  import { t, plural } from "../i18n/index.svelte";
  import { href } from "../router.svelte";
  import { isMac, isWindows } from "../platform";
  import { toast } from "../toast.svelte";

  let accounts = $state<CalendarAccount[]>([]);
  let settings = $state<ReminderSettings>({ remindMinutes: 2, openInAccount: true, detectUnscheduled: true, browserAccess: false });
  /** The "Google Meet" card of the catalog: tells whether this build can sign in at all. */
  let meet = $state<Connection | null>(null);
  let confirm = $state<string | null>(null);
  /** 0 = idle, 1..3 = steps of "Добавить аккаунт Google". */
  let adding = $state(0);
  let addRun = 0;

  async function load() { accounts = await integrations.accounts(); }
  onMount(() => {
    load();
    integrations.reminderSettings().then((s) => (settings = s));
    integrations.connections().then((all) => (meet = all.find((c) => c.id === "meet") ?? null)).catch(() => {});
    return onIntegrationEvent((e) => {
      if (e.type === "agendaChanged" || e.type === "authExpired") load();
      else if (e.type === "addProgress" && adding > 0) adding = e.step;
    });
  });

  const policyOptions = $derived<{ value: RecordPolicy; label: string }[]>([{ value: "ask", label: t("Спрашивать") }, { value: "always", label: t("Всегда") }, { value: "never", label: t("Никогда") }]);
  const remindOptions = $derived([{ value: "1", label: t("{n} мин", { n: 1 }) }, { value: "2", label: t("{n} мин", { n: 2 }) }, { value: "5", label: t("{n} мин", { n: 5 }) }]);
  const steps = $derived(["Выбери аккаунт в браузере", "Разреши читать календарь", "Забираю встречи на неделю"].map((label, i) => ({ label: t(label), done: adding > i + 1, active: adding === i + 1 })));
  const week = (n: number) => `${n} ${plural(n, ["встреча", "встречи", "встреч"], ["meeting", "meetings"])} ${t("на неделе")}`;
  const keychain = $derived(isMac ? t("Связке ключей macOS") : isWindows ? t("Диспетчере учётных данных Windows") : t("хранилище паролей системы"));

  function patch(id: string, p: Partial<CalendarAccount>) { accounts = accounts.map((a) => (a.id === id ? { ...a, ...p } : a)); }
  async function safely(job: Promise<unknown>) { try { await job; } catch (e) { toast.error(String(e)); load(); } }
  const setPolicy = (a: CalendarAccount, v: RecordPolicy) => { patch(a.id, { recordPolicy: v }); safely(integrations.setAccountPolicy(a.id, v)); };
  const setShow = (a: CalendarAccount, v: boolean) => { patch(a.id, { visible: v }); safely(integrations.setAccountVisible(a.id, v)); };
  async function remove(a: CalendarAccount) {
    confirm = null;
    accounts = accounts.filter((x) => x.id !== a.id);
    await safely(integrations.removeAccount(a.id));
    toast(t("{email} отключён", { email: a.email }));
  }
  async function reconnect(a: CalendarAccount) {
    patch(a.id, { status: "connecting" });
    try {
      await integrations.reauthAccount(a.id);
      toast(t("Снова вижу календарь {email}", { email: a.email }));
    } catch (e) { toast.error(String(e)); }
    load();
  }
  async function startAdd() {
    adding = 1;
    const run = ++addRun;
    try {
      const a = await integrations.addGoogleAccount();
      if (run !== addRun) return;
      adding = 0;
      await load();
      toast(`${t("{email} подключён", { email: a.email })} · ${week(a.meetingsThisWeek)}`);
    } catch (e) {
      if (run !== addRun || adding === 0) return; // cancelled by the user
      adding = 0;
      toast.error(String(e));
    }
  }
  function cancelAdd() { addRun++; adding = 0; integrations.cancelAddAccount(); }
  function saveReminders(p: Partial<ReminderSettings>) {
    settings = { ...settings, ...p };
    safely(integrations.setReminderSettings(settings));
  }
  const seen = [
    { label: "Время, название и ссылка на звонок", ok: true },
    { label: "Участники — чтобы подписать голоса в расшифровке", ok: true },
    { label: "Почта, файлы и контакты — нет", ok: false },
  ];
</script>

<main>
  <div class="yc-scroll" style="flex: 1; min-height: 0">
    <div class="yc-in col">
      <a class="back" href={href({ name: "connections" })}><Icon name="back" size={14} />{t("Подключения")}</a>

      <header>
        <span aria-hidden="true" class="gmark">G</span>
        <span style="flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px">
          <h1>Google Meet</h1>
          <span class="yc-pretty lead">{t("Встречи из всех аккаунтов — в одном списке. Личный и рабочий живут рядом и не путаются.")}</span>
        </span>
      </header>

      <div style="display: flex; flex-direction: column; gap: 8px">
        {#each accounts as a (a.id)}
          <div class="yc-in card" style="border-color: {a.status === 'expired' ? 'var(--warning-line)' : 'var(--border)'}">
            <div style="display: flex; align-items: center; gap: 12px">
              <span aria-hidden="true" class="av" style="background: {a.color}">{Array.from(a.label)[0]}</span>
              <span style="flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 1px">
                <span style="display: flex; align-items: center; gap: 8px; min-width: 0">
                  <span class="yc-ellipsis" style="font-size: 14px; font-weight: 700">{a.label}</span>
                  {#if a.workspace}<Chip>Workspace</Chip>{/if}
                </span>
                <span class="mg-mono yc-ellipsis" style="font-size: 12px; color: var(--muted)">{a.email}</span>
              </span>
              {#if a.status === "ok"}
                <span class="st"><StatusDot kind="ok" />{week(a.meetingsThisWeek)}</span>
              {:else if a.status === "connecting"}
                <span class="st"><svg class="yc-ic yc-spin" width="12" height="12" viewBox="0 0 24 24" style="color: var(--busy); stroke-width: 3.2"><path d="M12 3a9 9 0 1 0 9 9" /></svg>{t("жду браузер")}</span>
              {:else}
                <span class="st" style="color: var(--warning-text)"><Icon name="warn" size={12} />{t("вход истёк")}</span>
              {/if}
              <IconButton variant="flat" aria-label={t("Отключить аккаунт")} onclick={() => (confirm = confirm === a.id ? null : a.id)}><Icon name="x" size={14} /></IconButton>
            </div>

            {#if a.status === "expired"}
              <div class="sub">
                <span class="yc-pretty" style="flex: 1 1 240px; font-size: 12.5px; color: var(--muted)">{t("Google попросил войти снова. Пока не войдёшь, встречи из этого календаря не видны.")}</span>
                <Button size="sm" variant="primary" onclick={() => reconnect(a)}>{t("Войти снова")}</Button>
              </div>
            {/if}

            {#if a.status === "ok"}
              <div class="sub">
                <span style="font-size: 12.5px; color: var(--muted)">{t("Запись")}</span>
                <Segmented variant="inset" label={t("Записывать встречи аккаунта {email}", { email: a.email })} value={a.recordPolicy} onchange={(v) => setPolicy(a, v)} options={policyOptions} />
                <span style="flex: 1"></span>
                <label class="shw">{t("В календаре")}<Switch checked={a.visible} onchange={(v) => setShow(a, v)} /></label>
              </div>
            {/if}

            {#if confirm === a.id}
              <div class="yc-in confirm">
                <span class="yc-pretty" style="flex: 1 1 220px; font-size: 12.5px">{t("Отключить {email}? Записи и итоги останутся.", { email: a.email })}</span>
                <Button size="sm" variant="ghost" onclick={() => (confirm = null)}>{t("Отмена")}</Button>
                <Button size="sm" variant="danger" onclick={() => remove(a)}>{t("Отключить")}</Button>
              </div>
            {/if}
          </div>
        {/each}

        {#if meet?.state === "unconfigured"}
          <div class="yc-in unconf">
            <span style="display: inline-flex; padding-top: 2px; color: var(--warning-text)"><Icon name="warn" size={14} /></span>
            <span style="flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px">
              <span style="font-size: 14px; font-weight: 700">{t("Вход через Google не настроен")}</span>
              <span class="yc-pretty" style="font-size: 12.5px; color: var(--muted)">{t("В эту сборку не вшит ключ Google OAuth, поэтому календарь подключить нельзя. Записывать звонки можно и без него.")}</span>
              <a href={meet.docsUrl} target="_blank" rel="noopener noreferrer" style="align-self: flex-start; font-size: 12.5px; font-weight: 700; color: var(--accent-text)">{t("Как настроить (docs/GOOGLE_OAUTH.md)")}</a>
            </span>
          </div>
        {:else if adding === 0}
          <button type="button" class="yc-press addbtn" onclick={startAdd}><Icon name="plus" size={14} />{t("Добавить аккаунт Google")}</button>
        {:else}
          <div class="yc-in adding">
            <div style="display: flex; align-items: flex-start; gap: 12px">
              <span style="flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px">
                <span style="font-size: 14px; font-weight: 700">{t("Вход через Google")}</span>
                <span class="yc-pretty" style="font-size: 12.5px; color: var(--muted)">{t("Открыл браузер. Выбери аккаунт и разреши читать календарь — сюда вернусь сам.")}</span>
              </span>
              <Button size="sm" variant="ghost" onclick={cancelAdd}>{t("Отмена")}</Button>
            </div>
            <div style="display: flex; flex-direction: column; gap: 8px">
              {#each steps as st (st.label)}
                <div class="step" style="color: {st.done || st.active ? 'var(--text)' : 'var(--faint)'}">
                  <span aria-hidden="true" class="sdot" style="background: {st.done ? 'var(--accent)' : 'transparent'}; box-shadow: inset 0 0 0 1.5px {st.done ? 'var(--accent)' : st.active ? 'var(--accent-line)' : 'var(--border)'}">
                    {#if st.done}<Icon name="check" size={12} />{/if}
                    {#if st.active}<svg class="yc-ic yc-spin" width="12" height="12" viewBox="0 0 24 24" style="color: var(--accent-text); stroke-width: 3.2"><path d="M12 3a9 9 0 1 0 9 9" /></svg>{/if}
                  </span>
                  {st.label}
                </div>
              {/each}
            </div>
          </div>
        {/if}
      </div>

      <div style="padding: 14px 4px 0"><SectionLabel as="h2">{t("Перед звонком")}</SectionLabel></div>
      <div class="group">
        <div class="grow" style="flex-wrap: wrap; gap: 10px 16px">
          <span style="flex: 1 1 180px; min-width: 0; font-size: 14px">{t("Напомнить до начала")}</span>
          <Segmented variant="inset" label={t("За сколько напомнить")} value={String(settings.remindMinutes)} onchange={(v) => saveReminders({ remindMinutes: Number(v) })} options={remindOptions} />
        </div>
        <label class="grow bt">
          <span class="two"><span style="font-size: 14px">{t("Открывать Meet в нужном аккаунте")}</span><span class="subtext">{t("Рабочая встреча — под рабочим, личная — под личным, без «Сменить аккаунт»")}</span></span>
          <Switch checked={settings.openInAccount} onchange={(v) => saveReminders({ openInAccount: v })} />
        </label>
        <label class="grow bt">
          <span class="two"><span style="font-size: 14px">{t("Замечать звонки не из календаря")}</span><span class="subtext">{t("Зашёл в Meet по ссылке из чата — предложу записать")}</span></span>
          <Switch checked={settings.detectUnscheduled} onchange={(v) => saveReminders({ detectUnscheduled: v })} />
        </label>
        {#if isMac && settings.detectUnscheduled}
          <div class="grow bt" style="flex-wrap: wrap; gap: 10px 16px">
            <span class="two" style="flex: 1 1 220px"><span style="font-size: 14px">{t("Видеть вкладки браузера")}</span><span class="subtext">{settings.browserAccess ? t("Комнату Meet узнаю по адресу вкладки. Доступ к браузеру можно отозвать в Системных настройках → Конфиденциальность → Автоматизация.") : t("Без этого звонок замечу по микрофону и заголовку окна. С доступом — точнее: узнаю комнату Meet по адресу вкладки. macOS спросит разрешение только после нажатия.")}</span></span>
            {#if settings.browserAccess}
              <span class="st"><StatusDot kind="ok" />{t("Разрешено")}</span>
              <Button size="sm" variant="ghost" onclick={() => saveReminders({ browserAccess: false })}>{t("Выключить")}</Button>
            {:else}
              <Button size="sm" variant="secondary" onclick={() => saveReminders({ browserAccess: true })}>{t("Разрешить")}</Button>
            {/if}
          </div>
        {/if}
      </div>

      <div style="padding: 14px 4px 0"><SectionLabel as="h2">{t("Что видит YadaCall")}</SectionLabel></div>
      <div class="seen">
        {#each seen as r (r.label)}
          <div style="display: flex; align-items: center; gap: 10px; font-size: 13px; color: {r.ok ? 'var(--text)' : 'var(--muted)'}">
            <span aria-hidden="true" class="sico" style="background: {r.ok ? 'var(--accent-soft)' : 'var(--surface-2)'}; color: {r.ok ? 'var(--accent-text)' : 'var(--muted)'}"><Icon name={r.ok ? "check" : "x"} size={12} /></span>
            {t(r.label)}
          </div>
        {/each}
        <span class="lock">
          <span style="display: inline-flex; padding-top: 6px"><Icon name="lock" size={12} /></span>
          <span class="yc-pretty" style="padding-top: 4px">{t("Только чтение календаря. Ключи входа лежат в {store} и никуда не уходят.", { store: keychain })}</span>
        </span>
      </div>
    </div>
  </div>
</main>

<style>
  main { position: relative; flex: 1 1 520px; min-width: 0; display: flex; flex-direction: column; min-height: 0; }
  .col { max-width: 620px; box-sizing: border-box; margin: 0 auto; padding: 22px 28px 48px; display: flex; flex-direction: column; gap: 10px; }
  .back { align-self: flex-start; display: inline-flex; align-items: center; gap: 6px; font-size: 13px; color: var(--muted); text-decoration: none; }
  header { display: flex; align-items: center; gap: 14px; padding-bottom: 6px; }
  .gmark { width: 44px; height: 44px; flex: none; border-radius: 13px; background: var(--accent-soft); color: var(--accent-text); font-size: 19px; font-weight: 800; display: grid; place-items: center; }
  h1 { margin: 0; font-size: 28px; line-height: 1.15; font-weight: 800; letter-spacing: -0.04em; }
  .lead { font-size: 13px; color: var(--muted); }
  .card { display: flex; flex-direction: column; gap: 12px; padding: 14px; border: 1px solid var(--border); border-radius: 16px; background: var(--surface); transition: border-color 0.3s; }
  .av { width: 36px; height: 36px; flex: none; border-radius: 50%; color: #0c0c0e; font-size: 15px; font-weight: 800; display: grid; place-items: center; }
  .st { flex: none; display: inline-flex; align-items: center; gap: 6px; font-size: 12px; color: var(--muted); }
  .sub { display: flex; flex-wrap: wrap; align-items: center; gap: 10px 14px; padding-left: 48px; }
  .shw { display: inline-flex; align-items: center; gap: 8px; font-size: 12.5px; color: var(--muted); }
  .confirm { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 10px; margin-left: 48px; padding: 10px 12px; border-radius: 12px; background: var(--surface-2); }
  .addbtn { display: flex; align-items: center; justify-content: center; gap: 8px; height: 48px; border: 1px dashed var(--faint); border-radius: 16px; background: transparent; color: var(--text); font: inherit; font-size: 13.5px; font-weight: 700; }
  .unconf { display: flex; align-items: flex-start; gap: 12px; padding: 14px 16px; border: 1px solid var(--warning-line); border-radius: 16px; background: var(--surface); }
  .adding { display: flex; flex-direction: column; gap: 12px; padding: 14px 16px; border: 1px solid var(--accent-line); border-radius: 16px; background: var(--surface); }
  .step { display: flex; align-items: center; gap: 10px; font-size: 13px; }
  .sdot { width: 18px; height: 18px; flex: none; border-radius: 50%; display: grid; place-items: center; color: var(--ink-on-accent); }
  .group { border: 1px solid var(--border); border-radius: 16px; background: var(--surface); }
  .grow { display: flex; align-items: center; gap: 16px; padding: 12px 16px; }
  .bt { border-top: 1px solid var(--border); }
  .two { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .subtext { font-size: 12px; color: var(--muted); }
  .seen { display: flex; flex-direction: column; gap: 10px; padding: 14px 16px; border: 1px solid var(--border); border-radius: 16px; background: var(--surface); }
  .sico { width: 20px; height: 20px; flex: none; border-radius: 50%; display: grid; place-items: center; }
  .lock { display: flex; align-items: flex-start; gap: 8px; padding-top: 4px; border-top: 1px solid var(--border); font-size: 12px; color: var(--muted); }
</style>
