<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import StatusDot from "../kit/StatusDot.svelte";
  import FilterChips from "../kit/FilterChips.svelte";
  import { integrations } from "../api";
  import type { Connection } from "../api";
  import { onIntegrationEvent } from "../state.svelte";
  import { t, plural } from "../i18n/index.svelte";
  import { href } from "../router.svelte";
  import { toast } from "../toast.svelte";

  type Filter = "all" | "on" | "soon";
  let items = $state<Connection[]>([]);
  let filter = $state<Filter>("all");

  async function load() { items = await integrations.connections(); }
  onMount(() => { load(); return onIntegrationEvent((e) => { if (e.type === "agendaChanged") load(); }); });

  const options = $derived<{ value: Filter; label: string }[]>([{ value: "all", label: t("Все") }, { value: "on", label: t("Подключено") }, { value: "soon", label: t("Скоро") }]);
  const list = $derived(items.filter((c) => filter === "all" || (filter === "on" ? c.state === "connected" : c.state !== "connected")));
  const wanted = $derived(items.filter((c) => c.want).map((c) => c.name));

  /** "Хочу такое" is a local flag: nothing is sent anywhere. */
  async function toggle(c: Connection) {
    items = items.map((x) => (x.id === c.id ? { ...x, want: !x.want } : x));
    try { await integrations.toggleWant(c.id); } catch (e) { toast.error(e); load(); }
  }
  const accounts = (n = 0) => `${n} ${plural(n, ["аккаунт", "аккаунта", "аккаунтов"], ["account", "accounts"])}`;
</script>

<main>
  <div class="yc-scroll" style="flex: 1; min-height: 0">
    <div class="yc-in col">
      <a class="back" href={href({ name: "settings" })}><Icon name="back" size={14} />{t("Настройки")}</a>

      <header>
        <h1>{t("Подключения")}</h1>
        <span class="yc-pretty lead">{t("Откуда YadaCall берёт расписание звонков и куда умеет заходить сам. Записывать можно любой звонок — и без подключения.")}</span>
      </header>

      <FilterChips label={t("Какие сервисы показать")} value={filter} onchange={(v) => (filter = v)} {options} />

      <div class="grid">
        {#each list as c (c.id)}
          {@const on = c.state === "connected"}
          {@const unconf = c.state === "unconfigured"}
          <div class="yc-in card" style="border-color: {on ? 'var(--accent-line)' : unconf ? 'var(--warning-line)' : 'var(--border)'}; background: {on ? 'var(--surface)' : 'transparent'}">
            <div style="display: flex; align-items: center; gap: 10px">
              <span aria-hidden="true" class="tile" style="background: {on ? 'var(--accent-soft)' : 'var(--surface-2)'}; color: {on ? 'var(--accent-text)' : 'var(--muted)'}">{c.mono}</span>
              <span style="flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 1px">
                <span class="yc-ellipsis name" style="color: {on ? 'var(--text)' : 'var(--muted)'}">{c.name}</span>
                <span class="state" style="color: {on ? 'var(--muted)' : 'var(--faint)'}">
                  {#if on}<StatusDot kind="ok" />{/if}
                  {on ? `${t("Подключено")} · ${accounts(c.accounts)}` : unconf ? t("не настроено") : t("Скоро")}
                </span>
              </span>
            </div>
            <span class="yc-pretty desc">{t(c.description)}</span>
            {#if on}
              <a href={href({ name: "google" })} class="yc-press act primary">{t("Аккаунты")}<Icon name="chevronRight" size={12} /></a>
            {:else if unconf}
              <a href={c.docsUrl} target="_blank" rel="noopener noreferrer" class="yc-press act">{t("Как настроить")}<Icon name="chevronRight" size={12} /></a>
            {:else}
              <button type="button" class="yc-press act" class:want={c.want} aria-pressed={c.want} onclick={() => toggle(c)}>
                <Icon name={c.want ? "check" : "bell"} size={12} />{c.want ? t("Жду") : t("Хочу такое")}
              </button>
            {/if}
          </div>
        {/each}
      </div>

      {#if wanted.length}
        <div class="yc-in summary">
          <span style="display: inline-flex; color: var(--accent-text)"><Icon name="bell" size={14} /></span>
          <span class="yc-pretty" style="flex: 1">{t("Отмечено {n} {w}: {names}. Как только появится — включу и напишу в уведомлении.", { n: wanted.length, w: plural(wanted.length, ["сервис", "сервиса", "сервисов"], ["service", "services"]), names: wanted.join(", ") })}</span>
        </div>
      {/if}

      <span class="note">
        <span style="display: inline-flex; padding-top: 2px"><Icon name="info" size={12} /></span>
        <span class="yc-pretty">{t("В первой версии подключается только Google Meet. Нет нужного сервиса —")} <a href="https://github.com/limeflash/yadacall/issues" target="_blank" rel="noopener noreferrer" style="color: var(--accent-text)">{t("предложи на GitHub")}</a>.</span>
      </span>
    </div>
  </div>
</main>

<style>
  main { position: relative; flex: 1 1 520px; min-width: 0; display: flex; flex-direction: column; min-height: 0; }
  .col { max-width: 640px; box-sizing: border-box; margin: 0 auto; padding: 22px 28px 44px; display: flex; flex-direction: column; gap: 14px; }
  .back { align-self: flex-start; display: inline-flex; align-items: center; gap: 6px; font-size: 13px; color: var(--muted); text-decoration: none; }
  header { display: flex; flex-direction: column; gap: 4px; }
  h1 { margin: 0; font-size: 28px; line-height: 1.15; font-weight: 800; letter-spacing: -0.04em; }
  .lead { font-size: 13px; color: var(--muted); }
  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(250px, 1fr)); gap: 10px; }
  .card { display: flex; flex-direction: column; gap: 10px; min-height: 132px; box-sizing: border-box; padding: 14px; border: 1px solid var(--border); border-radius: 16px; }
  .tile { width: 36px; height: 36px; flex: none; border-radius: 11px; font-size: 15px; font-weight: 800; display: grid; place-items: center; }
  .name { font-size: 14px; font-weight: 700; }
  .state { display: inline-flex; align-items: center; gap: 6px; font-size: 12px; }
  .desc { flex: 1; font-size: 12.5px; line-height: 1.45; color: var(--muted); }
  .act { align-self: flex-start; display: inline-flex; align-items: center; gap: 6px; height: 30px; padding: 0 12px; border: 1px solid var(--border); border-radius: 10px; background: transparent; color: var(--text); font: inherit; font-size: 12.5px; font-weight: 700; text-decoration: none; }
  .act.want { border-color: var(--accent-line); background: var(--accent-soft); color: var(--accent-text); }
  .act.primary { border: 0; background: var(--accent); color: var(--ink-on-accent); font-weight: 800; }
  .summary { display: flex; align-items: center; gap: 10px; padding: 11px 14px; border: 1px solid var(--border); border-radius: 14px; background: var(--surface); font-size: 12.5px; color: var(--muted); }
  .note { display: flex; align-items: flex-start; gap: 7px; padding: 2px 2px 0; font-size: 12px; color: var(--muted); }
</style>
