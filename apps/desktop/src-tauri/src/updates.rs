//! In-app update checks and installation.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Update, UpdaterExt};
use yc_core::{Error, Result};

use crate::api::{Backend, RecorderPhase};

const INITIAL_CHECK_DELAY: Duration = Duration::from_secs(30);
const CHECK_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);

/// Update availability returned to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// Whether a newer signed release is available.
    pub available: bool,
    /// The release version, when an update is available.
    pub version: Option<String>,
    /// Release notes, when provided by GitHub.
    pub notes: Option<String>,
    /// Release date as an RFC 3339 timestamp, when provided by GitHub.
    pub date: Option<String>,
}

/// Update events emitted on the `update` channel.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "event",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum UpdateEvent {
    /// A newer release was found by the background check.
    Available {
        version: String,
        notes: Option<String>,
        date: Option<String>,
    },
    /// The update package download has begun.
    Downloading {
        downloaded_bytes: u64,
        content_length: Option<u64>,
    },
    /// More bytes of the signed update package have arrived.
    DownloadProgress {
        downloaded_bytes: u64,
        content_length: Option<u64>,
    },
    /// The package has been downloaded and signature-verified.
    Downloaded,
    /// Installation was stopped because a recording became active during download.
    InstallDeferred { reason: String },
    /// Installation was started; the app will relaunch after the updater finishes.
    Installed,
    /// The tray's update entry was clicked: the main window shows the update sheet.
    OpenSheet,
}

/// Checks the configured updater endpoint and returns the available release metadata.
pub async fn check(app: &AppHandle) -> Result<UpdateStatus> {
    let update = get_update(app).await?;
    let info = status(update.as_ref());
    crate::shell::set_update_available(app, info.version.as_deref());
    Ok(info)
}

/// Downloads and installs the newest update, refusing to interrupt an active recording.
pub async fn install(app: &AppHandle, backend: &dyn Backend) -> Result<()> {
    ensure_no_recording(backend).await?;

    let update = get_update(app)
        .await?
        .ok_or_else(|| Error::NotFound("available update".to_owned()))?;
    let _ = app.emit(
        "update",
        UpdateEvent::Downloading {
            downloaded_bytes: 0,
            content_length: None,
        },
    );

    let progress_handle = app.clone();
    let mut downloaded_bytes = 0_u64;
    let bytes = update
        .download(
            move |chunk_length, content_length| {
                downloaded_bytes += chunk_length as u64;
                let _ = progress_handle.emit(
                    "update",
                    UpdateEvent::DownloadProgress {
                        downloaded_bytes,
                        content_length,
                    },
                );
            },
            || {},
        )
        .await
        .map_err(|error| Error::Network(error.to_string()))?;
    let _ = app.emit("update", UpdateEvent::Downloaded);

    if let Err(error) = ensure_no_recording(backend).await {
        let reason = error.to_string();
        let _ = app.emit(
            "update",
            UpdateEvent::InstallDeferred {
                reason: reason.clone(),
            },
        );
        return Err(error);
    }

    update
        .install(&bytes)
        .map_err(|error| Error::Invalid(format!("updater installation failed: {error}")))?;
    let _ = app.emit("update", UpdateEvent::Installed);
    app.restart();
}

/// Starts a quiet check 30 seconds after launch, then repeats every 12 hours.
pub fn start_background_check(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(INITIAL_CHECK_DELAY).await;
        loop {
            match get_update(&app).await {
                Ok(Some(update)) => {
                    let info = status(Some(&update));
                    crate::shell::set_update_available(&app, info.version.as_deref());
                    let _ = app.emit(
                        "update",
                        UpdateEvent::Available {
                            version: info.version.unwrap_or_default(),
                            notes: info.notes,
                            date: info.date,
                        },
                    );
                }
                Ok(None) => {}
                Err(error) => tracing::debug!(%error, "background update check failed"),
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

async fn get_update(app: &AppHandle) -> Result<Option<Update>> {
    let updater = app
        .updater()
        .map_err(|error| Error::Network(error.to_string()))?;
    match updater.check().await {
        Ok(update) => Ok(update),
        // No published release (or no manifest for this platform yet): nothing to update to.
        Err(tauri_plugin_updater::Error::ReleaseNotFound) => Ok(None),
        Err(error) => Err(Error::Network(error.to_string())),
    }
}

async fn ensure_no_recording(backend: &dyn Backend) -> Result<()> {
    let recorder = backend.recorder_state().await?;
    if recorder.phase != RecorderPhase::Idle {
        return Err(Error::Invalid(
            "Finish or cancel the active recording before installing an update".to_owned(),
        ));
    }
    Ok(())
}

fn status(update: Option<&Update>) -> UpdateStatus {
    match update {
        Some(update) => UpdateStatus {
            available: true,
            version: Some(update.version.clone()),
            notes: update.body.clone(),
            date: update.date.map(|date| {
                DateTime::<Utc>::from_timestamp(date.unix_timestamp(), date.nanosecond())
                    .map(|value| value.to_rfc3339())
                    .unwrap_or_else(|| date.to_string())
            }),
        },
        None => UpdateStatus {
            available: false,
            version: None,
            notes: None,
            date: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_events_serialize_with_camel_case_fields() {
        let json = serde_json::to_value(UpdateEvent::DownloadProgress {
            downloaded_bytes: 5,
            content_length: Some(10),
        })
        .unwrap();
        assert_eq!(json["event"], "downloadProgress");
        assert_eq!(json["downloadedBytes"], 5);
        assert_eq!(json["contentLength"], 10);
    }
}
