<script lang="ts">
  import { onMount } from "svelte";
  import { app, initApp, theme } from "./lib/state.svelte";
  import { nav, replace } from "./lib/router.svelte";
  import { i18n } from "./lib/i18n/index.svelte";
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
          {#if route.name === "meetings"}<Meetings />
          {:else if route.name === "meeting"}<MeetingDetail id={route.id} tab={route.tab} at={route.at} />
          {:else if route.name === "record"}<Record />
          {:else if route.name === "ask"}<Ask q={route.q} />
          {:else if route.name === "tasks"}<Tasks />
          {:else if route.name === "upload"}<Upload />
          {:else if route.name === "settings"}<Settings />
          {:else if route.name === "models"}<Models />
          {/if}
        {/key}
      </div>
    {/if}
  {/if}
  <Toasts />
{/if}
