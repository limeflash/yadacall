<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import Button from "../kit/Button.svelte";
  import { api, inTauri } from "../api";
  import type { ImportJob } from "../api";
  import { t } from "../i18n/index.svelte";
  import { go } from "../router.svelte";
  import { toast } from "../toast.svelte";

  let jobs = $state<ImportJob[]>([]);
  let over = $state(false);
  let fileInput: HTMLInputElement;
  const EXT = ["wav", "mp3", "m4a", "aac", "flac", "ogg", "opus", "mp4", "mov", "mkv", "webm"];

  async function refresh() { jobs = await api.listImports(); }
  onMount(() => {
    refresh();
    const iv = setInterval(refresh, 700);
    let unlisten: (() => void) | undefined;
    if (inTauri) {
      import("@tauri-apps/api/webview").then(async ({ getCurrentWebview }) => {
        unlisten = await getCurrentWebview().onDragDropEvent((e) => {
          const p = e.payload;
          if (p.type === "over") over = true;
          else if (p.type === "leave") over = false;
          else if (p.type === "drop") { over = false; add(p.paths); }
        });
      });
    }
    return () => { clearInterval(iv); unlisten?.(); };
  });

  async function add(paths: string[]) {
    const ok = paths.filter((p) => EXT.includes((p.split(".").pop() ?? "").toLowerCase()));
    if (!ok.length) { toast.error(t("Нужен аудио- или видеофайл: wav, mp3, m4a, mp4…")); return; }
    await api.importAudio(ok); refresh();
  }
  async function choose() {
    if (inTauri) {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const r = await open({ multiple: true, filters: [{ name: "Audio / video", extensions: EXT }] });
      if (r) await add(Array.isArray(r) ? r : [r]);
    } else fileInput.click();
  }
  const stateLabel = (j: ImportJob) => j.state === "queued" ? t("в очереди") : j.state === "running" ? t("Распознаю локально") : j.state === "done" ? t("готово") : t("ошибка");
</script>

<main>
  <div class="yc-scroll" style="flex: 1; min-height: 0">
    <div class="yc-in col">
      <h1>{t("Загрузить запись")}</h1>
      <p class="lead">{t("Перетащи аудио или видео — расшифруем на компьютере так же, как живую запись.")}</p>

      <div class="drop" class:over role="button" tabindex="0" onclick={choose} onkeydown={(e) => (e.key === "Enter" || e.key === " ") && choose()}
        ondragover={(e) => { e.preventDefault(); if (!inTauri) over = true; }} ondragleave={() => !inTauri && (over = false)}
        ondrop={(e) => { e.preventDefault(); over = false; if (!inTauri) add([...(e.dataTransfer?.files ?? [])].map((f) => f.name)); }}>
        <span class="dic"><Icon name="upload" size={22} /></span>
        <span style="font-size: 15px; font-weight: 800">{over ? t("Отпусти, чтобы добавить") : t("Перетащи файлы сюда")}</span>
        <span style="font-size: 12px; color: var(--muted)">{t("или")} <span style="color: var(--accent-text); font-weight: 700">{t("выбери на компьютере")}</span> · wav, mp3, m4a, mp4, mov…</span>
      </div>
      <input bind:this={fileInput} type="file" multiple accept="audio/*,video/*" hidden onchange={(e) => { add([...(e.currentTarget.files ?? [])].map((f) => f.name)); e.currentTarget.value = ""; }} />

      {#if jobs.length}
        <div style="padding: 10px 4px 0; font-size: 12px; font-weight: 600; color: var(--muted)">{t("Очередь")}</div>
        <div class="list">
          {#each jobs as j (j.id)}
            <div class="job">
              <div style="display: flex; align-items: baseline; gap: 12px">
                <span class="yc-ellipsis" style="flex: 1; font-size: 14px; font-weight: 700">{j.fileName}</span>
                {#if j.state === "done" && j.meetingId}<Button size="xs" onclick={() => go({ name: "meeting", id: j.meetingId!, tab: "summary" })}>{t("Открыть")}</Button>{/if}
                <span class="mg-mono yc-num" style="font-size: 12px; color: {j.state === 'failed' ? 'var(--danger-text)' : j.state === 'done' ? 'var(--ok)' : 'var(--accent-text)'}">{j.state === "running" ? Math.round(j.progress * 100) + "%" : stateLabel(j)}</span>
              </div>
              {#if j.state === "running" || j.state === "queued"}
                <span aria-hidden="true" class="bar"><span style="width: {Math.round(j.progress * 100)}%"></span></span>
              {/if}
              {#if j.error}<span style="font-size: 12px; color: var(--danger-text)">{j.error}</span>{/if}
            </div>
          {/each}
        </div>
      {/if}
    </div>
  </div>
</main>

<style>
  main { flex: 1 1 520px; min-width: 0; display: flex; flex-direction: column; min-height: 0; }
  .col { max-width: 600px; box-sizing: border-box; margin: 0 auto; padding: 40px 28px 56px; display: flex; flex-direction: column; gap: 10px; }
  h1 { margin: 0 0 4px 4px; font-size: 30px; line-height: 1.15; font-weight: 800; letter-spacing: -0.04em; }
  .lead { margin: 0 4px 12px; font-size: 13px; color: var(--muted); }
  .drop { display: flex; flex-direction: column; align-items: center; gap: 8px; padding: 40px 20px; border: 1.5px dashed var(--faint); border-radius: 18px; background: var(--surface); text-align: center; cursor: pointer; transition: border-color 0.2s, background-color 0.2s, transform 0.35s var(--ease-spring); }
  .drop:hover, .drop.over { border-color: var(--accent); background: var(--accent-soft); }
  .drop.over { transform: scale(1.01); }
  .dic { width: 48px; height: 48px; border-radius: 14px; background: var(--accent-soft); color: var(--accent-text); display: grid; place-items: center; }
  .list { display: flex; flex-direction: column; gap: 6px; }
  .job { display: flex; flex-direction: column; gap: 8px; padding: 12px 14px; border: 1px solid var(--border); border-radius: 14px; background: var(--surface); }
  .bar { position: relative; display: block; height: 6px; border-radius: 3px; overflow: hidden; background: var(--surface-2); }
  .bar span { position: absolute; left: 0; top: 0; bottom: 0; border-radius: 3px; background: linear-gradient(90deg, var(--accent), var(--accent-soft), var(--accent)); background-size: 200% 100%; animation: ycShimmer 1.6s linear infinite; transition: width 0.2s; }
</style>
