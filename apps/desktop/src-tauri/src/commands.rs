//! Tauri commands: one-liners into the [`Backend`]. Names are the snake_case twins of the methods in
//! `ui/src/lib/api/tauri.ts`; the same list is the ACL allow-list in `build.rs` and `capabilities/*.json`.

use std::sync::Arc;

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};
use yc_core::events::EngineEvent;
use yc_core::summary::Summary;

use crate::api::*;
use crate::shell;

/// Error type sent to the UI: a plain, user-presentable string.
#[derive(Debug)]
pub struct CmdError(String);

impl From<yc_core::Error> for CmdError {
    fn from(e: yc_core::Error) -> Self {
        CmdError(e.to_string())
    }
}

impl Serialize for CmdError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

type B<'a> = State<'a, Arc<dyn Backend>>;
type R<T> = Result<T, CmdError>;

// ---- meetings -----------------------------------------------------------------------------------------

#[tauri::command]
pub async fn list_meetings(b: B<'_>) -> R<Vec<MeetingListItem>> {
    Ok(b.list_meetings().await?)
}
#[tauri::command]
pub async fn get_meeting(b: B<'_>, id: String) -> R<MeetingDetail> {
    Ok(b.get_meeting(&id).await?)
}
#[tauri::command]
pub async fn rename_meeting(b: B<'_>, id: String, title: String) -> R<()> {
    Ok(b.rename_meeting(&id, &title).await?)
}
#[tauri::command]
pub async fn delete_meeting(b: B<'_>, id: String) -> R<()> {
    Ok(b.delete_meeting(&id).await?)
}
#[tauri::command]
pub async fn rename_speaker(b: B<'_>, id: String, key: String, name: String) -> R<()> {
    Ok(b.rename_speaker(&id, &key, &name).await?)
}
#[tauri::command]
pub async fn set_task_done(b: B<'_>, id: String, index: usize, done: bool) -> R<()> {
    Ok(b.set_task_done(&id, index, done).await?)
}
#[tauri::command]
pub async fn regenerate_summary(b: B<'_>, id: String) -> R<Summary> {
    Ok(b.regenerate_summary(&id).await?)
}
#[tauri::command]
pub async fn export_meeting(b: B<'_>, id: String, format: ExportFormat) -> R<String> {
    Ok(b.export_meeting(&id, format).await?)
}
#[tauri::command]
pub async fn copy_for_claude(b: B<'_>, id: String) -> R<String> {
    Ok(b.copy_for_claude(&id).await?)
}

#[tauri::command]
pub async fn reveal_meeting(b: B<'_>, id: String) -> R<()> {
    if let Some(dir) = b.meeting_dir(&id).await? {
        let _ = tauri_plugin_opener::reveal_item_in_dir(dir);
    }
    Ok(())
}

// ---- recorder -----------------------------------------------------------------------------------------

#[tauri::command]
pub async fn recorder_state(b: B<'_>) -> R<RecorderSnapshot> {
    Ok(b.recorder_state().await?)
}
#[tauri::command]
pub async fn recorder_start(b: B<'_>, opts: RecorderStartOptions) -> R<RecorderSnapshot> {
    Ok(b.recorder_start(opts).await?)
}
#[tauri::command]
pub async fn recorder_pause(b: B<'_>) -> R<()> {
    Ok(b.recorder_pause().await?)
}
#[tauri::command]
pub async fn recorder_resume(b: B<'_>) -> R<()> {
    Ok(b.recorder_resume().await?)
}
#[tauri::command]
pub async fn recorder_stop(b: B<'_>) -> R<String> {
    Ok(b.recorder_stop().await?)
}
#[tauri::command]
pub async fn recorder_cancel(b: B<'_>) -> R<()> {
    Ok(b.recorder_cancel().await?)
}
#[tauri::command]
pub async fn recorder_bookmark(b: B<'_>, label: Option<String>) -> R<()> {
    Ok(b.recorder_bookmark(label).await?)
}
#[tauri::command]
pub async fn list_input_devices(b: B<'_>) -> R<Vec<InputDevice>> {
    Ok(b.list_input_devices().await?)
}

