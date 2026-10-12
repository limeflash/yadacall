//! Tauri commands for the integrations: one-liners into [`Integrations`]. Names are the snake_case twins of
//! the methods in `ui/src/lib/api/tauri.ts`; the list lives in `INTEGRATION_COMMANDS` (also the ACL allow-list
//! in `build.rs` and `capabilities/*.json`).

use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::api::Backend;
use crate::commands::CmdError;
use crate::integrations_api::*;
use crate::integrations_shell::apply_join;

type I<'a> = State<'a, Arc<dyn Integrations>>;
type R<T> = Result<T, CmdError>;

// ---- accounts -----------------------------------------------------------------------------------------

#[tauri::command]
pub async fn accounts(i: I<'_>) -> R<Vec<CalendarAccount>> {
    Ok(i.accounts().await?)
}
#[tauri::command]
pub async fn add_google_account(i: I<'_>) -> R<CalendarAccount> {
    Ok(i.add_google_account().await?)
}
#[tauri::command]
pub async fn cancel_add_account(i: I<'_>) -> R<()> {
    Ok(i.cancel_add_account().await?)
}
#[tauri::command]
pub async fn remove_account(i: I<'_>, id: String) -> R<()> {
    Ok(i.remove_account(&id).await?)
}
#[tauri::command]
pub async fn reauth_account(i: I<'_>, id: String) -> R<CalendarAccount> {
    Ok(i.reauth_account(&id).await?)
}
#[tauri::command]
pub async fn set_account_policy(i: I<'_>, id: String, policy: RecordPolicy) -> R<()> {
    Ok(i.set_account_policy(&id, policy).await?)
}
#[tauri::command]
pub async fn set_account_visible(i: I<'_>, id: String, visible: bool) -> R<()> {
    Ok(i.set_account_visible(&id, visible).await?)
}

// ---- agenda -------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn agenda(i: I<'_>, range: TimeRange) -> R<Vec<AgendaEvent>> {
    Ok(i.agenda(range).await?)
}
#[tauri::command]
pub async fn set_event_recording(i: I<'_>, event_id: String, record: bool) -> R<()> {
    Ok(i.set_event_recording(&event_id, record).await?)
}
#[tauri::command]
pub async fn join_and_record(
    app: AppHandle,
    i: I<'_>,
    b: State<'_, Arc<dyn Backend>>,
    event_id: String,
) -> R<JoinOutcome> {
    let out = i.join_and_record(&event_id).await?;
    apply_join(&app, &b, &out).await;
    Ok(out)
}
#[tauri::command]
pub async fn join_without_recording(
    app: AppHandle,
    i: I<'_>,
    b: State<'_, Arc<dyn Backend>>,
    event_id: String,
) -> R<JoinOutcome> {
    let out = i.join_without_recording(&event_id).await?;
    apply_join(&app, &b, &out).await;
    Ok(out)
}

// ---- catalog / reminders ------------------------------------------------------------------------------

#[tauri::command]
pub async fn connections(i: I<'_>) -> R<Vec<Connection>> {
    Ok(i.connections().await?)
}
#[tauri::command]
pub async fn toggle_want(i: I<'_>, service: String) -> R<bool> {
    Ok(i.toggle_want(&service).await?)
}
#[tauri::command]
pub async fn reminder_settings(i: I<'_>) -> R<ReminderSettings> {
    Ok(i.reminder_settings().await?)
}
#[tauri::command]
pub async fn set_reminder_settings(i: I<'_>, settings: ReminderSettings) -> R<ReminderSettings> {
    Ok(i.set_reminder_settings(settings).await?)
}
#[tauri::command]
pub async fn detected_call(i: I<'_>) -> R<Option<DetectedCall>> {
    Ok(i.detected_call().await?)
}
#[tauri::command]
pub async fn resolve_detected_call(
    app: AppHandle,
    i: I<'_>,
    b: State<'_, Arc<dyn Backend>>,
    id: String,
    record: bool,
) -> R<()> {
    if let Some(out) = i.resolve_detected_call(&id, record).await? {
        apply_join(&app, &b, &out).await;
    }
    Ok(())
}
