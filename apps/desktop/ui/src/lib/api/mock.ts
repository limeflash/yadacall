/**
 * Browser-only implementation of `Api` (used by `pnpm dev` without Tauri). It mirrors `src-tauri/src/demo.rs`:
 * same demo library, a simulated recorder (levels, live phrases), processing progress and model downloads.
 */
import type {
  Api, AskEvent, BenchmarkResult, Bookmark, Folder, EngineEvent, ImportJob, InputDevice, MeetingDetail, MeetingListItem, ModelEntry, ModelsOverview,
  OllamaBase, OllamaStatus, Permissions, ProviderTest, SummaryMode, RecorderSnapshot, RecorderStartOptions, SearchHit, Settings, SettingsPatch, Summary, TaskItem, Turn, Meeting,
} from "./types";
import { mergeSummaries, noKeys } from "./summaries";
import { AppError } from "./error";
import { mockCatalog } from "./mock_models";
import { demoAsk, demoMeetings, demoModels, LIVE_SCRIPT, type DemoMeeting } from "./demo";

const SETTINGS_KEY = "yadacall.mock.settings";
const defaults: Settings = {
  theme: "dark", accent: "#7dd3a0", uiLanguage: "ru", meetingLanguage: "auto", keepAudio: "30d", libraryDir: "~/YadaCall",
  hotkeyRecord: "Alt+Shift+R", hotkeyBookmark: "Alt+Shift+M", bookmarkScreenshot: "window", launchAtLogin: false, recordingReminder: true, recognizeVoices: true, terms: [], miniPlayer: true,
  summaries: { mode: "local", model: "", keysSet: { ...noKeys, anthropic: true } }, onboarded: true,
};

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const uid = () => "0199d1a0-0000-7000-8000-" + Math.random().toString(16).slice(2, 14).padEnd(12, "0");