// ---- search / ask / tasks / import --------------------------------------------------------------------------

#[tauri::command]
pub async fn search(b: B<'_>, query: String) -> R<Vec<SearchHit>> {
    Ok(b.search(&query).await?)
}
#[tauri::command]
pub async fn ask(b: B<'_>, question: String, on_event: Channel<AskEvent>) -> R<()> {
    let sink: AskSink = Arc::new(move |e| {
        let _ = on_event.send(e);
    });
    Ok(b.ask(&question, sink).await?)
}
#[tauri::command]
pub async fn list_tasks(b: B<'_>, include_done: bool) -> R<Vec<TaskItem>> {
    Ok(b.list_tasks(include_done).await?)
}
#[tauri::command]
pub async fn import_audio(b: B<'_>, paths: Vec<String>) -> R<Vec<ImportJob>> {
    Ok(b.import_audio(paths).await?)
}
#[tauri::command]
pub async fn list_imports(b: B<'_>) -> R<Vec<ImportJob>> {
    Ok(b.list_imports().await?)
}

// ---- models -------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn models_overview(b: B<'_>) -> R<ModelsOverview> {
    Ok(b.models_overview().await?)
}
#[tauri::command]
pub async fn model_download(b: B<'_>, id: String, progress: Channel<EngineEvent>) -> R<()> {
    let sink: EventSink = Arc::new(move |e| {
        let _ = progress.send(e);
    });
    Ok(b.model_download(&id, sink).await?)
}
#[tauri::command]
pub async fn model_pause(b: B<'_>, id: String) -> R<()> {
    Ok(b.model_pause(&id).await?)
}
#[tauri::command]
pub async fn model_cancel(b: B<'_>, id: String) -> R<()> {
    Ok(b.model_cancel(&id).await?)
}
#[tauri::command]
pub async fn model_remove(b: B<'_>, id: String) -> R<()> {
    Ok(b.model_remove(&id).await?)
}
#[tauri::command]
pub async fn model_set_active(b: B<'_>, id: String) -> R<()> {
    Ok(b.model_set_active(&id).await?)
}
#[tauri::command]
pub async fn model_benchmark(b: B<'_>, id: String) -> R<BenchmarkResult> {
    Ok(b.model_benchmark(&id).await?)
}

// ---- settings / permissions ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_settings(b: B<'_>) -> R<Settings> {
    Ok(b.get_settings().await?)
}
#[tauri::command]
pub async fn update_settings(app: AppHandle, b: B<'_>, patch: SettingsPatch) -> R<Settings> {
    let s = b.update_settings(patch).await?;
    shell::apply_settings(&app, &s);
    Ok(s)
}
#[tauri::command]
pub async fn permissions(b: B<'_>) -> R<Permissions> {
    Ok(b.permissions().await?)
}
#[tauri::command]
pub async fn request_permission(b: B<'_>, kind: PermissionKind) -> R<Permissions> {
    Ok(b.request_permission(kind).await?)
}
#[tauri::command]
pub async fn open_system_settings(b: B<'_>, kind: PermissionKind) -> R<()> {
    b.open_system_settings(kind).await?;
    shell::open_os_privacy_settings(kind);
    Ok(())
}

// ---- shell --------------------------------------------------------------------------------------------

#[tauri::command]
pub fn show_main_window(app: AppHandle) {
    shell::show_main(&app);
}

/// Shows the main window and asks it to navigate to a hash route (`#/meeting/<id>/summary`).
#[tauri::command]
pub fn open_in_main(app: AppHandle, hash: String) {
    shell::show_main(&app);
    let _ = app.emit_to("main", "navigate", hash);
}

#[tauri::command]
pub fn popover_fit(app: AppHandle, label: String, height: f64) {
    shell::fit_popover(&app, &label, height);
}

#[tauri::command]
pub fn hide_popover(app: AppHandle, label: String) {
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.hide();
    }
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}
