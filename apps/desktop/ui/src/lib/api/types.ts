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
export interface RenameOutcome { mergedInto?: { key: string; name: string } }
export interface ParticipantHint { name: string; email?: string }
export interface Participant { key: string; name: string; voiceId?: string; /** Invite names ranked as likely identities of this speaker. */ suggestions?: string[] }
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
  /** People invited to the calendar event behind the recording. */
  participantHints?: ParticipantHint[];
  asrModel?: string;
  status: MeetingStatus;
}

export interface Segment { startMs: Ms; endMs: Ms; track: TrackKind; speaker: string; text: string; confidence?: number; corrections?: TextCorrection[] }
export interface TextCorrection { original: string; replacement: string }
export interface Turn { speaker: string; startMs: Ms; endMs: Ms; text: string; /** Term-dictionary replacements inside this turn. */ corrections?: TextCorrection[] }

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
  /** `generated: false` — summaries are off, only the matching passages (citations) were found. */
  | { kind: "done"; sent: SentNote; generated?: boolean }
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
/** Languages a model is good at; "multilingual" means many, Russian included, unless tags say otherwise. */
export type ModelLang = "ru" | "en" | "multilingual";
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
  /** False for models that cannot be measured (voice-activity detection); absent means true. */
  benchmarkable?: boolean;
  /** Present while a download is queued / running / paused / failed. */
  download?: ModelDownload;

  /** Who made it, short and human ("Сбер", "NVIDIA", "Qwen · Alibaba"). Shown as-is, not translated. */
  vendor: string;
  /** Model family for grouping and search ("GigaAM", "Gemma 4", "QVikhr"). */
  family: string;
  languages: ModelLang[];
  /** For "multilingual" models: how many languages ("25 языков"); absent means "many". */
  languageCount?: number;
  /** Machine-readable traits: "ru-tuned", "moe", "english-only", "reasoning-off", "streaming", "tiny". Unknown ones are ignored. */
  tags: string[];
  /** Total parameters, e.g. 4.0e9. */
  params?: number;
  /** Active parameters (mixture-of-experts only). */
  activeParams?: number;
  /** SPDX-ish id or a custom name: "Apache-2.0", "MIT", "YandexGPT-5-Lite". */
  license: string;
  licenseUrl?: string;
  /** Not a standard permissive license (custom terms, usage thresholds): the UI flags it. */
  licenseCustom?: boolean;
  homepage?: string;
  /** The backend's pick for this machine within its kind (ASR: one per language group). */
  recommended?: boolean;
  /** Relative quality inside its kind, higher is better. */
  qualityRank?: number;
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
/** The llama.cpp runtime under the LLM section; `id` goes to `modelDownload` to reinstall it. */
export interface RuntimeInfo { id: string; name: string; sizeBytes: number; installed: boolean; busy: boolean }
export interface ModelsOverview { hardware: HardwareProfile; models: ModelEntry[]; runtime?: RuntimeInfo }
export interface BenchmarkResult { modelId: string; realtimeFactor: number }

// ---- updates -------------------------------------------------------------------------------------
export interface UpdateStatus { available: boolean; version?: string; notes?: string; date?: string }
export type UpdateEvent =
  | { event: "available"; version: string; notes?: string; date?: string }
  | { event: "downloading"; downloadedBytes: number; contentLength?: number }
  | { event: "downloadProgress"; downloadedBytes: number; contentLength?: number }
  | { event: "downloaded" }
  | { event: "installDeferred"; reason: string }
  | { event: "installed" }
  | { event: "openSheet" };

// ---- settings ------------------------------------------------------------------------------------
export type ThemeChoice = "system" | "light" | "dark";
export type UiLanguage = "ru" | "en";
export type KeepAudio = "none" | "7d" | "30d" | "forever";
export type OllamaBase = "local" | "cloud";
export type ApiProvider = "anthropic" | "openAiCompatible" | "openRouter" | "lmStudio";
/** How summaries (and Ask answers) are produced. */
export type SummaryMode =
  | { mode: "off" }
  /** `model`: catalog id of an on-device Gemma, empty = automatic. */
  | { mode: "local"; model: string }
  | { mode: "ollama"; base: OllamaBase; model: string }
  | { mode: "api"; provider: ApiProvider; model: string; baseUrl?: string };
export type KeyAccount = "anthropic" | "openAi" | "openRouter" | "ollamaCloud";
/** Which keys are in the OS keychain (values never reach the UI). */
export type KeysSet = Record<KeyAccount, boolean>;
export type SummarySettings = SummaryMode & { keysSet: KeysSet };
export interface OllamaModel { name: string; sizeBytes: number; cloud: boolean }
export interface OllamaStatus { running: boolean; version?: string; signedIn: boolean; models: OllamaModel[]; error?: string }
export interface ProviderTest { ok: boolean; latencyMs: number; reply?: string; error?: string }
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
  /** The user's own terms (product names, jargon) the transcript should spell correctly. */
  terms: string[];
  miniPlayer: boolean;
  summaries: SummarySettings;
  onboarded: boolean;
}
/** Partial update; `summaries.key` is written to the OS keychain by the real backend (an empty value removes it). */
export type SettingsPatch = Partial<Omit<Settings, "summaries">> & { summaries?: { mode?: SummaryMode; key?: { account: KeyAccount; value: string } } };

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
  /** Re-runs recognition on the kept audio (`meeting_retranscribe`, engine branch). */
  retranscribeMeeting(id: string): Promise<void>;
  /** `mergedInto` is set when the name already belonged to another voice: the two were merged. */
  renameSpeaker(id: string, key: string, name: string): Promise<RenameOutcome>;
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
  // summary providers
  /** Probes the local Ollama daemon, or lists Ollama Cloud with the stored key. */
  ollamaStatus(base: OllamaBase): Promise<OllamaStatus>;
  /** One short request with `mode` (the saved one when omitted), timed. */
  testSummaryProvider(mode?: SummaryMode): Promise<ProviderTest>;
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
  /** Opens the folder with the app's log files. */
  revealLogs(): Promise<void>;
  /** Opens System Settings > Privacy > Automation (macOS). */
  openAutomationSettings(): Promise<void>;
  quit(): Promise<void>;
  /** Hash routes requested by other windows (tray popover). */
  onNavigate(cb: (hash: string) => void): Promise<() => void>;
  /** Application updates (`src-tauri/src/updates.rs`). */
  checkUpdate(): Promise<UpdateStatus>;
  /** Downloads and installs the update, then the app restarts; rejects while a recording is active. */
  installUpdate(): Promise<void>;
  /** Update events: found by the background check, download progress, tray click. */
  onUpdate(cb: (e: UpdateEvent) => void): Promise<() => void>;
  /** Subscribe to engine events. Returns an unsubscribe function. */
  onEngine(cb: (e: EngineEvent) => void): Promise<() => void>;
}

