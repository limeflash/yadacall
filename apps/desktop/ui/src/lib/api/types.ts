/**
 * Hand-written mirror of the serde types in `yc-core` and `src-tauri/src/api.rs` (camelCase).
 * (tauri-specta is not used: it still pins pre-release tauri 2 internals; hand-written bindings are
 * a few hundred lines and are kept honest by `serde(rename_all = "camelCase")` on the Rust side.)
 */

export type Ms = number;
export type TrackKind = "mic" | "system";
export type Stage = "transcribe" | "diarize" | "index" | "summarize";

export type MeetingStatus =
  | { state: "recording" }
  | { state: "processing"; stage: Stage; progress: number }
  | { state: "ready" }
  | { state: "failed"; stage: Stage; message: string; retryable: boolean };

export interface Bookmark { atMs: Ms; label?: string }
export interface Participant { key: string; name: string; voiceId?: string }
export interface TrackInfo { kind: TrackKind; device: string; offsetMs: Ms; samples: number }

/** `meeting.json` */
export interface Meeting {
  id: string;
  title: string;
  startedAt: string;
  durationMs: Ms;
  language: string;
  expectedSpeakers?: number;
  tracks: TrackInfo[];
  bookmarks: Bookmark[];
  participants: Participant[];
  asrModel?: string;
  status: MeetingStatus;
}

export interface Segment { startMs: Ms; endMs: Ms; track: TrackKind; speaker: string; text: string; confidence?: number }
export interface Turn { speaker: string; startMs: Ms; endMs: Ms; text: string }

export interface Decision { text: string; atMs?: Ms }
export interface SummaryTask { text: string; owner?: string; due?: string; atMs?: Ms; done: boolean }
export interface Summary {
  title?: string;
  about: string[];
  decisions: Decision[];
  tasks: SummaryTask[];
  openQuestions: string[];
  language: string;
  model: string;
}

// ---- engine events (`yc_core::events::EngineEvent`, tag = "type") -------------------------------
export type RecorderState = "recording" | "paused" | "stopped" | "cancelled";
export interface Level { rms: number; peak: number }
export type DownloadState =
  | { state: "queued" } | { state: "running" } | { state: "paused" } | { state: "verifying" } | { state: "done" }
  | { state: "failed"; message: string; resumable: boolean };
export type NoticeLevel = "info" | "warning" | "error";

export type EngineEvent =
  | { type: "recorder"; meeting: string; state: RecorderState; elapsedMs: Ms }
  | { type: "levels"; mic: Level; system: Level }
  | { type: "trackSilent"; meeting: string; track: TrackKind; silentForMs: Ms }
  | { type: "deviceChanged"; track: TrackKind; device: string }
  | { type: "liveSegment"; meeting: string; segment: Segment }
  | { type: "status"; meeting: string; status: MeetingStatus }
  | { type: "download"; item: string; doneBytes: number; totalBytes: number; bytesPerSec: number; state: DownloadState }
  | { type: "notice"; level: NoticeLevel; message: string };

// ---- meetings ------------------------------------------------------------------------------------
export interface MeetingListItem {
  id: string;
  title: string;
  startedAt: string;
  durationMs: Ms;
  status: MeetingStatus;
  /** One-line gist shown under the title. */
  snippet?: string;
  /** e.g. "audio deleted after 7 days, text and summary kept". */
  audioNote?: string;
}

export interface MeetingDetail {
  meeting: Meeting;
  turns: Turn[];
  summary?: Summary;
  /** `asset://` URL of the mixed audio when it is still on disk. */
  audioUrl?: string;
  hasAudio: boolean;
}

export type ExportFormat = "md" | "json";

// ---- recorder ------------------------------------------------------------------------------------
export interface InputDevice { id: string; name: string; isDefault: boolean }
export interface RecorderStartOptions { title?: string; expectedSpeakers?: number; micDevice?: string }
export type RecorderPhase = "idle" | "recording" | "paused";
export interface RecorderSnapshot {
  phase: RecorderPhase;
  meetingId?: string;
  title?: string;
  elapsedMs: Ms;
  bookmarks: Ms[];
  micDevice?: string;
  /** Name of the app whose audio is captured, "Zoom" / "Система". */
  systemSource?: string;
}

// ---- search / ask --------------------------------------------------------------------------------
export interface SearchHit { meetingId: string; title: string; startedAt: string; atMs: Ms; speaker: string; text: string; score: number }
export interface Citation { n: number; meetingId: string; meetingTitle: string; startedAt: string; atMs: Ms; speaker: string; quote: string }
export interface SentNote { excerpts: number; provider: string; audio: boolean }
export type AskEvent =
  | { kind: "token"; text: string }
  | { kind: "citations"; items: Citation[] }
  | { kind: "done"; sent: SentNote }
  | { kind: "error"; message: string };

// ---- tasks / import ------------------------------------------------------------------------------
export interface TaskItem {
  meetingId: string; meetingTitle: string; startedAt: string; index: number;
  text: string; owner?: string; due?: string; atMs?: Ms; done: boolean;
}
export type ImportState = "queued" | "running" | "done" | "failed";
export interface ImportJob { id: string; fileName: string; state: ImportState; progress: number; meetingId?: string; error?: string }

