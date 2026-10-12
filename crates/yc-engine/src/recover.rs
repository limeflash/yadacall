//! Recordings that did not finish (crash, power loss), and imports of existing audio files.
//! Both end in the normal finalisation path.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use yc_audio::writer::{TrackMeta, TrackWriter};
use yc_core::events::{EngineEvent, NoticeLevel};
use yc_core::meeting::{MeetingId, MeetingStatus, Stage, TrackKind};
use yc_core::{Error, Result, samples_to_ms};

use crate::engine::Engine;
use crate::finalize::FinalizeOpts;
use crate::livelog::{LiveLine, read_live};
use crate::meta::{LIVE_FILE, SessionMeta, Source};

/// A recording folder the engine found at startup without a finished meeting.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingRecovery {
    pub id: MeetingId,
    pub title: String,
    pub started_at: DateTime<Utc>,
    pub source: Source,
    /// Audio on disk, ms of the longest track.
    pub audio_ms: u64,
    /// Segments live transcription had already saved.
    pub live_segments: usize,
}

/// Options of [`Engine::import_file`].
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    pub title: Option<String>,
    pub language: Option<String>,
    pub expected_speakers: Option<u8>,
}

impl Engine {
    /// Unfinished recordings, newest first. Cleans up folders whose meeting was already saved.
    pub fn pending_recoveries(&self) -> Result<Vec<PendingRecovery>> {
        let active = self
            .active
            .lock()
            .ok()
            .and_then(|a| a.as_ref().map(|s| s.id));
        let inflight = self.inflight.lock().map(|s| s.clone()).unwrap_or_default();
        let mut out = Vec::new();
        let root = self.recording_root();
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Ok(out);
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Ok(meta) = SessionMeta::read(&dir) else {
                continue;
            };
            if Some(meta.id) == active || inflight.contains(&meta.id) {
                continue;
            }
            let pcms: Vec<u64> = TrackKind::ALL
                .iter()
                .filter_map(|k| std::fs::metadata(yc_audio::writer::pcm_path(&dir, *k)).ok())
                .map(|m| m.len() / 2)
                .collect();
            if pcms.is_empty() {
                if self.store.get_meeting(meta.id).is_ok() {
                    let _ = std::fs::remove_dir_all(&dir); // finished earlier; only litter left
                }
                continue;
            }
            let live_segments = read_live(&dir.join(LIVE_FILE))
                .map(|l| {
                    l.iter()
                        .filter(|x| matches!(x, LiveLine::Segment { .. }))
                        .count()
                })
                .unwrap_or(0);
            out.push(PendingRecovery {
                id: meta.id,
                title: meta.title,
                started_at: meta.started_at,
                source: meta.source,
                audio_ms: samples_to_ms(pcms.into_iter().max().unwrap_or(0)),
                live_segments,
            });
        }
        out.sort_by_key(|r| std::cmp::Reverse(r.started_at));
        Ok(out)
    }

    /// Rebuilds the meeting of an unfinished recording: transcribes what live processing missed
    /// (VAD + ASR over the raw tracks, in bounded memory) and runs the normal finalisation.
    /// Returns immediately; follow `EngineEvent::Status` or [`Engine::wait_ready`].
    pub async fn recover(self: &Arc<Self>, id: MeetingId) -> Result<MeetingId> {
        let dir = self.recording_root().join(id.to_string());
        let meta =
            SessionMeta::read(&dir).map_err(|_| Error::NotFound(format!("recording {id}")))?;
        if self
            .active
            .lock()
            .is_ok_and(|a| a.as_ref().is_some_and(|s| s.id == id))
        {
            return Err(Error::Invalid("that recording is still running".into()));
        }
        self.spawn_background_finalize(id, dir, meta.language, true)?;
        Ok(id)
    }

    /// Throws an unfinished recording away.
    pub async fn discard_recovery(self: &Arc<Self>, id: MeetingId) -> Result<()> {
        let dir = self.recording_root().join(id.to_string());
        if self.inflight.lock().is_ok_and(|s| s.contains(&id)) {
            return Err(Error::Invalid("that recording is being processed".into()));
        }
        self.blocking(move |_| {
            std::fs::remove_dir_all(&dir)?;
            Ok(())
        })
        .await
    }

    /// Imports an audio or video file as a meeting (one "them" track). Returns the new meeting's
    /// id immediately; decoding, transcription and the follow-up jobs run in the background.
    pub async fn import_file(
        self: &Arc<Self>,
        path: impl Into<PathBuf>,
        opts: ImportOptions,
    ) -> Result<MeetingId> {
        let path: PathBuf = path.into();
        let md = std::fs::metadata(&path)
            .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
        let settings = self.settings();
        let id = MeetingId::new();
        let dir = self.recording_root().join(id.to_string());
        let language = opts
            .language
            .clone()
            .unwrap_or_else(|| settings.meeting_language.code().to_owned());
        let modified: DateTime<Utc> = md.modified().map_or_else(|_| Utc::now(), Into::into);
        let (title, title_auto) = match opts.title.as_deref().map(str::trim) {
            Some(t) if !t.is_empty() => (t.to_owned(), false),
            _ => (
                path.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "Import".to_owned()),
                true,
            ),
        };
        let meta = SessionMeta {
            id,
            title,
            title_auto,
            started_at: modified,
            language: language.clone(),
            expected_speakers: opts.expected_speakers,
            source: Source::Import,
            bookmarks: Vec::new(),
        };
        let dir2 = dir.clone();
        self.blocking(move |_| {
            std::fs::create_dir_all(&dir2)?;
            meta.write(&dir2)
        })
        .await?;
        // Mark in-flight before the task starts so a concurrent listing never offers it.
        if let Ok(mut s) = self.inflight.lock() {
            s.insert(id);
        }
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let result = {
                let (dir, path) = (dir.clone(), path.clone());
                this.blocking(move |e| e.decode_import(&dir, &path)).await
            };
            this.inflight_remove(id);
            match result {
                Ok(()) => {
                    this.run_background_finalize(id, dir, language, true).await;
                }
                Err(e) => this.fail_import(id, &dir, &e),
            }
        });
        Ok(id)
    }

    fn decode_import(&self, dir: &Path, path: &Path) -> Result<()> {
        let started = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let name = path
            .file_name()
            .map_or_else(|| "import".to_owned(), |n| n.to_string_lossy().into_owned());
        let meta = TrackMeta::new(TrackKind::System, &name, started, 0);
        let mut writer = TrackWriter::create(dir, meta, std::time::Instant::now())?;
        let info = yc_audio::decode_to_16k_mono(path, &mut |block| writer.write(block))?;
        writer.finish(std::time::Instant::now())?;
        tracing::info!("decoded {} ({} ms)", path.display(), info.duration_ms);
        Ok(())
    }

    fn inflight_remove(&self, id: MeetingId) {
        if let Ok(mut s) = self.inflight.lock() {
            s.remove(&id);
        }
    }

    fn fail_import(&self, id: MeetingId, dir: &Path, e: &Error) {
        tracing::error!("import {id} failed: {e}");
        let _ = std::fs::remove_dir_all(dir);
        self.emit(EngineEvent::Status {
            meeting: id,
            status: MeetingStatus::Failed {
                stage: Stage::Transcribe,
                message: e.to_string(),
                retryable: false,
            },
        });
        self.notice(
            NoticeLevel::Error,
            &format!("Не удалось импортировать файл: {e}"),
            &format!("Could not import the file: {e}"),
        );
    }

    fn spawn_background_finalize(
        self: &Arc<Self>,
        id: MeetingId,
        dir: PathBuf,
        language: String,
        fill_tail: bool,
    ) -> Result<()> {
        {
            let mut set = self
                .inflight
                .lock()
                .map_err(|_| Error::Storage("lock poisoned".into()))?;
            if !set.insert(id) {
                return Err(Error::Invalid(
                    "that recording is already being processed".into(),
                ));
            }
        }
        let this = Arc::clone(self);
        tokio::spawn(async move {
            this.run_background_finalize(id, dir, language, fill_tail)
                .await;
            this.inflight_remove(id);
        });
        Ok(())
    }

    async fn run_background_finalize(
        self: &Arc<Self>,
        id: MeetingId,
        dir: PathBuf,
        language: String,
        fill_tail: bool,
    ) {
        self.emit(EngineEvent::Status {
            meeting: id,
            status: MeetingStatus::Processing {
                stage: Stage::Transcribe,
                progress: 0.0,
            },
        });
        if let Err(e) = self
            .finalize_dir(
                &dir,
                FinalizeOpts {
                    language,
                    fill_tail,
                },
            )
            .await
        {
            tracing::error!("finalising {id} failed: {e}");
            self.emit(EngineEvent::Status {
                meeting: id,
                status: MeetingStatus::Failed {
                    stage: Stage::Transcribe,
                    message: e.to_string(),
                    retryable: e.is_retryable() || matches!(e, Error::ModelMissing(_)),
                },
            });
            self.notice(
                NoticeLevel::Error,
                &format!("Не удалось обработать запись: {e}"),
                &format!("Could not process the recording: {e}"),
            );
        }
    }
}
