//! The real [`Backend`]: [`yc_engine::Engine`] wired with the `yc-ml` / `yc-llm` / `yc-hub` adapters.
//!
//! Meetings, recorder, search, ask, tasks, import and settings map one to one onto the engine; the
//! models screen is the `yc-hub` catalog with installed state and predictor verdicts; downloads stream
//! progress as [`EngineEvent::Download`]. The pieces live in submodules: [`settings`] (UI settings <->
//! engine settings), [`models`] (catalog, downloads, benchmarks), [`playback`] (audio for the player).

mod models;
mod playback;
mod settings;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use yc_core::events::EngineEvent;
use yc_core::meeting::{MeetingId, MeetingStatus};
use yc_core::summary::Summary;
use yc_core::{Error, Result};
use yc_engine::ports::{AnswerEvent, Ports};
use yc_engine::{Engine, EngineConfig, ImportOptions, MicChoice, StartOptions, YadaRecorder};
use yc_hub::catalog::ModelKind as CatKind;
use yc_store::SearchFilters;

use crate::adapters::hub::{Insights, ModelHub};
use crate::adapters::llm::Llm;
use crate::adapters::runtime_keeper::RuntimeKeeper;
use crate::adapters::{Prefs, SharedPrefs, ml, parse_summary_mode, read_prefs};
use crate::api::*;
use crate::bookmark_screenshot::{BookmarkScreenshot, XcapCapture, capture_with};
use crate::{paths, permissions};
use models::ActiveDownload;
use settings::{AppPrefs, SystemAudioKnown};

/// Everything [`EngineBackend::open`] needs.
pub struct BackendConfig {
    /// App data directory (index, models, runtimes, calibration, in-progress recordings).
    pub data_dir: PathBuf,
    /// Library folder to open instead of the default (tests); the setting still wins once saved.
    pub library_dir: Option<PathBuf>,
    /// Receives every engine event (forwarded to the webviews and the tray).
    pub events: EventSink,
    /// Hold a no-idle-sleep assertion while recording.
    pub prevent_sleep: bool,
    /// Skip the free-disk-space check when recording starts (tests).
    pub skip_disk_check: bool,
    /// Never read or write the OS keychain (tests): every key counts as missing.
    pub skip_keychain: bool,
}

impl BackendConfig {
    /// Production defaults for `data_dir`.
    pub fn new(data_dir: impl Into<PathBuf>, events: EventSink) -> Self {
        Self {
            data_dir: data_dir.into(),
            library_dir: None,
            events,
            prevent_sleep: true,
            skip_disk_check: false,
            skip_keychain: false,
        }
    }
}

struct RecState {
    title: String,
    mic_device: Option<String>,
    bookmarks: Vec<u64>,
}

#[derive(Default)]
pub(crate) struct State {
    rec: Option<RecState>,
    imports: Vec<ImportJob>,
    import_of_meeting: HashMap<MeetingId, String>,
    downloads: HashMap<String, ActiveDownload>,
    progress: HashMap<String, ModelDownload>,
    app: AppPrefs,
    screenshot_hint_sent: bool,
}

/// The engine behind the Tauri commands.
pub struct EngineBackend {
    engine: Arc<Engine>,
    hub: Arc<ModelHub>,
    insights: Arc<Insights>,
    llm: Arc<Llm>,
    keeper: Arc<RuntimeKeeper>,
    prefs: SharedPrefs,
    data_dir: PathBuf,
    events: EventSink,
    state: Mutex<State>,
    bench_lock: tokio::sync::Mutex<()>,
    me: Weak<EngineBackend>,
}

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

fn mid(id: &str) -> Result<MeetingId> {
    id.parse()
        .map_err(|_| invalid(format!("bad meeting id `{id}`")))
}

fn parse_time(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").map(|n| n.and_utc()))
        .unwrap_or_else(|_| Utc::now())
}

impl EngineBackend {
    #[cfg(target_os = "macos")]
    fn hint_screenshot_permission(&self) {
        if permissions::screen_recording_state() != PermissionState::Granted
            && let Ok(mut state) = self.state.lock()
            && !state.screenshot_hint_sent
        {
            state.screenshot_hint_sent = true;
            (self.events)(EngineEvent::ScreenshotPermissionRequired);
        }
    }

