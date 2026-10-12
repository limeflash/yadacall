use tauri_build::{AppManifest, Attributes};

// Keep in sync with `COMMANDS` in src/lib.rs. Each command becomes an `allow-<name>` permission that the
// capability files grant per window (main: everything; tray/mini: the recorder subset).
const COMMANDS: &[&str] = &[
    "list_meetings",
    "get_meeting",
    "rename_meeting",
    "delete_meeting",
    "rename_speaker",
    "set_task_done",
    "regenerate_summary",
    "export_meeting",
    "reveal_meeting",
    "copy_for_claude",
    "recorder_state",
    "recorder_start",
    "recorder_pause",
    "recorder_resume",
    "recorder_stop",
    "recorder_cancel",
    "recorder_bookmark",
    "list_input_devices",
    "search",
    "ask",
    "list_tasks",
    "import_audio",
    "list_imports",
    "models_overview",
    "model_download",
    "model_pause",
    "model_cancel",
    "model_remove",
    "model_set_active",
    "model_benchmark",
    "get_settings",
    "update_settings",
    "permissions",
    "request_permission",
    "open_system_settings",
    "show_main_window",
    "open_in_main",
    "popover_fit",
    "hide_popover",
    "quit_app",
];

fn main() {
    tauri_build::try_build(Attributes::new().app_manifest(AppManifest::new().commands(COMMANDS)))
        .expect("failed to run tauri-build");
}