// ---- models --------------------------------------------------------------------------------------
export type ModelKind = "asr" | "voice" | "embed" | "llm";
export type Verdict = "great" | "ok" | "slow" | "wontRun";
export interface ModelEntry {
  id: string;
  kind: ModelKind;
  name: string;
  sizeBytes: number;
  /** Short note: "точная, RU и EN". */
  blurb: string;
  installed: boolean;
  active: boolean;
  verdict: Verdict;
  /** One-line estimate: "≈ 40× быстрее реального времени". */
  estimate: string;
  /** Present while a download is queued / running / paused / failed. */
  download?: ModelDownload;
}
export interface ModelDownload {
  state: DownloadState;
  doneBytes: number;
  totalBytes: number;
  bytesPerSec: number;
  etaSecs?: number;
}
export interface HardwareProfile {
  label: string;           // "MacBook Pro"
  chip: string;            // "M1 Pro"
  ramGb: number;
  backend: string;         // "coreml" | "cuda" | "directml" | "cpu"
  diskFreeBytes: number;
  modelsBytes: number;
}
export interface ModelsOverview { hardware: HardwareProfile; models: ModelEntry[] }
export interface BenchmarkResult { modelId: string; realtimeFactor: number }

// ---- settings ------------------------------------------------------------------------------------
export type ThemeChoice = "system" | "light" | "dark";
export type UiLanguage = "ru" | "en";
export type KeepAudio = "none" | "7d" | "30d" | "forever";
export type SummaryProvider = "local" | "claude" | "openai" | "ollama";
export interface SummarySettings { provider: SummaryProvider; model: string; baseUrl?: string; apiKeySet: boolean }
export interface Settings {
  theme: ThemeChoice;
  accent: string;
  uiLanguage: UiLanguage;
  meetingLanguage: "auto" | "ru" | "en";
  keepAudio: KeepAudio;
  libraryDir: string;
  hotkeyRecord: string;
  hotkeyBookmark: string;
  launchAtLogin: boolean;
  recordingReminder: boolean;
  recognizeVoices: boolean;
  miniPlayer: boolean;
  summaries: SummarySettings;
  onboarded: boolean;
}
/** Partial update; `summaries.apiKey` is written to the OS keychain by the real backend. */
export type SettingsPatch = Partial<Omit<Settings, "summaries">> & { summaries?: Partial<SummarySettings> & { apiKey?: string } };

// ---- permissions ---------------------------------------------------------------------------------
export type PermissionState = "granted" | "denied" | "notDetermined";
export interface Permissions { microphone: PermissionState; systemAudio: PermissionState }
export type PermissionKind = "microphone" | "systemAudio";

/** Everything the UI can ask of the engine. Implemented by Tauri `invoke` and by the browser mock. */
export interface Api {
  // meetings
  listMeetings(): Promise<MeetingListItem[]>;
  getMeeting(id: string): Promise<MeetingDetail>;
  renameMeeting(id: string, title: string): Promise<void>;
  deleteMeeting(id: string): Promise<void>;
  renameSpeaker(id: string, key: string, name: string): Promise<void>;
  setTaskDone(id: string, index: number, done: boolean): Promise<void>;
  regenerateSummary(id: string): Promise<Summary>;
  exportMeeting(id: string, format: ExportFormat): Promise<string>;
  revealMeeting(id: string): Promise<void>;
  copyForClaude(id: string): Promise<string>;
  // recorder
  recorderState(): Promise<RecorderSnapshot>;
  recorderStart(opts: RecorderStartOptions): Promise<RecorderSnapshot>;
  recorderPause(): Promise<void>;
  recorderResume(): Promise<void>;
  recorderStop(): Promise<string>;
  recorderCancel(): Promise<void>;
  recorderBookmark(label?: string): Promise<void>;
  listInputDevices(): Promise<InputDevice[]>;
  // search / ask / tasks / import
  search(query: string): Promise<SearchHit[]>;
  ask(question: string, onEvent: (e: AskEvent) => void): Promise<void>;
  listTasks(includeDone: boolean): Promise<TaskItem[]>;
  importAudio(paths: string[]): Promise<ImportJob[]>;
  listImports(): Promise<ImportJob[]>;
  // models
  modelsOverview(): Promise<ModelsOverview>;
  modelDownload(id: string): Promise<void>;
  modelPause(id: string): Promise<void>;
  modelCancel(id: string): Promise<void>;
  modelRemove(id: string): Promise<void>;
  modelSetActive(id: string): Promise<void>;
  modelBenchmark(id: string): Promise<BenchmarkResult>;
  // settings / permissions
  getSettings(): Promise<Settings>;
  updateSettings(patch: SettingsPatch): Promise<Settings>;
  pickFolder(): Promise<string | null>;
  permissions(): Promise<Permissions>;
  requestPermission(kind: PermissionKind): Promise<Permissions>;
  openSystemSettings(kind: PermissionKind): Promise<void>;
  // shell
  showMainWindow(): Promise<void>;
  /** Show the main window and navigate it to a hash route (used by the tray popover). */
  openInMain(hash: string): Promise<void>;
  /** Popover windows report their content height so the shell can fit and re-anchor them to the tray icon. */
  popoverFit(label: "tray" | "mini", height: number): Promise<void>;
  hidePopover(label: "tray" | "mini"): Promise<void>;
  quit(): Promise<void>;
  /** Hash routes requested by other windows (tray popover). */
  onNavigate(cb: (hash: string) => void): Promise<() => void>;
  /** Subscribe to engine events. Returns an unsubscribe function. */
  onEngine(cb: (e: EngineEvent) => void): Promise<() => void>;
}