    /// Opens the engine on `cfg.data_dir` with the real adapters and starts forwarding its events.
    pub async fn open(cfg: BackendConfig) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&cfg.data_dir)?;
        let hub = Arc::new(ModelHub::new(&cfg.data_dir)?);
        let insights = Insights::new(&cfg.data_dir);
        let russian_system = paths::system_is_russian();
        let prefs: SharedPrefs = Arc::new(std::sync::RwLock::new(Prefs {
            russian: russian_system,
            ..Prefs::default()
        }));
        let keeper = RuntimeKeeper::new(
            Arc::clone(&hub),
            Arc::clone(&insights),
            Arc::clone(&cfg.events),
        );
        let llm = Llm::new(
            Arc::clone(&hub),
            Arc::clone(&insights),
            Arc::clone(&keeper),
            Arc::clone(&prefs),
            !cfg.skip_keychain,
        );
        let ports = Ports {
            asr: Arc::new(ml::Asr::new(Arc::clone(&hub), Arc::clone(&prefs))),
            vad: Arc::new(ml::Vad::new(&hub)),
            speaker: Arc::new(ml::Speaker::new(Arc::clone(&hub))),
            text_embedder: Arc::new(ml::TextEmbed::new(Arc::clone(&hub), Arc::clone(&prefs))),
            summarizer: Some(llm.clone() as Arc<dyn yc_engine::ports::Summarizer>),
            answerer: Some(llm.clone() as Arc<dyn yc_engine::ports::Answerer>),
            recorder: Arc::new(YadaRecorder),
        };
        let library = cfg
            .library_dir
            .clone()
            .unwrap_or_else(|| paths::default_library_dir(russian_system));
        let mut ec = EngineConfig::new(&cfg.data_dir, library, ports);
        ec.prevent_sleep = cfg.prevent_sleep;
        ec.skip_disk_check = cfg.skip_disk_check;
        let engine = Engine::open(ec).await?;

        let backend = Arc::new_cyclic(|me| Self {
            me: me.clone(),
            engine: Arc::clone(&engine),
            hub,
            insights,
            llm,
            keeper: Arc::clone(&keeper),
            prefs,
            data_dir: cfg.data_dir.clone(),
            events: cfg.events,
            state: Mutex::new(State {
                app: AppPrefs::load(&settings::prefs_path(&cfg.data_dir)),
                ..State::default()
            }),
            bench_lock: tokio::sync::Mutex::new(()),
        });
        if engine.settings().summary_provider.is_null() {
            let library = engine.library_root();
            engine
                .update_settings(|s| settings::first_run(s, &library, russian_system))
                .await?;
        }
        backend.sync_prefs();
        backend.spawn_event_pump();
        // A broken or removed llama.cpp runtime is repaired in the background.
        tokio::spawn(keeper.validate_in_background());
        // Existing installs that predate the automatic search-model download.
        let me = Arc::clone(&backend);
        tokio::spawn(async move { me.queue_default_embedder().await });
        Ok(backend)
    }

    /// Stops background work and the local LLM server (app exit). Recordings stay recoverable on disk.
    pub async fn shutdown(&self) {
        self.engine.shutdown();
        self.llm.stop_server().await;
    }

    /// The engine (for the shell and tests).
    pub fn engine(&self) -> Arc<Engine> {
        Arc::clone(&self.engine)
    }

    fn arc(&self) -> Result<Arc<Self>> {
        self.me.upgrade().ok_or(Error::Cancelled)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Copies the model/provider choices from the engine settings to the adapters.
    fn sync_prefs(&self) {
        let s = self.engine.settings();
        if let Ok(mut p) = self.prefs.write() {
            p.asr = s.models.asr.clone();
            p.speaker = s.models.speaker.clone();
            p.text_embedding = s.models.text_embedding.clone();
            p.llm = s.models.llm.clone();
            p.summary = parse_summary_mode(&s.summary_provider).unwrap_or_default();
            p.russian = s.russian();
        }
    }

    fn save_app_prefs(&self, app: &AppPrefs) {
        app.save(&settings::prefs_path(&self.data_dir));
    }

    fn spawn_event_pump(self: &Arc<Self>) {
        let weak: Weak<Self> = Arc::downgrade(self);
        let mut rx = self.engine.events();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        let Some(me) = weak.upgrade() else { break };
                        me.on_event(&ev);
                        let ev = match ev {
                            EngineEvent::Status { meeting, status } => {
                                if let MeetingStatus::Failed { message, .. } = &status {
                                    tracing::warn!("meeting {meeting} failed: {message}");
                                }
                                EngineEvent::Status {
                                    meeting,
                                    status: crate::commands::friendly_status(status),
                                }
                            }
                            other => other,
                        };
                        (me.events)(ev);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!("event pump lagged by {n}");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    /// Bookkeeping the backend does on engine events before the UI sees them.
    fn on_event(&self, ev: &EngineEvent) {
        match ev {
            EngineEvent::Status { meeting, status } => {
                let mut g = self.lock();
                let Some(job_id) = g.import_of_meeting.get(meeting).cloned() else {
                    return;
                };
                if let Some(job) = g.imports.iter_mut().find(|j| j.id == job_id) {
                    match status {
                        MeetingStatus::Processing { progress, .. } => {
                            job.state = ImportState::Running;
                            job.progress = *progress;
                        }
                        MeetingStatus::Ready => {
                            job.state = ImportState::Done;
                            job.progress = 1.0;
                        }
                        MeetingStatus::Failed { message, .. } => {
                            job.state = ImportState::Failed;
                            job.error = Some(message.clone());
                        }
                        MeetingStatus::Recording => {}
                    }
                }
            }
            EngineEvent::Recorder { state, .. } => {
                use yc_core::events::RecorderState::*;
                if matches!(state, Stopped | Cancelled) {
                    self.lock().rec = None;
                }
            }
            EngineEvent::Levels { system, .. } if system.peak > 0.0 => {
                // Real system audio arrived: the permission is evidently granted.
                let mut g = self.lock();
                if g.app.system_audio != SystemAudioKnown::Granted {
                    g.app.system_audio = SystemAudioKnown::Granted;
                    let app = g.app.clone();
                    drop(g);
                    self.save_app_prefs(&app);
                }
            }
            _ => {}
        }
    }

    fn snapshot(&self) -> RecorderSnapshot {
        let Some(info) = self.engine.recording() else {
            return RecorderSnapshot::default();
        };
        let g = self.lock();
        let russian = self.engine.settings().russian();
        RecorderSnapshot {
            phase: if info.paused {
                RecorderPhase::Paused
            } else {
                RecorderPhase::Recording
            },
            meeting_id: Some(info.meeting.to_string()),
            title: Some(
                g.rec
                    .as_ref()
                    .map(|r| r.title.clone())
                    .unwrap_or_else(|| if russian { "Встреча" } else { "Meeting" }.to_owned()),
            ),
            elapsed_ms: info.elapsed_ms,
            bookmarks: g
                .rec
                .as_ref()
                .map(|r| r.bookmarks.clone())
                .unwrap_or_default(),
            mic_device: g.rec.as_ref().and_then(|r| r.mic_device.clone()),
            system_source: Some(if russian { "Система" } else { "System" }.to_owned()),
        }
    }

    fn system_audio_state(&self) -> PermissionState {
        if !cfg!(target_os = "macos") {
            return PermissionState::Granted;
        }
        match self.lock().app.system_audio {
            SystemAudioKnown::Granted => PermissionState::Granted,
            SystemAudioKnown::Denied => PermissionState::Denied,
            SystemAudioKnown::Unknown => PermissionState::NotDetermined,
        }
    }

    fn audio_note(&self, started: DateTime<Utc>, has_audio: bool) -> Option<String> {
        let s = self.engine.settings();
        let ru = s.russian();
        let days = s.keep_audio.days()?;
        if days == 0 {
            return Some(
                if ru {
                    "аудио не хранится, остаются текст и итоги"
                } else {
                    "audio is not kept, text and summary stay"
                }
                .to_owned(),
            );
        }
        if !has_audio {
            return Some(
                if ru {
                    "аудио удалено, текст и итоги сохранены"
                } else {
                    "audio deleted, text and summary kept"
                }
                .to_owned(),
            );
        }
        let left = (started + chrono::Duration::days(i64::from(days)) - Utc::now()).num_days();
        let left = left.max(1);
        Some(if ru {
            format!("аудио удалится через {left} дн., текст и итоги останутся")
        } else {
            format!("audio is deleted in {left} days, text and summary stay")
        })
    }
}

#[async_trait]
impl Backend for EngineBackend {
    // ---- meetings -------------------------------------------------------------------------------

    async fn list_meetings(
        &self,
        folder_id: Option<&str>,
        favorites_only: bool,
    ) -> Result<Vec<MeetingListItem>> {
        let rows = self
            .engine
            .list_meetings_filtered(10_000, 0, folder_id.map(str::to_owned), favorites_only)
            .await?;
        let root = self.engine.library_root();
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let started_at = parse_time(&row.started_at);
            let ready = row.status == MeetingStatus::Ready;
            let folder = root.join(&row.folder);
            let (snippet, has_audio) = if ready {
                let f = folder.clone();
                tokio::task::spawn_blocking(move || {
                    let snippet = std::fs::read(f.join("summary.json"))
                        .ok()
                        .and_then(|b| serde_json::from_slice::<Summary>(&b).ok())
                        .and_then(|s| s.about.into_iter().next());
                    (snippet, f.join("audio").is_dir())
                })
                .await
                .unwrap_or((None, false))
            } else {
                (None, false)
            };
            out.push(MeetingListItem {
                id: row.id.to_string(),
                title: row.title,
                started_at,
                duration_ms: row.duration_ms,
                audio_note: if row.favorite {
                    Some(
                        if self.engine.settings().russian() {
                            "звук хранится всегда"
                        } else {
                            "audio kept forever"
                        }
                        .to_owned(),
                    )
                } else if ready {
                    self.audio_note(started_at, has_audio)
                } else {
                    None
                },
                snippet,
                status: crate::commands::friendly_status(row.status),
                favorite: row.favorite,
                folder_id: row.folder_id,
            });
        }
        Ok(out)
    }

    async fn list_folders(&self) -> Result<Vec<yc_core::meeting::Folder>> {
        self.engine.list_folders().await
    }
    async fn create_folder(
        &self,
        name: &str,
        color: Option<String>,
    ) -> Result<yc_core::meeting::Folder> {
        self.engine.create_folder(name.to_owned(), color).await
    }
    async fn update_folder(
        &self,
        id: &str,
        name: Option<String>,
        color: Option<Option<String>>,
        order: Option<u32>,
    ) -> Result<yc_core::meeting::Folder> {
        self.engine
            .update_folder(id.to_owned(), name, color, order)
            .await
    }
    async fn delete_folder(&self, id: &str) -> Result<()> {
        self.engine.delete_folder(id.to_owned()).await
    }
    async fn set_meeting_folder(&self, id: &str, folder_id: Option<String>) -> Result<()> {
        self.engine.set_meeting_folder(mid(id)?, folder_id).await
    }
    async fn set_meeting_favorite(&self, id: &str, favorite: bool) -> Result<()> {
        self.engine.set_meeting_favorite(mid(id)?, favorite).await
    }
    async fn add_bookmark(
        &self,
        id: &str,
        at_ms: u64,
        label: Option<String>,
    ) -> Result<yc_core::meeting::Bookmark> {
        self.engine.add_bookmark(mid(id)?, at_ms, label).await
    }
    async fn update_bookmark(
        &self,
        id: &str,
        bookmark_id: &str,
        label: Option<String>,
    ) -> Result<yc_core::meeting::Bookmark> {
        self.engine
            .update_bookmark(mid(id)?, bookmark_id.to_owned(), label)
            .await
    }
    async fn remove_bookmark(&self, id: &str, bookmark_id: &str) -> Result<()> {
        self.engine
            .remove_bookmark(mid(id)?, bookmark_id.to_owned())
            .await
    }

    async fn get_meeting(&self, id: &str) -> Result<MeetingDetail> {
        let detail = self.engine.get_meeting(mid(id)?).await?;
        let audio_url = self.audio_url(&detail).await;
        let dir_path = self
            .engine
            .library_root()
            .join(&detail.folder)
            .to_string_lossy()
            .into_owned();
        let mut meeting = detail.meeting;
        meeting.status = crate::commands::friendly_status(meeting.status);
        Ok(MeetingDetail {
            has_audio: audio_url.is_some(),
            audio_url,
            meeting,
            turns: detail.turns,
            summary: detail.summary,
            dir_path,
        })
    }

    async fn rename_meeting(&self, id: &str, title: &str) -> Result<()> {
        self.engine.rename_meeting(mid(id)?, title.to_owned()).await
    }

    async fn delete_meeting(&self, id: &str) -> Result<()> {
        let mid = mid(id)?;
        self.engine.delete_meeting(mid).await?;
        let _ = std::fs::remove_file(self.playback_dir().join(format!("{mid}.flac")));
        Ok(())
    }

    async fn rename_speaker(&self, id: &str, key: &str, name: &str) -> Result<RenameOutcome> {
        self.engine
            .rename_speaker(mid(id)?, key.to_owned(), name.to_owned())
            .await
    }

    async fn set_task_done(&self, id: &str, index: usize, done: bool) -> Result<()> {
        self.engine
            .set_task_done(mid(id)?, index as u32, done)
            .await
    }

    async fn regenerate_summary(&self, id: &str) -> Result<Summary> {
        let mid = mid(id)?;
        self.engine.regenerate_summary(mid).await?;
        let status = self
            .engine
            .wait_ready(mid, std::time::Duration::from_secs(45 * 60))
            .await?;
        if let MeetingStatus::Failed { message, .. } = status {
            return Err(Error::Llm(message));
        }
        self.engine
            .get_meeting(mid)
            .await?
            .summary
            .ok_or_else(|| Error::Llm("the summary was not produced".to_owned()))
    }

    async fn meeting_retranscribe(&self, id: &str) -> Result<()> {
        self.engine.retranscribe(mid(id)?).await
    }

    async fn export_meeting(&self, id: &str, format: crate::api::ExportFormat) -> Result<String> {
        let mid = mid(id)?;
        let (fmt, file) = match format {
            crate::api::ExportFormat::Md => (yc_engine::ExportFormat::Markdown, "export.md"),
            crate::api::ExportFormat::Json => (yc_engine::ExportFormat::Json, "export.json"),
        };
        let content = self.engine.export(mid, fmt).await?;
        let folder = self
            .engine
            .library_root()
            .join(self.engine.get_meeting(mid).await?.folder);
        let path = folder.join(file);
        tokio::fs::write(&path, content).await?;
        Ok(path.to_string_lossy().into_owned())
    }

    async fn meeting_dir(&self, id: &str) -> Result<Option<PathBuf>> {
        let detail = self.engine.get_meeting(mid(id)?).await?;
        Ok(Some(self.engine.library_root().join(detail.folder)))
    }

    async fn copy_for_claude(&self, id: &str) -> Result<String> {
        self.engine.copy_for_claude(mid(id)?).await
    }

    // ---- recorder -------------------------------------------------------------------------------

    async fn recorder_state(&self) -> Result<RecorderSnapshot> {
        Ok(self.snapshot())
    }

    async fn recorder_start(&self, opts: RecorderStartOptions) -> Result<RecorderSnapshot> {
        if permissions::microphone_state() == PermissionState::Denied {
            return Err(Error::Permission(PermissionKind::Microphone));
        }
        let title = opts.title.clone().filter(|t| !t.trim().is_empty());
        let start = StartOptions {
            title: title.clone(),
            expected_speakers: opts.expected_speakers,
            expected_speakers_explicit: opts.expected_speakers.is_some()
                && opts.participant_hints.is_empty(),
            mic: opts
                .mic_device
                .clone()
                .map_or(MicChoice::Configured, MicChoice::Device),
            system_audio: true,
            language: None,
            participant_hints: opts.participant_hints.clone(),
        };
        self.engine.start_recording(start).await?;
        let russian = self.engine.settings().russian();
        self.lock().rec = Some(RecState {
            title: title.unwrap_or_else(|| if russian { "Встреча" } else { "Meeting" }.to_owned()),
            mic_device: opts.mic_device,
            bookmarks: Vec::new(),
        });
        Ok(self.snapshot())
    }

    async fn recorder_pause(&self) -> Result<()> {
        self.engine.pause_recording()
    }

    async fn recorder_resume(&self) -> Result<()> {
        self.engine.resume_recording()
    }

    async fn recorder_stop(&self) -> Result<String> {
        let id = self.engine.stop_recording().await?;
        self.lock().rec = None;
        Ok(id.to_string())
    }

    async fn recorder_cancel(&self) -> Result<()> {
        self.engine.cancel_recording().await?;
        self.lock().rec = None;
        Ok(())
    }

    async fn recorder_bookmark(&self, label: Option<String>) -> Result<yc_core::meeting::Bookmark> {
        let b = self.engine.bookmark(label)?;
        let mode = self.lock().app.bookmark_screenshot;
        let meeting_id = self.engine.active_meeting_id()?;
        if let Some(r) = self.lock().rec.as_mut() {
            r.bookmarks.push(b.at_ms);
        }
        if mode != BookmarkScreenshot::Off {
            let engine = Arc::clone(&self.engine);
            #[cfg(target_os = "macos")]
            let backend = self.me.upgrade();
            let id = b.id.clone();
            tokio::spawn(async move {
                let captured = tokio::task::spawn_blocking(move || {
                    capture_with(&XcapCapture, mode, std::process::id())
                })
                .await;
                match captured {
                    Ok(Ok(Some(bytes))) => {
                        if let Err(error) = engine
                            .attach_bookmark_screenshot(meeting_id, id, bytes)
                            .await
                        {
                            tracing::warn!(%error, "could not attach bookmark screenshot");
                        }
                    }
                    Ok(Ok(None)) => {
                        tracing::debug!(
                            "bookmark screenshot capture found no eligible display window"
                        );
                        #[cfg(target_os = "macos")]
                        if let Some(backend) = backend.as_ref() {
                            backend.hint_screenshot_permission();
                        }
                    }
                    Ok(Err(error)) => {
                        tracing::debug!(%error, "bookmark screenshot capture unavailable");
                        #[cfg(target_os = "macos")]
                        {
                            if let Some(backend) = backend {
                                backend.hint_screenshot_permission();
                            }
                        }
                    }
                    Err(error) => tracing::warn!(%error, "bookmark screenshot worker failed"),
                }
            });
        }
        Ok(b)
    }

    async fn list_input_devices(&self) -> Result<Vec<InputDevice>> {
        let devices = tokio::task::spawn_blocking(yc_audio::list_input_devices)
            .await
            .map_err(|e| Error::Device(e.to_string()))??;
        Ok(devices
            .into_iter()
            .map(|d| InputDevice {
                id: d.id,
                name: d.name,
                is_default: d.is_default,
            })
            .collect())
    }

    // ---- search / ask / tasks / import ----------------------------------------------------------

    async fn search(&self, query: &str, folder_id: Option<&str>) -> Result<Vec<SearchHit>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let hits = self
            .engine
            .search(
                q.to_owned(),
                SearchFilters {
                    folder_id: folder_id.map(str::to_owned),
                    ..SearchFilters::default()
                },
                30,
            )
            .await?;
        Ok(hits
            .into_iter()
            .map(|h| SearchHit {
                meeting_id: h.meeting_id.to_string(),
                title: h.meeting_title,
                started_at: parse_time(&h.started_at),
                at_ms: h.start_ms,
                speaker: h.speakers.first().cloned().unwrap_or_default(),
                text: h.snippet.replace("**", ""),
                score: h.score as f32,
            })
            .collect())
    }

    async fn ask(&self, question: &str, folder_id: Option<&str>, sink: AskSink) -> Result<()> {
        let (excerpts, mut rx) = match self
            .engine
            .ask_with_excerpts(question.to_owned(), folder_id.map(str::to_owned))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("ask failed: {e}");
                sink(AskEvent::Error {
                    message: crate::commands::friendly(&e).to_owned(),
                });
                return Ok(());
            }
        };
        // With summaries off the answerer returns the ranked passages only.
        let generated_answer = read_prefs(&self.prefs).summary != SummaryMode::Off;
        while let Some(ev) = rx.recv().await {
            match ev {
                AnswerEvent::Egress(_) => {}
                AnswerEvent::Token(text) => sink(AskEvent::Token { text }),
                AnswerEvent::Citations(indices) => {
                    let items = indices
                        .into_iter()
                        .filter_map(|i| excerpts.get(i).map(|e| (i, e)))
                        .map(|(i, e)| Citation {
                            n: i as u32 + 1,
                            meeting_id: e.meeting_id.to_string(),
                            meeting_title: e.meeting_title.clone(),
                            started_at: parse_time(&e.started_at),
                            at_ms: e.start_ms,
                            speaker: e.speakers.first().cloned().unwrap_or_default(),
                            quote: e.text.chars().take(280).collect(),
                        })
                        .collect();
                    sink(AskEvent::Citations { items });
                }
                AnswerEvent::Done => {
                    let provider = self.llm.egress_label().await;
                    sink(AskEvent::Done {
                        sent: SentNote {
                            excerpts: if provider == "local" {
                                0
                            } else {
                                excerpts.len() as u32
                            },
                            provider,
                            audio: false,
                        },
                        generated: generated_answer,
                    });
                    break;
                }
                AnswerEvent::Failed(message) => {
                    tracing::warn!("answer failed: {message}");
                    sink(AskEvent::Error {
                        message: crate::commands::friendly_text(&message).to_owned(),
                    });
                    break;
                }
            }
        }
        Ok(())
    }

    async fn list_tasks(&self, include_done: bool) -> Result<Vec<TaskItem>> {
        let tasks = self.engine.list_tasks(!include_done).await?;
        // Tasks do not carry their meeting's start time; it comes from the meeting list.
        let started: HashMap<MeetingId, DateTime<Utc>> = self
            .engine
            .list_meetings(10_000, 0)
            .await?
            .into_iter()
            .map(|m| (m.id, parse_time(&m.started_at)))
            .collect();
        Ok(tasks
            .into_iter()
            .map(|t| TaskItem {
                started_at: started.get(&t.meeting_id).copied().unwrap_or_else(Utc::now),
                meeting_id: t.meeting_id.to_string(),
                meeting_title: t.meeting_title,
                index: t.idx as usize,
                text: t.text,
                owner: t.owner,
                due: t.due,
                at_ms: t.at_ms,
                done: t.done,
                meeting_folder_id: t.meeting_folder_id,
                meeting_favorite: t.meeting_favorite,
            })
            .collect())
    }

    async fn import_audio(&self, paths: Vec<String>) -> Result<Vec<ImportJob>> {
        let mut jobs = Vec::new();
        for path in paths {
            let file_name = std::path::Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            let job_id = uuid::Uuid::now_v7().to_string();
            let mut job = ImportJob {
                id: job_id.clone(),
                file_name,
                state: ImportState::Running,
                progress: 0.0,
                meeting_id: None,
                error: None,
            };
            match self
                .engine
                .import_file(&path, ImportOptions::default())
                .await
            {
                Ok(meeting) => {
                    job.meeting_id = Some(meeting.to_string());
                    let mut g = self.lock();
                    g.import_of_meeting.insert(meeting, job_id);
                    g.imports.push(job.clone());
                }
                Err(e) => {
                    job.state = ImportState::Failed;
                    job.error = Some(e.to_string());
                    self.lock().imports.push(job.clone());
                }
            }
            jobs.push(job);
        }
        Ok(jobs)
    }

    async fn list_imports(&self) -> Result<Vec<ImportJob>> {
        Ok(self.lock().imports.clone())
    }

    // ---- models ---------------------------------------------------------------------------------

    async fn models_overview(&self) -> Result<ModelsOverview> {
        self.overview().await
    }

    async fn model_download(&self, id: &str, progress: EventSink) -> Result<()> {
        // `self` is only reachable as `&Self` here; the download task needs an `Arc`.
        self.arc()?.start_download(id, progress).await
    }

    async fn model_pause(&self, id: &str) -> Result<()> {
        self.pause_download(id);
        Ok(())
    }

    async fn model_cancel(&self, id: &str) -> Result<()> {
        self.cancel_download(id);
        Ok(())
    }

    async fn model_remove(&self, id: &str) -> Result<()> {
        self.remove_model(id).await
    }

    async fn model_set_active(&self, id: &str) -> Result<()> {
        self.set_active(id).await
    }

    async fn model_benchmark(&self, id: &str) -> Result<BenchmarkResult> {
        self.benchmark_by_id(id).await
    }

    // ---- settings / permissions -----------------------------------------------------------------

    async fn get_settings(&self) -> Result<Settings> {
        let s = self.engine.settings();
        let app = self.lock().app.clone();
        let mode = read_prefs(&self.prefs).summary;
        Ok(settings::to_api(
            &s,
            &self.engine.library_root(),
            &app,
            &mode,
            self.llm.keys_set(),
        ))
    }

    async fn update_settings(&self, patch: SettingsPatch) -> Result<Settings> {
        let mut app = self.lock().app.clone();
        let hub = Arc::clone(&self.hub);
        let before = read_prefs(&self.prefs).summary;
        let summary_patch = patch.summaries.clone();
        if let Some(key) = summary_patch.as_ref().and_then(|sp| sp.key.as_ref()) {
            self.llm.apply_key(key.account, &key.value)?;
        }
        self.engine
            .update_settings(|s| {
                settings::apply_patch(s, &mut app, &patch);
                if let Some(mode) = summary_patch.as_ref().and_then(|sp| sp.mode.as_ref()) {
                    let mode = settings::clean_mode(mode);
                    if let SummaryMode::Local { model } = &mode {
                        let ids: Vec<&str> = hub
                            .offered()
                            .filter(|m| m.kind == CatKind::Llm)
                            .map(|m| m.id.as_str())
                            .collect();
                        s.models.llm = settings::local_model_choice(&ids, model);
                    }
                    s.summary_provider =
                        serde_json::to_value(&mode).unwrap_or(serde_json::Value::Null);
                }
            })
            .await?;
        self.lock().app = app.clone();
        self.save_app_prefs(&app);
        self.sync_prefs();
        if matches!(before, SummaryMode::Local { .. })
            && !matches!(read_prefs(&self.prefs).summary, SummaryMode::Local { .. })
        {
            self.llm.stop_server().await;
        }
        self.get_settings().await
    }

    async fn ollama_status(&self, base: OllamaBase) -> Result<OllamaStatus> {
        Ok(self.llm.ollama_status(base).await)
    }

    async fn test_summary_provider(&self, mode: Option<SummaryMode>) -> Result<ProviderTest> {
        let mode = mode
            .map(|m| settings::clean_mode(&m))
            .unwrap_or_else(|| read_prefs(&self.prefs).summary);
        Ok(self.llm.test_provider(&mode).await)
    }

    async fn permissions(&self) -> Result<Permissions> {
        Ok(Permissions {
            microphone: permissions::microphone_state(),
            system_audio: self.system_audio_state(),
            screen_recording: permissions::screen_recording_state(),
        })
    }

    async fn request_permission(&self, kind: PermissionKind) -> Result<Permissions> {
        match kind {
            PermissionKind::Microphone => {
                permissions::request_microphone().await;
            }
            PermissionKind::SystemAudio => {
                let probe = tokio::task::spawn_blocking(permissions::probe_system_audio)
                    .await
                    .map_err(|e| Error::Device(e.to_string()))?;
                let known = match probe {
                    Ok(permissions::SystemAudioProbe::Heard) => Some(SystemAudioKnown::Granted),
                    Ok(permissions::SystemAudioProbe::Denied) | Err(Error::Permission(_)) => {
                        Some(SystemAudioKnown::Denied)
                    }
                    Ok(permissions::SystemAudioProbe::Inconclusive) => None,
                    Err(e) => return Err(e),
                };
                if let Some(known) = known {
                    let mut g = self.lock();
                    g.app.system_audio = known;
                    let app = g.app.clone();
                    drop(g);
                    self.save_app_prefs(&app);
                }
            }
            PermissionKind::ScreenRecording => {
                let _ = permissions::request_screen_recording();
            }
        }
        self.permissions().await
    }

    async fn open_system_settings(&self, _kind: PermissionKind) -> Result<()> {
        // The command handler opens the OS pane (`permissions::open_system_settings`).
        Ok(())
    }
}
