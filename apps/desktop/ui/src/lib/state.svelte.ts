import { api, inTauri } from "./api";
import type { EngineEvent, Level, RecorderSnapshot, Segment, Settings, SettingsPatch } from "./api";
import { i18n, t } from "./i18n/index.svelte";
import { toast } from "./toast.svelte";

/** Global reactive app state. Components read from here; engine events and API calls write to it. */
export const app = $state({
  ready: false,
  settings: null as Settings | null,
  systemDark: typeof matchMedia !== "undefined" && matchMedia("(prefers-color-scheme: dark)").matches,
  recorder: { phase: "idle", elapsedMs: 0, bookmarks: [] } as RecorderSnapshot,
  /** Wall-clock anchor: `Date.now() - elapsed` while recording, so the timer ticks smoothly between events. */
  anchor: 0,
  now: Date.now(),
  live: [] as Segment[],
  silentTrack: null as null | "mic" | "system",
  openTasks: 0,
  /** Bumped when meetings change (status events, deletes) so lists refetch. */
  rev: 0,
  downloads: {} as Record<string, { done: number; total: number; state: string; bytesPerSec: number }>,
});

/** Live input levels, ~25 Hz. Plain object: read by canvas loops, not by templates. */
export const levels: { mic: Level; system: Level } = { mic: { rms: 0, peak: 0 }, system: { rms: 0, peak: 0 } };

export const theme = {
  get resolved(): "light" | "dark" {
    const t = app.settings?.theme ?? "dark";
    return t === "system" ? (app.systemDark ? "dark" : "light") : t;
  },
  get accent(): string { return app.settings?.accent ?? "#7dd3a0"; },
};

/** Elapsed recording time in ms (0 when idle). */
export function elapsedMs(): number {
  const r = app.recorder;
  if (r.phase === "recording") return Math.max(0, app.now - app.anchor);
  return r.elapsedMs;
}

function setRecorder(s: RecorderSnapshot) {
  app.recorder = s;
  app.anchor = Date.now() - s.elapsedMs;
  if (s.phase === "idle") { app.live = []; app.silentTrack = null; }
}

export async function refreshTasks() {
  try { app.openTasks = (await api.listTasks(false)).length; } catch { /* backend not ready */ }
}

export async function saveSettings(patch: SettingsPatch) {
  // optimistic: theme / accent / language must feel instant
  if (app.settings) {
    const { summaries, ...rest } = patch;
    app.settings = { ...app.settings, ...rest, summaries: { ...app.settings.summaries, ...(summaries ?? {}), apiKeySet: app.settings.summaries.apiKeySet || !!summaries?.apiKey } };
    if (patch.uiLanguage) i18n.lang = patch.uiLanguage;
  }
  try { app.settings = await api.updateSettings(patch); } catch (e) { toast.error(String(e)); }
}

const handlers: ((e: EngineEvent) => void)[] = [];
/** Subscribe to the raw engine stream (returns unsubscribe). */
export function onEngineEvent(h: (e: EngineEvent) => void) { handlers.push(h); return () => { const i = handlers.indexOf(h); if (i >= 0) handlers.splice(i, 1); }; }

function onEvent(e: EngineEvent) {
  switch (e.type) {
    case "recorder":
      if (e.state === "recording" || e.state === "paused") {
        app.recorder = { ...app.recorder, phase: e.state === "paused" ? "paused" : "recording", meetingId: e.meeting, elapsedMs: e.elapsedMs };
        app.anchor = Date.now() - e.elapsedMs;
      } else {
        setRecorder({ phase: "idle", elapsedMs: 0, bookmarks: [] });
        app.rev++;
      }
      break;
    case "levels":
      levels.mic = e.mic; levels.system = e.system; break;
    case "liveSegment":
      app.live = [...app.live, e.segment].slice(-40); app.silentTrack = null; break;
    case "trackSilent":
      app.silentTrack = e.track; break;
    case "status":
      app.rev++;
      if (e.status.state === "ready") refreshTasks();
      break;
    case "download":
      app.downloads[e.item] = { done: e.doneBytes, total: e.totalBytes, state: e.state.state, bytesPerSec: e.bytesPerSec };
      if (e.state.state === "done") setTimeout(() => delete app.downloads[e.item], 1500);
      break;
    case "notice":
      if (e.level === "error") toast.error(e.message); else toast(e.message);
      break;
  }
  handlers.forEach((h) => h(e));
}

export async function startRecording(opts: { title?: string; expectedSpeakers?: number; micDevice?: string } = {}) {
  try {
    setRecorder(await api.recorderStart(opts));
    app.rev++;
  } catch (e) { toast.error(t("Не удалось начать запись") + ": " + String(e)); }
}
export async function stopRecording(): Promise<string | undefined> {
  try { const id = await api.recorderStop(); setRecorder({ phase: "idle", elapsedMs: 0, bookmarks: [] }); app.rev++; return id; }
  catch (e) { toast.error(String(e)); }
}
export async function togglePause() {
  if (app.recorder.phase === "recording") { await api.recorderPause(); app.recorder = { ...app.recorder, phase: "paused", elapsedMs: elapsedMs() }; }
  else if (app.recorder.phase === "paused") { await api.recorderResume(); app.anchor = Date.now() - app.recorder.elapsedMs; app.recorder = { ...app.recorder, phase: "recording" }; }
}
export async function addBookmark() {
  if (app.recorder.phase === "idle") return;
  const at = elapsedMs();
  await api.recorderBookmark();
  app.recorder = { ...app.recorder, bookmarks: [...app.recorder.bookmarks, at] };
}

let started = false;
export async function initApp() {
  if (started) return;
  started = true;
  if (typeof matchMedia !== "undefined") matchMedia("(prefers-color-scheme: dark)").addEventListener("change", (e) => { app.systemDark = e.matches; });
  setInterval(() => { app.now = Date.now(); }, 200);
  await api.onEngine(onEvent);
  await api.onNavigate((hash) => { location.hash = hash; });
  try {
    const [s, r] = await Promise.all([api.getSettings(), api.recorderState()]);
    app.settings = s; i18n.lang = s.uiLanguage; setRecorder(r);
  } catch (e) { console.error("init failed", e); }
  refreshTasks();
  app.ready = true;
  if (inTauri) document.documentElement.classList.add("tauri");
}
