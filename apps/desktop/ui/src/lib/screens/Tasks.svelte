<script lang="ts">
  import { onMount } from "svelte";
  import SectionLabel from "../kit/SectionLabel.svelte";
  import { api } from "../api";
  import type { TaskItem } from "../api";
  import { app, refreshTasks } from "../state.svelte";
  import { t, plural } from "../i18n/index.svelte";
  import { clock, dueLabel, shortDate } from "../format";
  import { go } from "../router.svelte";
  import Switch from "../kit/Switch.svelte";
  import FolderBar from "../components/FolderBar.svelte";
  import { filters, loadFolders, matchesFilter, folderById, folderColor } from "../folders.svelte";

  let tasks = $state<TaskItem[]>([]);
  let showDone = $state(false);
  let loaded = $state(false);

  async function load() {
    tasks = await api.listTasks(true);
    loaded = true;
    // Keep the sidebar badge in step with what this screen shows.
    app.openTasks = tasks.filter((x) => !x.done).length;
  }
  onMount(() => { load(); loadFolders(); });
  let lastRev = app.rev;
  $effect(() => { const r = app.rev; if (r !== lastRev) { lastRev = r; load(); } });

  const filter = $derived(filters.tasks);
  const inScope = $derived(tasks.filter((x) => matchesFilter(filter, { favorite: x.meetingFavorite, folderId: x.meetingFolderId })));
  const visible = $derived(inScope.filter((x) => showDone || !x.done));
  const groups = $derived.by(() => {
    const out: { id: string; title: string; startedAt: string; folderId?: string; items: TaskItem[] }[] = [];
    for (const x of visible) {
      let g = out.find((y) => y.id === x.meetingId);
      if (!g) { g = { id: x.meetingId, title: x.meetingTitle, startedAt: x.startedAt, folderId: x.meetingFolderId, items: [] }; out.push(g); }
      g.items.push(x);
    }
    return out.sort((a, b) => b.startedAt.localeCompare(a.startedAt));
  });
  const openCount = $derived(inScope.filter((x) => !x.done).length);

  async function toggle(x: TaskItem) {
    x.done = !x.done;
    await api.setTaskDone(x.meetingId, x.index, x.done);
    refreshTasks();
  }
</script>

<main>
  <div class="yc-scroll" style="flex: 1; min-height: 0">
    <div class="yc-in col">
      <header>
        <div>
          <h1>{t("Задачи")}</h1>
          <p class="lead">{openCount} {plural(openCount, ["открытая задача", "открытые задачи", "открытых задач"], ["open task", "open tasks"])} · {t("из итогов встреч")}</p>
        </div>
        <label class="sw"><span>{t("Показать выполненные")}</span><Switch bind:checked={showDone} aria-label={t("Показать выполненные")} /></label>
      </header>

      {#if loaded && (tasks.length > 0 || filter !== "all")}
        <div class="fb"><FolderBar value={filter} onchange={(v) => (filters.tasks = v)} manage={false} label="Папки встреч" /></div>
      {/if}

      {#if loaded && groups.length === 0}
        <p class="empty">{filter === "all" ? t("Все задачи выполнены — можно выдохнуть.") : t("В этой подборке открытых задач нет.")}</p>
      {/if}

      {#each groups as g (g.id)}
        <section class="grp">
          <div class="gh">
            <button type="button" class="yc-dash gt" onclick={() => go({ name: "meeting", id: g.id, tab: "summary" })}>{g.title}</button>
            {#if folderById(g.folderId) && filter !== g.folderId}<span class="gf"><span aria-hidden="true" class="cd" style="background: {folderColor(folderById(g.folderId))}"></span>{folderById(g.folderId)?.name}</span>{/if}
            <span class="mg-mono gd">{shortDate(g.startedAt)}</span>
          </div>
          <div class="list">
            {#each g.items as x (x.index)}
              <label class="task">
                <input type="checkbox" class="yc-check" checked={x.done} onchange={() => toggle(x)} />
                <span class={x.done ? "yc-task-done" : ""} style="flex: 1; min-width: 0; font-size: 15px">{x.text}</span>
                <span class="meta">{x.owner === "me" || !x.owner ? t("вы") : x.owner}{x.due ? " · " + dueLabel(x.due) : ""}</span>
                {#if x.atMs != null}
                  <button type="button" class="yc-tc mg-mono ts" aria-label={t("Перейти к моменту в расшифровке")} onclick={(e) => { e.preventDefault(); go({ name: "meeting", id: x.meetingId, tab: "transcript", at: x.atMs }); }}>▸ {clock(x.atMs)}</button>
                {/if}
              </label>
            {/each}
          </div>
        </section>
      {/each}
    </div>
  </div>
</main>

<style>
  main { flex: 1 1 520px; min-width: 0; display: flex; flex-direction: column; min-height: 0; }
  .col { max-width: 680px; box-sizing: border-box; margin: 0 auto; padding: 40px 28px 56px; display: flex; flex-direction: column; gap: 14px; }
  header { display: flex; align-items: flex-end; justify-content: space-between; gap: 16px; margin: 0 4px 6px; }
  h1 { margin: 0; font-size: 30px; line-height: 1.15; font-weight: 800; letter-spacing: -0.04em; }
  .lead { margin: 4px 0 0; font-size: 13px; color: var(--muted); }
  .sw { display: inline-flex; align-items: center; gap: 10px; font-size: 12px; color: var(--muted); }
  .empty { margin: 24px 4px; color: var(--muted); }
  .grp { border: 1px solid var(--border); border-radius: 16px; background: var(--surface); padding: 12px 16px 8px; display: flex; flex-direction: column; gap: 6px; }
  .gh { display: flex; align-items: baseline; gap: 10px; }
  .gt { flex: 1; min-width: 0; font-size: 13px; font-weight: 700; }
  .gd { font-size: 11px; color: var(--muted); }
  .gf { display: inline-flex; align-items: center; gap: 6px; font-size: 11.5px; font-weight: 600; color: var(--muted); }
  .cd { flex: none; width: 7px; height: 7px; border-radius: 50%; }
  .fb { margin: 0 4px -4px; }
  .list { display: flex; flex-direction: column; gap: 2px; }
  .task { display: flex; align-items: center; gap: 12px; padding: 5px 0; cursor: pointer; }
  .meta { flex: none; font-size: 12px; color: var(--muted); }
  .ts { flex: none; padding: 2px 7px; border: 1px solid var(--border); border-radius: 6px; background: var(--surface-2); color: var(--accent-text); font-size: 10px; font-weight: 700; }
</style>
