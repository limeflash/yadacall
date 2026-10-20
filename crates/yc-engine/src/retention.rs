//! Audio retention: periodically delete the raw audio of meetings older than the policy.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use yc_core::Result;
use yc_core::meeting::{MeetingStatus, Stage};

use crate::engine::Engine;
use crate::finalize::audio_dir;

const SWEEP_EVERY: Duration = Duration::from_secs(6 * 3600);
const FIRST_SWEEP_AFTER: Duration = Duration::from_secs(60);

pub(crate) fn spawn_sweeper(engine: &Arc<Engine>) {
    let weak = Arc::downgrade(engine);
    let token = engine.shutdown.clone();
    tokio::spawn(async move {
        let mut wait = FIRST_SWEEP_AFTER;
        loop {
            tokio::select! {
                _ = token.cancelled() => return,
                _ = tokio::time::sleep(wait) => {}
            }
            wait = SWEEP_EVERY;
            let Some(engine) = weak.upgrade() else { return };
            match engine.sweep_retention().await {
                Ok(0) => {}
                Ok(n) => tracing::info!("retention: removed the audio of {n} meetings"),
                Err(e) => tracing::warn!("retention sweep failed: {e}"),
            }
        }
    });
}

impl Engine {
    /// Deletes the audio of meetings past the retention period. The transcript must be on disk, the
    /// meeting must not be mid-processing, and a meeting whose transcription failed keeps its audio
    /// (it is the only way to get the text). Returns the number of meetings cleaned.
    pub async fn sweep_retention(self: &Arc<Self>) -> Result<usize> {
        self.sweep_retention_at(Utc::now()).await
    }

    /// [`Engine::sweep_retention`] as if it were `now` (lets tests and the CLI simulate time).
    pub async fn sweep_retention_at(self: &Arc<Self>, now: DateTime<Utc>) -> Result<usize> {
        self.blocking(move |e| e.sweep_blocking(now)).await
    }

    pub(crate) fn sweep_blocking(&self, now: DateTime<Utc>) -> Result<usize> {
        let Some(days) = self.settings().keep_audio.days() else {
            return Ok(0);
        };
        let mut removed = 0;
        let mut offset = 0;
        loop {
            let page = self.store.list_meetings(200, offset)?;
            if page.is_empty() {
                break;
            }
            offset += page.len();
            for item in page {
                if item.favorite {
                    continue;
                }
                if matches!(
                    item.status,
                    MeetingStatus::Recording
                        | MeetingStatus::Processing { .. }
                        | MeetingStatus::Failed {
                            stage: Stage::Transcribe,
                            ..
                        }
                ) {
                    continue;
                }
                let busy = self
                    .outstanding
                    .lock()
                    .map(|m| m.contains_key(&item.id))
                    .unwrap_or(false);
                if busy {
                    continue;
                }
                let Ok(started) = DateTime::parse_from_rfc3339(&item.started_at) else {
                    continue;
                };
                let ended = started.with_timezone(&Utc)
                    + chrono::Duration::milliseconds(item.duration_ms as i64);
                if now - ended < chrono::Duration::days(i64::from(days)) {
                    continue;
                }
                let folder = self.library_root().join(&item.folder);
                let audio = audio_dir(&self.library_root(), &item.folder);
                if audio.is_dir() && folder.join("transcript.json").is_file() {
                    std::fs::remove_dir_all(&audio)?;
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }
}