// ---- integrations (`src-tauri/src/integrations_api.rs`) -------------------------------------------
export type RecordPolicy = "ask" | "always" | "never";
export type AccountStatus = "ok" | "expired" | "connecting";

export interface CalendarAccount {
  id: string;
  /** "Рабочий", "Личный". */
  label: string;
  email: string;
  /** Avatar colour, `#rrggbb`. */
  color: string;
  workspace: boolean;
  status: AccountStatus;
  meetingsThisWeek: number;
  recordPolicy: RecordPolicy;
  /** "В календаре": show the account's events in the agenda. */
  visible: boolean;
}

export interface TimeRange { from: string; to: string }

export interface AgendaEvent {
  id: string;
  accountId: string;
  title: string;
  start: string;
  end: string;
  meetUrl?: string;
  /** Other participants (without the user). */
  attendees: string[];
  recordPolicy: RecordPolicy;
  /** Account policy + the user's per-event choice. */
  willRecord: boolean;
  /** Ids of visible events of other accounts that run at the same time. */
  overlaps: string[];
  /** Library meeting recorded for this event. */
  meetingId?: string;
}

export interface JoinOutcome {
  title: string;
  /** Link opened in the browser by the shell; absent when there is nothing to open. */
  meetUrl?: string;
  accountEmail: string;
  record: boolean;
  endsAt: string;
  /** Calendar event the call belongs to. */
  eventId?: string;
  /** Remote people expected; absent when the guest list is unknown. */
  expectedSpeakers?: number;
  /** Display names of the other attendees. */
  participants?: { name: string; email?: string }[];
}

/** "unconfigured": this build has no Google OAuth client; `docsUrl` explains how to add one. */
export type ConnectionState = "connected" | "soon" | "unconfigured";
export interface Connection {
  id: string;
  name: string;
  mono: string;
  description: string;
  state: ConnectionState;
  accounts?: number;
  /** "Хочу такое": a purely local flag, never sent anywhere. */
  want: boolean;
  docsUrl?: string;
}

export interface ReminderSettings {
  /** 1, 2 or 5. */
  remindMinutes: number;
  openInAccount: boolean;
  detectUnscheduled: boolean;
  /** The user pressed "Разрешить": detection may read browser tabs (macOS asks for Automation access then). */
  browserAccess: boolean;
  /** Browser macOS blocked from Automation (tab access), when it did. */
  browserAutomationPermissionRequired?: string;
}

export interface DetectedCall { id: string; platform: string; /** Absent for a generic browser call. */ url?: string; code?: string; /** Page title or "Звонок в Brave". */ title?: string; detectedAt: string }

/** The `integrations` Tauri event (tag = "type"). */
export type IntegrationEvent =
  | { type: "reminderDue"; event: AgendaEvent; accountLabel: string; accountEmail: string; minutesBefore: number }
  | { type: "callStarting"; event: AgendaEvent; accountLabel: string; accountEmail: string }
  | { type: "callDetected"; call: DetectedCall }
  | { type: "authExpired"; accountId: string; email: string }
  | { type: "addProgress"; step: number }
  | { type: "agendaChanged" };

/** Calendar accounts, agenda, connections catalog and reminders. Tauri `invoke` or the browser mock. */
export interface IntegrationsApi {
  accounts(): Promise<CalendarAccount[]>;
  /** Resolves with the new account once the browser round-trip finished; rejects with "cancelled" after `cancelAddAccount`. */
  addGoogleAccount(): Promise<CalendarAccount>;
  cancelAddAccount(): Promise<void>;
  removeAccount(id: string): Promise<void>;
  reauthAccount(id: string): Promise<CalendarAccount>;
  setAccountPolicy(id: string, policy: RecordPolicy): Promise<void>;
  setAccountVisible(id: string, visible: boolean): Promise<void>;
  agenda(range: TimeRange): Promise<AgendaEvent[]>;
  setEventRecording(eventId: string, record: boolean): Promise<void>;
  joinAndRecord(eventId: string): Promise<JoinOutcome>;
  joinWithoutRecording(eventId: string): Promise<JoinOutcome>;
  connections(): Promise<Connection[]>;
  /** Flips "Хочу такое"; resolves with the new value. */
  toggleWant(service: string): Promise<boolean>;
  reminderSettings(): Promise<ReminderSettings>;
  setReminderSettings(s: ReminderSettings): Promise<ReminderSettings>;
  detectedCall(): Promise<DetectedCall | null>;
  resolveDetectedCall(id: string, record: boolean): Promise<void>;
  onIntegrations(cb: (e: IntegrationEvent) => void): Promise<() => void>;
}