export function createMockApi(): Api {
  const listeners = new Set<(e: EngineEvent) => void>();
  const emit = (e: EngineEvent) => listeners.forEach((l) => l(e));

  let settings: Settings = { ...defaults };
  try { const s = localStorage.getItem(SETTINGS_KEY); if (s) settings = { ...defaults, ...JSON.parse(s) }; if (!("mode" in settings.summaries)) settings.summaries = defaults.summaries; } catch { /* private mode */ }
  const persist = () => { try { localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings)); } catch { /* ignore */ } };
  // `?onboarding` in the URL lets screenshots start from the first-run flow.
  if (typeof location !== "undefined" && /onboarding/.test(location.hash)) settings = { ...settings, onboarded: false };

  const meetings: DemoMeeting[] = demoMeetings();

  // ---- favourites / folders / bookmarks demo state -------------------------------------------------
  const folders: Folder[] = [
    { id: "fld-work", name: "Работа", color: "mint", order: 0 },
    { id: "fld-personal", name: "Личная", color: "rose", order: 1 },
    { id: "fld-projects", name: "Проекты", color: "lavender", order: 2 },
  ];
  /** A small generated "screenshot" so the bookmark thumbnail can be seen without the capture package. */
  const shot = (a: string, b: string) => "data:image/svg+xml;utf8," + encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" width="160" height="100" viewBox="0 0 160 100"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${a}"/><stop offset="1" stop-color="${b}"/></linearGradient></defs><rect width="160" height="100" fill="url(#g)"/><rect x="14" y="14" width="78" height="9" rx="4" fill="#fff" fill-opacity=".55"/><rect x="14" y="31" width="132" height="6" rx="3" fill="#fff" fill-opacity=".32"/><rect x="14" y="43" width="112" height="6" rx="3" fill="#fff" fill-opacity=".32"/><rect x="14" y="62" width="54" height="24" rx="7" fill="#fff" fill-opacity=".4"/></svg>`);
  const seed = (id: string, patch: { favorite?: boolean; folderId?: string }) => {
    const m = meetings.find((x) => x.meeting.id.endsWith(id)); if (!m) return;
    Object.assign(m.meeting, patch); Object.assign(m.list, patch);
  };
  seed("000002", { favorite: true, folderId: "fld-projects" }); seed("000003", { folderId: "fld-work" }); seed("000004", { folderId: "fld-personal" });
  seed("000005", { folderId: "fld-projects" }); seed("000006", { favorite: true, folderId: "fld-work" }); seed("000007", { folderId: "fld-projects" });
  const sync = meetings.find((x) => x.meeting.id.endsWith("000002"));
  if (sync) sync.meeting.bookmarks.push(
    { id: "bm-demo-1", atMs: 1_520_000, label: "Договорились про сроки", createdAfter: true, screenshot: shot("#6e50c0", "#2a2a4a") },
    { id: "bm-demo-2", atMs: 2_105_000, createdAfter: true },
  );
  const daysLeft = (iso: string) => 30 - Math.floor((Date.now() - Date.parse(iso)) / 86400000);
  /** What the engine would put in `audioNote` for a meeting that is not starred. */
  const audioNoteOf = (m: DemoMeeting) => {
    if (m.meeting.favorite) return undefined;
    if (!m.hasAudio) return m.list.audioNote;
    const d = daysLeft(m.meeting.startedAt);
    return d > 0 && settings.keepAudio === "30d" ? `аудио удалится через ${d} дн., текст и итоги останутся` : undefined;
  };
  const { hardware, devices } = demoModels();
  const freshInstall = typeof location !== "undefined" && /onboarding/.test(location.hash);
  const models: ModelEntry[] = mockCatalog(freshInstall);
  const imports: ImportJob[] = [];
  let perms: Permissions = { microphone: freshInstall ? "notDetermined" : "granted", systemAudio: "notDetermined", screenRecording: "granted" };

  // ---- processing meeting: progress loops as in the design ------------------------------------------
  const proc = meetings.find((m) => m.meeting.status.state === "processing");
  if (proc) {
    setInterval(() => {
      if (proc.meeting.status.state !== "processing") return;
      const p = proc.meeting.status.progress;
      const next = p >= 0.99 ? 0.12 : p + 0.01;
      proc.meeting.status = { state: "processing", stage: "transcribe", progress: next };
      proc.list.status = proc.meeting.status;
      emit({ type: "status", meeting: proc.meeting.id, status: proc.meeting.status });
    }, 220);
  }

  // ---- model downloads ------------------------------------------------------------------------------
  setInterval(() => {
    for (const m of models) {
      const d = m.download;
      if (!d || d.state.state !== "running") continue;
      d.doneBytes = Math.min(d.totalBytes, d.doneBytes + d.bytesPerSec / 4);
      d.etaSecs = d.bytesPerSec ? Math.ceil((d.totalBytes - d.doneBytes) / d.bytesPerSec) : undefined;
      if (d.doneBytes >= d.totalBytes) { m.installed = true; m.download = undefined; emit({ type: "download", item: m.id, doneBytes: d.totalBytes, totalBytes: d.totalBytes, bytesPerSec: 0, state: { state: "done" } }); continue; }
      emit({ type: "download", item: m.id, doneBytes: d.doneBytes, totalBytes: d.totalBytes, bytesPerSec: d.bytesPerSec, state: d.state });
    }
  }, 250);

  // ---- recorder simulation --------------------------------------------------------------------------
  let rec: { id: string; title: string; startedAt: number; pausedAt?: number; pausedTotal: number; marks: number[]; turns: Turn[]; mic?: string; speakers?: number } | null = null;
  let who: "me" | "them" | "none" | "both" = "me", left = 70, liveIdx = 0, nextLive = 0;
  const elapsed = () => (rec ? (rec.pausedAt ?? Date.now()) - rec.startedAt - rec.pausedTotal : 0);
  const snapshot = (): RecorderSnapshot => rec
    ? { phase: rec.pausedAt ? "paused" : "recording", meetingId: rec.id, title: rec.title, elapsedMs: elapsed(), bookmarks: [...rec.marks], micDevice: rec.mic ?? "MacBook Pro", systemSource: "Zoom" }
    : { phase: "idle", elapsedMs: 0, bookmarks: [] };

  setInterval(() => {
    if (!rec || rec.pausedAt) return;
    if (--left <= 0) {
      const r = Math.random();
      who = r < 0.42 ? "me" : r < 0.86 ? "them" : r < 0.95 ? "none" : "both";
      left = 12 + Math.floor(Math.random() * 35);
    }
    const tt = performance.now() / 1000;
    const lv = (on: boolean, k: number) => { const a = on ? 0.35 + 0.55 * Math.abs(Math.sin(tt * k)) : 0.02; return { rms: a * 0.6, peak: a }; };
    emit({ type: "levels", mic: lv(who === "me" || who === "both", 7.3), system: lv(who === "them" || who === "both", 6.1) });
  }, 40);
  setInterval(() => {
    if (!rec) return;
    emit({ type: "recorder", meeting: rec.id, state: rec.pausedAt ? "paused" : "recording", elapsedMs: elapsed() });
    if (!rec.pausedAt && elapsed() >= nextLive && liveIdx < LIVE_SCRIPT.length) {
      const l = LIVE_SCRIPT[liveIdx++];
      const startMs = elapsed();
      const turn: Turn = { speaker: l.speaker, startMs, endMs: startMs + 4000, text: l.text };
      rec.turns.push(turn);
      emit({ type: "liveSegment", meeting: rec.id, segment: { ...l, startMs, endMs: startMs + 4000 } });
      nextLive = elapsed() + 6000;
    }
  }, 500);

  function summaryFor(turns: Turn[]): Summary {
    return { about: ["Ревью дизайна личного кабинета: обсудили карточки профиля и поведение кнопки на мобильных."], decisions: [{ text: "Карточки профиля — как в прошлой версии", atMs: turns[2]?.startMs }], tasks: [{ text: "Проверить кнопку над клавиатурой на iPhone", owner: "me", done: false, atMs: turns[1]?.startMs }], openQuestions: [], language: "ru", model: "local:gemma-4-e4b" };
  }

  const find = (id: string) => { const m = meetings.find((x) => x.meeting.id === id); if (!m) throw new Error("not found"); return m; };
  const toTasks = (all: boolean): TaskItem[] => meetings.flatMap((m) => (m.summary?.tasks ?? []).map((t, index) => ({
    meetingId: m.meeting.id, meetingTitle: m.meeting.title, startedAt: m.meeting.startedAt, index, text: t.text,
    owner: t.owner && t.owner !== "me" ? m.meeting.participants.find((p) => p.key === t.owner)?.name ?? t.owner : t.owner, due: t.due, atMs: t.atMs, done: t.done,
    meetingFolderId: m.meeting.folderId, meetingFavorite: m.meeting.favorite,
  }))).filter((t) => all || !t.done);

  const api: Api = {
    async listMeetings() {
      const list: MeetingListItem[] = meetings.map((m) => ({ ...m.list, status: m.meeting.status, audioNote: audioNoteOf(m) }));
      if (rec) list.unshift({ id: rec.id, title: rec.title || "Без названия", startedAt: new Date(rec.startedAt).toISOString(), durationMs: elapsed(), status: { state: "recording" }, snippet: "Идёт запись — текст пишется на лету", favorite: false });
      return list.sort((a, b) => b.startedAt.localeCompare(a.startedAt));
    },
    async getMeeting(id): Promise<MeetingDetail> {
      const m = find(id);
      return { meeting: structuredClone(m.meeting), turns: m.turns, summary: m.summary, hasAudio: m.hasAudio };
    },
    async renameMeeting(id, title) { const m = find(id); m.meeting.title = title; m.list.title = title; },
    async deleteMeeting(id) { const i = meetings.findIndex((m) => m.meeting.id === id); if (i >= 0) meetings.splice(i, 1); },
    async retranscribeMeeting() { await sleep(600); },
    async renameSpeaker(id, key, name) {
      const m = find(id); const p = m.meeting.participants.find((x) => x.key === key);
      const other = m.meeting.participants.find((x) => x.key !== key && x.name === name);
      if (other) { m.turns.forEach((t) => { if (t.speaker === key) t.speaker = other.key; }); m.meeting.participants = m.meeting.participants.filter((x) => x.key !== key); return { mergedInto: { key: other.key, name } }; }
      if (p) p.name = name; else m.meeting.participants.push({ key, name });
      return {};
    },
    async setTaskDone(id, index, done) { const t = find(id).summary?.tasks[index]; if (t) t.done = done; },
    async regenerateSummary(id) { await sleep(900); return find(id).summary ?? summaryFor([]); },
    async exportMeeting(id, format) { return `~/YadaCall/${find(id).meeting.title}/transcript.${format}`; },
    async revealMeeting() {},
    async copyForClaude(id) {
      const m = find(id);
      const name = (k: string) => (k === "me" ? "Я" : m.meeting.participants.find((p) => p.key === k)?.name ?? "Гость");
      const fmt = (ms: number) => `${String(Math.floor(ms / 60000)).padStart(2, "0")}:${String(Math.floor(ms / 1000) % 60).padStart(2, "0")}`;
      return `# ${m.meeting.title}\n\n` + m.turns.map((t) => `[${fmt(t.startMs)}] ${name(t.speaker)}: ${t.text}`).join("\n");
    },

    async listFolders() { return folders.map((f) => ({ ...f })).sort((a, b) => a.order - b.order); },
    async createFolder(name, color) {
      const f: Folder = { id: uid(), name: name.trim(), color, order: Math.max(-1, ...folders.map((x) => x.order)) + 1 };
      folders.push(f); return { ...f };
    },
    async updateFolder(id, patch) {
      const f = folders.find((x) => x.id === id); if (!f) throw new Error("not found");
      if (patch.name !== undefined) f.name = patch.name.trim() || f.name;
      if (patch.color !== undefined) f.color = patch.color;
      if (patch.order !== undefined) f.order = patch.order;
      return { ...f };
    },
    async deleteFolder(id) {
      const i = folders.findIndex((x) => x.id === id); if (i >= 0) folders.splice(i, 1);
      for (const m of meetings) if (m.meeting.folderId === id) { m.meeting.folderId = undefined; m.list.folderId = undefined; }
    },
    async setMeetingFolder(meetingId, folderId) { const m = find(meetingId); m.meeting.folderId = m.list.folderId = folderId ?? undefined; },
    async setMeetingFavorite(meetingId, favorite) { const m = find(meetingId); m.meeting.favorite = m.list.favorite = favorite; },
    async addBookmark(meetingId, atMs, label) {
      const b: Bookmark = { id: uid(), atMs: Math.round(atMs), label: label?.trim() || undefined, createdAfter: true };
      find(meetingId).meeting.bookmarks.push(b); return { ...b };
    },
    async updateBookmark(meetingId, bookmarkId, label) {
      const b = find(meetingId).meeting.bookmarks.find((x) => x.id === bookmarkId); if (!b) throw new Error("not found");
      b.label = label?.trim() || undefined; return { ...b };
    },
    async removeBookmark(meetingId, bookmarkId) { const m = find(meetingId).meeting; m.bookmarks = m.bookmarks.filter((x) => x.id !== bookmarkId); },

    async recorderState() { return snapshot(); },
    async recorderStart(opts: RecorderStartOptions) {
      if (rec) return snapshot();
      who = "me"; left = 75; liveIdx = 0; nextLive = 4000;
      rec = { id: uid(), title: opts.title?.trim() || "", startedAt: Date.now(), pausedTotal: 0, marks: [], turns: [], mic: devices.find((d) => d.id === opts.micDevice)?.name, speakers: opts.expectedSpeakers };
      emit({ type: "recorder", meeting: rec.id, state: "recording", elapsedMs: 0 });
      return snapshot();
    },
    async recorderPause() { if (rec && !rec.pausedAt) { rec.pausedAt = Date.now(); emit({ type: "recorder", meeting: rec.id, state: "paused", elapsedMs: elapsed() }); } },
    async recorderResume() { if (rec?.pausedAt) { rec.pausedTotal += Date.now() - rec.pausedAt; rec.pausedAt = undefined; emit({ type: "recorder", meeting: rec.id, state: "recording", elapsedMs: elapsed() }); } },
    async recorderStop() {
      if (!rec) return "";
      const r = rec; rec = null;
      const dur = Date.now() - r.startedAt - r.pausedTotal;
      const meeting: Meeting = { id: r.id, title: r.title || "Без названия", startedAt: new Date(r.startedAt).toISOString(), durationMs: dur, language: "ru", tracks: [], bookmarks: r.marks.map((atMs) => ({ id: uid(), atMs })), favorite: false, participants: [], status: { state: "processing", stage: "transcribe", progress: 0 } };
      const dm: DemoMeeting = { list: { id: r.id, title: meeting.title, startedAt: meeting.startedAt, durationMs: dur, status: meeting.status, snippet: "Собираю итоги…", favorite: false }, meeting, turns: r.turns, hasAudio: true };
      meetings.unshift(dm);
      emit({ type: "recorder", meeting: r.id, state: "stopped", elapsedMs: dur });
      let p = 0;
      const stages = ["transcribe", "diarize", "index", "summarize"] as const;
      const iv = setInterval(() => {
        p += 0.03 + Math.random() * 0.02;
        if (p >= 1) {
          clearInterval(iv);
          dm.summary = summaryFor(r.turns);
          dm.list.snippet = dm.summary.about[0];
          meeting.status = dm.list.status = { state: "ready" };
          emit({ type: "status", meeting: r.id, status: meeting.status });
          return;
        }
        meeting.status = dm.list.status = { state: "processing", stage: stages[Math.min(3, Math.floor(p * 4))], progress: p };
        emit({ type: "status", meeting: r.id, status: meeting.status });
      }, 900);
      return r.id;
    },
    async recorderCancel() { if (rec) { emit({ type: "recorder", meeting: rec.id, state: "cancelled", elapsedMs: elapsed() }); rec = null; } },
    async recorderBookmark(label) {
      const atMs = elapsed(); if (rec) rec.marks.push(atMs);
      return { id: uid(), atMs, label };
    },
    async listInputDevices(): Promise<InputDevice[]> { return devices; },

    async search(q, folderId): Promise<SearchHit[]> {
      const needle = q.trim().toLowerCase();
      if (!needle) return [];
      return meetings.filter((m) => !folderId || m.meeting.folderId === folderId).flatMap((m) => m.turns.filter((t) => t.text.toLowerCase().includes(needle)).map((t) => ({
        meetingId: m.meeting.id, title: m.meeting.title, startedAt: m.meeting.startedAt, atMs: t.startMs, speaker: t.speaker, text: t.text, score: 1,
      })));
    },
    async ask(_q, onEvent: (e: AskEvent) => void, folderId?: string) {
      const a = demoAsk();
      if (folderId) a.citations = a.citations.filter((c) => meetings.find((m) => m.meeting.id === c.meetingId)?.meeting.folderId === folderId);
      const m = settings.summaries;
      await sleep(500);
      if (m.mode !== "off") {
        const words = a.answer.split(/(?<=\s)/);
        for (const w of words) { onEvent({ kind: "token", text: w }); await sleep(35); }
      }
      onEvent({ kind: "citations", items: a.citations });
      const provider = m.mode === "api" ? (m.provider === "anthropic" ? "Claude" : m.provider === "openRouter" ? "OpenRouter" : "API") : m.mode === "ollama" ? "Ollama Cloud" : "local";
      onEvent({ kind: "done", sent: { excerpts: provider === "local" ? 0 : a.excerpts, provider, audio: false }, generated: m.mode !== "off" });
    },
    async listTasks(includeDone) { return toTasks(includeDone); },
    async importAudio(paths) {
      const jobs = paths.map<ImportJob>((p) => ({ id: uid(), fileName: p.split(/[\\/]/).pop() ?? p, state: "queued", progress: 0 }));
      imports.unshift(...jobs);
      jobs.forEach((j, i) => {
        setTimeout(() => {
          j.state = "running";
          const iv = setInterval(() => { j.progress = Math.min(1, j.progress + 0.04); if (j.progress >= 1) { j.state = "done"; clearInterval(iv); } }, 200);
        }, 600 + i * 1500);
      });
      return jobs;
    },
    async listImports() { return imports; },

    async modelsOverview(): Promise<ModelsOverview> {
      const bytes = models.filter((m) => m.installed).reduce((s, m) => s + m.sizeBytes, 0);
      return { hardware: { ...hardware, modelsBytes: bytes }, models: models.map((m) => ({ ...m, download: m.download ? { ...m.download } : undefined })) };
    },
    async modelDownload(id) {
      const m = models.find((x) => x.id === id); if (!m) return;
      const done = m.download?.doneBytes ?? 0;
      m.download = { state: { state: "running" }, doneBytes: done, totalBytes: m.sizeBytes, bytesPerSec: 6_200_000 };
    },
    async modelPause(id) { const d = models.find((x) => x.id === id)?.download; if (d) d.state = { state: "paused" }; },
    async modelCancel(id) { const m = models.find((x) => x.id === id); if (m) m.download = undefined; },
    /** Like the engine: refuses during a recording, frees the files and any partial download, and un-primaries the removed model. */
    async modelRemove(id) {
      const m = models.find((x) => x.id === id); if (!m) return;
      if (rec) throw new AppError("Идёт запись — модель можно удалить после неё", "a recording is in progress");
      if (m.tags.includes("required")) throw new AppError("Эта модель нужна для записи", "model is required");
      m.installed = false; m.active = false; m.download = undefined;
    },
    async modelSetActive(id) {
      const m = models.find((x) => x.id === id); if (!m || !m.installed) return;
      models.filter((x) => x.kind === m.kind).forEach((x) => (x.active = x.id === id));
    },
    async modelBenchmark(id): Promise<BenchmarkResult> { await sleep(1200); return { modelId: id, realtimeFactor: 38 + Math.round(Math.random() * 8) }; },

    async ollamaStatus(base: OllamaBase): Promise<OllamaStatus> {
      await sleep(250);
      const cloud = (name: string) => ({ name, sizeBytes: 338, cloud: true });
      if (/ollama-(down|signedout)/.test(location.hash + location.search)) return { running: !/down/.test(location.hash + location.search), signedIn: false, models: [{ name: "gemma4:e4b", sizeBytes: 5_000_000_000, cloud: false }] };
      if (base === "cloud") return settings.summaries.keysSet.ollamaCloud ? { running: true, signedIn: true, models: [cloud("gpt-oss:120b"), cloud("glm-5.2"), cloud("qwen3-coder:480b")] } : { running: false, signedIn: false, models: [], error: "no key" };
      return { running: true, version: "0.40.3", signedIn: true, models: [cloud("glm-5.2:cloud"), { name: "gemma4:e4b", sizeBytes: 5_000_000_000, cloud: false }, { name: "embeddinggemma:latest", sizeBytes: 621_875_917, cloud: false }] };
    },
    async testSummaryProvider(_mode?: SummaryMode): Promise<ProviderTest> { await sleep(900); return { ok: true, latencyMs: 1100, reply: "Да, я на связи." }; },

    async getSettings() { return structuredClone(settings); },
    async updateSettings(patch: SettingsPatch) {
      const { summaries, ...rest } = patch;
      settings = { ...settings, ...rest, summaries: mergeSummaries(settings.summaries, summaries) };
      persist();
      return structuredClone(settings);
    },
    async pickFolder() { return "~/Documents/YadaCall"; },
    async permissions() { return perms; },
    async requestPermission(kind) { perms = { ...perms, [kind]: "granted" }; return perms; },
    async openSystemSettings() {},
    async showMainWindow() {},
    async openInMain(hash) { location.hash = hash; },
    async popoverFit() {},
    async hidePopover() {},
    async quit() {},
    async onNavigate() { return () => {}; },
    async revealLogs() {},
    async openAutomationSettings() {},
    async checkUpdate() { await sleep(400); return { available: false }; },
    async installUpdate() { throw new Error("no update in the browser mock"); },
    async onUpdate() { return () => {}; },
    async onEngine(cb) { listeners.add(cb); return () => listeners.delete(cb); },
  };
  return api;
}
