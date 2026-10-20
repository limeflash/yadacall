<script lang="ts">
  import { onMount } from "svelte";
  import { app, initApp, theme } from "./lib/state.svelte";
  import { nav, replace } from "./lib/router.svelte";
  import { i18n, t } from "./lib/i18n/index.svelte";
  import Button from "./lib/kit/Button.svelte";
  import Sidebar from "./lib/Sidebar.svelte";
  import Toasts from "./lib/kit/Toasts.svelte";
  import Meetings from "./lib/screens/Meetings.svelte";
  import MeetingDetail from "./lib/screens/MeetingDetail.svelte";
  import Record from "./lib/screens/Record.svelte";
  import Ask from "./lib/screens/Ask.svelte";
  import Tasks from "./lib/screens/Tasks.svelte";
  import Upload from "./lib/screens/Upload.svelte";
  import Settings from "./lib/screens/Settings.svelte";
  import Models from "./lib/screens/Models.svelte";
  import Calendar from "./lib/screens/Calendar.svelte";
  import Connections from "./lib/screens/Connections.svelte";
  import GoogleAccounts from "./lib/screens/GoogleAccounts.svelte";
  import Onboarding from "./lib/screens/Onboarding.svelte";
  import Tray from "./lib/screens/Tray.svelte";
  import Mini from "./lib/screens/Mini.svelte";
  import { overlayTitleBar } from "./lib/platform";

  const win = new URLSearchParams(location.search).get("w") ?? "main";

  // Theme + accent live on <html>, exactly like the design's `data-theme` / `--accent`.
  $effect(() => {
    const el = document.documentElement;
    el.dataset.theme = theme.resolved;
    el.style.setProperty("--accent", theme.accent);
    el.lang = i18n.lang;
  });

  onMount(() => { initApp(); });

  // First run: send the user to onboarding once settings are loaded.
  $effect(() => {
    if (win === "main" && app.ready && app.settings && !app.settings.onboarded && nav.route.name !== "onboarding") replace({ name: "onboarding" });
  });

  const route = $derived(nav.route);
</script>

{#if win === "tray"}
  <Tray />
{:else if win === "mini"}
  <Mini />
{:else}
  {#if overlayTitleBar}<div class="drag-strip" data-tauri-drag-region></div>{/if}
  {#if app.ready}
    {#if route.name === "onboarding"}
      <Onboarding />
    {:else}
      <div class="yc-app">
        <Sidebar />
        {#key route.name === "meeting" ? "meeting" : route.name}
          <svelte:boundary onerror={(e) => console.error("screen failed", e)}>
          {#if route.name === "meetings"}<Meetings />
          {:else if route.name === "meeting"}<MeetingDetail id={route.id} tab={route.tab} at={route.at} />
          {:else if route.name === "record"}<Record />
          {:else if route.name === "ask"}<Ask q={route.q} folder={route.folder} />
          {:else if route.name === "tasks"}<Tasks />
          {:else if route.name === "upload"}<Upload />
          {:else if route.name === "settings"}<Settings />
          {:else if route.name === "models"}<Models />
          {:else if route.name === "calendar"}<Calendar />
          {:else if route.name === "connections"}<Connections />
          {:else if route.name === "google"}<GoogleAccounts />
          {/if}
          {#snippet failed(error, reset)}
            <main class="crash" role="alert">
              <h1>{t("Не получилось показать этот экран")}</h1>
              <p>{t("Данные в порядке — сбой только в отображении. Попробуй открыть экран ещё раз.")}</p>
              <div class="crash-btns">
                <Button size="md" variant="primary" onclick={reset}>{t("Повторить")}</Button>
                <Button size="md" variant="ghost" onclick={() => replace({ name: "meetings" })}>{t("К встречам")}</Button>
              </div>
              <details>
                <summary>{t("Подробнее")}</summary>
                <pre>{String((error as Error)?.stack ?? error)}</pre>
              </details>
            </main>
          {/snippet}
          </svelte:boundary>
        {/key}
      </div>
    {/if}
  {/if}
  <Toasts />
{/if}

<style>
  .crash { flex: 1 1 520px; min-width: 0; box-sizing: border-box; padding: 56px 40px; display: flex; flex-direction: column; align-items: flex-start; gap: 12px; overflow: auto; }
  .crash h1 { margin: 0; font-size: 24px; font-weight: 800; letter-spacing: -0.03em; }
  .crash p { margin: 0; max-width: 420px; font-size: 14px; color: var(--muted); }
  .crash-btns { display: flex; gap: 8px; margin-top: 4px; }
  .crash details { max-width: 100%; margin-top: 8px; font-size: 12px; color: var(--muted); }
  .crash summary { cursor: pointer; }
  .crash pre { margin: 8px 0 0; padding: 10px 12px; max-width: 100%; overflow: auto; border-radius: 12px; background: var(--surface); font-family: var(--font-mono); font-size: 11px; white-space: pre-wrap; overflow-wrap: anywhere; user-select: text; }
</style>
