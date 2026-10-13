//! Tauri commands: one-liners into the [`Backend`]. Names are the snake_case twins of the methods in
//! `ui/src/lib/api/tauri.ts`; the same list is the ACL allow-list in `build.rs` and `capabilities/*.json`.

use std::sync::Arc;

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, State};
use yc_core::events::EngineEvent;
use yc_core::meeting::MeetingStatus;
use yc_core::summary::Summary;

use crate::api::*;
use crate::shell;

/// Error sent to the UI: a short user-facing `message` (Russian; the UI translates it) and the raw
/// error text as `detail` for an expandable "Подробнее".
#[derive(Debug, Serialize)]
pub struct CmdError {
    message: String,
    detail: String,
}

impl From<yc_core::Error> for CmdError {
    fn from(e: yc_core::Error) -> Self {
        tracing::warn!("command failed: {e}");
        CmdError {
            message: friendly(&e).to_owned(),
            detail: e.to_string(),
        }
    }
}

/// A short, actionable message for `e`, in Russian (the UI maps it to the interface language).
pub fn friendly(e: &yc_core::Error) -> &'static str {
    use yc_core::Error as E;
    use yc_core::error::PermissionKind;
    match e {
        E::Permission(PermissionKind::Microphone) => {
            "Нет доступа к микрофону. Разреши его в настройках системы."
        }
        E::Permission(PermissionKind::SystemAudio) => {
            "Нет доступа к системному звуку. Разреши его в настройках системы."
        }
        E::Device(_) => "Проблема с аудиоустройством. Проверь, что оно подключено, и повтори.",
        E::ModelMissing(_) => "Нужная модель не установлена. Скачай её в разделе «Модели».",
        E::Model { .. } => "Модель не смогла выполнить задачу. Попробуй ещё раз или выбери другую.",
        E::DiskFull { .. } => "Не хватает места на диске. Освободи его и повтори.",
        E::Network(_) => "Нет связи с сервером. Проверь интернет и повтори.",
        E::Integrity(_) => "Файл повреждён. Скачай его заново.",
        E::Storage(_) => "Не удалось прочитать или сохранить данные приложения.",
        E::Llm(_) => "Языковая модель не ответила. Проверь настройки итогов и повтори.",
        E::Cancelled => "Отменено.",
        E::NotFound(_) => "Не найдено: возможно, это уже удалено.",
        E::Invalid(m) if m.contains("GOOGLE_OAUTH.md") => {
            "Вход через Google не настроен в этой сборке."
        }
        E::Invalid(m) if m.contains("active recording") => {
            "Идёт запись. Заверши или отмени её, потом обнови приложение."
        }
        E::Invalid(m) if m.contains("sign in again") => "Вход в Google истёк. Войди заново.",
        E::Invalid(m) if m.contains("sign-in was denied") => "Вход в Google отменён.",
        E::Invalid(_) => "Сейчас это действие недоступно.",
        E::Io(_) => "Ошибка чтения или записи файла.",
    }
}

/// Same idea for an error that only survived as text (`Error::to_string()` of the engine's own errors).
fn friendly_known(text: &str) -> Option<&'static str> {
    Some(if text.starts_with("llm provider error") {
        "Языковая модель не ответила. Проверь настройки итогов и повтори."
    } else if text.starts_with("network error") {
        "Нет связи с сервером. Проверь интернет и повтори."
    } else if text.contains("is not installed") {
        "Нужная модель не установлена. Скачай её в разделе «Модели»."
    } else if text.starts_with("not enough disk space") {
        "Не хватает места на диске. Освободи его и повтори."
    } else if text.starts_with("audio device error") {
        "Проблема с аудиоустройством. Проверь, что оно подключено, и повтори."
    } else if text.starts_with("model `") {
        "Модель не смогла выполнить задачу. Попробуй ещё раз или выбери другую."
    } else if text.starts_with("storage error") || text.starts_with("integrity check") {
        "Не удалось прочитать или сохранить данные приложения."
    } else {
        return None;
    })
}

/// Message for a failed answer.
pub fn friendly_text(text: &str) -> &'static str {
    friendly_known(text).unwrap_or("Не удалось получить ответ. Попробуй ещё раз.")
}

/// Replaces the raw engine text of a failed meeting by a short message (the UI translates it).
pub fn friendly_status(status: MeetingStatus) -> MeetingStatus {
    match status {
        MeetingStatus::Failed {
            stage,
            message,
            retryable,
        } => MeetingStatus::Failed {
            stage,
            message: friendly_known(&message)
                .unwrap_or("Не удалось обработать запись. Попробуй повторить обработку.")
                .to_owned(),
            retryable,
        },
        other => other,
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
pub async fn meeting_retranscribe(b: B<'_>, id: String) -> R<()> {
    Ok(b.meeting_retranscribe(&id).await?)
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
pub async fn ollama_status(b: B<'_>, base: OllamaBase) -> R<OllamaStatus> {
    Ok(b.ollama_status(base).await?)
}

#[tauri::command]
pub async fn test_summary_provider(b: B<'_>, mode: Option<SummaryMode>) -> R<ProviderTest> {
    Ok(b.test_summary_provider(mode).await?)
}

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
    shell::hide_popover_window(&app, &label);
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

/// Checks the configured release endpoint for a newer application version.
#[tauri::command]
pub async fn check_update(app: AppHandle) -> R<crate::updates::UpdateStatus> {
    Ok(crate::updates::check(&app).await?)
}

/// Installs an available update after confirming that no recording is active.
#[tauri::command]
pub async fn install_update(app: AppHandle, b: B<'_>) -> R<()> {
    Ok(crate::updates::install(&app, b.inner().as_ref()).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yc_core::Error;

    #[test]
    fn errors_become_short_user_messages_and_keep_the_raw_text() {
        let e = CmdError::from(Error::Network("dns error: lookup failed".into()));
        assert!(e.message.starts_with("Нет связи"));
        assert_eq!(e.detail, "network error: dns error: lookup failed");
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["message"], e.message);
        assert!(!friendly(&Error::Invalid("`x` cannot be benchmarked".into())).is_ascii());
        assert!(
            friendly(&Error::Invalid(
                "Google sign-in is not configured: see docs/GOOGLE_OAUTH.md".into()
            ))
            .contains("Google")
        );
        assert!(friendly_text("llm provider error: 401").starts_with("Языковая"));
        let failed = MeetingStatus::Failed {
            stage: yc_core::meeting::Stage::Transcribe,
            message: "model `x` failed: oops".into(),
            retryable: true,
        };
        let MeetingStatus::Failed { message, .. } = friendly_status(failed) else {
            unreachable!()
        };
        assert!(message.starts_with("Модель"));
    }
}
