/** `Api` over Tauri `invoke`. Command names are the snake_case twins of the method names. */
import { invoke as rawInvoke, Channel } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type { Api, AskEvent, EngineEvent, UpdateEvent } from "./types";
import { toAppError } from "./error";

/** `invoke` that rejects with an `AppError` (short message + raw detail). */
const invoke = <T>(cmd: string, args?: Record<string, unknown>): Promise<T> => rawInvoke<T>(cmd, args).catch((e) => { throw toAppError(e); });

export function createTauriApi(): Api {
  const local = new Set<(e: EngineEvent) => void>();
  const fanOut = (e: EngineEvent) => local.forEach((l) => l(e));

  return {
    listMeetings: () => invoke("list_meetings"),
    getMeeting: (id) => invoke("get_meeting", { id }),
    renameMeeting: (id, title) => invoke("rename_meeting", { id, title }),
    deleteMeeting: (id) => invoke("delete_meeting", { id }),
    retranscribeMeeting: (id) => invoke("meeting_retranscribe", { id }),
    renameSpeaker: (id, key, name) => invoke("rename_speaker", { id, key, name }),
    setTaskDone: (id, index, done) => invoke("set_task_done", { id, index, done }),
    regenerateSummary: (id) => invoke("regenerate_summary", { id }),
    exportMeeting: (id, format) => invoke("export_meeting", { id, format }),
    revealMeeting: (id) => invoke("reveal_meeting", { id }),
    copyForClaude: (id) => invoke("copy_for_claude", { id }),
    listFolders: () => invoke("list_folders"),
    createFolder: (name, color) => invoke("create_folder", { name, color }),
    updateFolder: (id, patch) => invoke("update_folder", { id, patch }),
    deleteFolder: (id) => invoke("delete_folder", { id }),
    setMeetingFolder: (meetingId, folderId) => invoke("set_meeting_folder", { meetingId, folderId }),
    setMeetingFavorite: (meetingId, favorite) => invoke("set_meeting_favorite", { meetingId, favorite }),
    addBookmark: (meetingId, atMs, label) => invoke("add_bookmark", { meetingId, atMs, label }),
    updateBookmark: (meetingId, bookmarkId, label) => invoke("update_bookmark", { meetingId, bookmarkId, label }),
    removeBookmark: (meetingId, bookmarkId) => invoke("remove_bookmark", { meetingId, bookmarkId }),

    recorderState: () => invoke("recorder_state"),
    recorderStart: (opts) => invoke("recorder_start", { opts }),
    recorderPause: () => invoke("recorder_pause"),
    recorderResume: () => invoke("recorder_resume"),
    recorderStop: () => invoke("recorder_stop"),
    recorderCancel: () => invoke("recorder_cancel"),
    recorderBookmark: (label) => invoke("recorder_bookmark", { label }),
    listInputDevices: () => invoke("list_input_devices"),

    search: (query, folderId) => invoke("search", { query, folderId }),
    ask: (question, onEvent, folderId) => {
      const ch = new Channel<AskEvent>();
      ch.onmessage = onEvent;
      return invoke("ask", { question, onEvent: ch, folderId });
    },
    listTasks: (includeDone) => invoke("list_tasks", { includeDone }),
    importAudio: (paths) => invoke("import_audio", { paths }),
    listImports: () => invoke("list_imports"),

    modelsOverview: () => invoke("models_overview"),
    modelDownload: (id) => {
      const ch = new Channel<EngineEvent>();
      ch.onmessage = fanOut;
      return invoke("model_download", { id, progress: ch });
    },
    modelPause: (id) => invoke("model_pause", { id }),
    modelCancel: (id) => invoke("model_cancel", { id }),
    modelRemove: (id) => invoke("model_remove", { id }),
    modelSetActive: (id) => invoke("model_set_active", { id }),
    modelBenchmark: (id) => invoke("model_benchmark", { id }),

    ollamaStatus: (base) => invoke("ollama_status", { base }),
    testSummaryProvider: (mode) => invoke("test_summary_provider", { mode: mode ?? null }),
    getSettings: () => invoke("get_settings"),
    updateSettings: (patch) => invoke("update_settings", { patch }),
    pickFolder: async () => {
      const r = await open({ directory: true, multiple: false });
      return typeof r === "string" ? r : null;
    },
    permissions: () => invoke("permissions"),
    requestPermission: (kind) => invoke("request_permission", { kind }),
    openSystemSettings: (kind) => invoke("open_system_settings", { kind }),

    showMainWindow: () => invoke("show_main_window"),
    openInMain: (hash) => invoke("open_in_main", { hash }),
    popoverFit: (label, height) => invoke("popover_fit", { label, height }),
    hidePopover: (label) => invoke("hide_popover", { label }),
    quit: () => invoke("quit_app"),
    revealLogs: () => invoke("reveal_logs"),
    openAutomationSettings: () => invoke("open_automation_settings"),
    checkUpdate: () => invoke("check_update"),
    installUpdate: () => invoke("install_update"),
    async onUpdate(cb) {
      return listen<UpdateEvent>("update", (e) => cb(e.payload));
    },
    async onNavigate(cb) {
      const un = await listen<string>("navigate", (e) => cb(e.payload));
      return un;
    },
    async onEngine(cb) {
      local.add(cb);
      const un = await listen<EngineEvent>("engine", (e) => cb(e.payload));
      return () => { local.delete(cb); un(); };
    },
  };
}
