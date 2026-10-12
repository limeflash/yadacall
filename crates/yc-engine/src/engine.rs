//! The [`Engine`]: owns the store, the ports and all background work.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use tokio::sync::{Notify, broadcast};
use tokio_util::sync::CancellationToken;
use yc_core::events::{EngineEvent, NoticeLevel};
use yc_core::meeting::{MeetingId, MeetingStatus};
use yc_core::ports::TextEmbedder;
use yc_core::{Error, Result};
use yc_store::Store;

use crate::jobs::JobConfig;
use crate::ports::Ports;
use crate::session::Session;
use crate::settings::{SETTINGS_KEY, Settings};

/// How to open an [`Engine`].
#[derive(Clone)]
pub struct EngineConfig {
    /// Holds `index.sqlite`, `recording/` (in-progress sessions) and `tmp/`.
    pub app_data_dir: PathBuf,
    /// Where meeting folders live (overridden by `Settings::library_dir` when that is set).
    pub library_dir: PathBuf,
    /// The adapters.
    pub ports: Ports,
    /// Job runner tuning.
    pub jobs: JobConfig,
    /// Hold a no-idle-sleep assertion while recording.
    pub prevent_sleep: bool,
    /// Skip the free-disk-space check when recording starts (tests).
    pub skip_disk_check: bool,
}

impl EngineConfig {
    /// Defaults for everything but the paths and the adapters.
    pub fn new(
        app_data_dir: impl Into<PathBuf>,
        library_dir: impl Into<PathBuf>,
        ports: Ports,
    ) -> Self {
        Self {
            app_data_dir: app_data_dir.into(),
            library_dir: library_dir.into(),
            ports,
            jobs: JobConfig::default(),
            prevent_sleep: true,
            skip_disk_check: false,
        }
    }
}

/// The orchestrator. Create it with [`Engine::open`] and share the `Arc`.
pub struct Engine {
    pub(crate) store: Arc<Store>,
    pub(crate) ports: Ports,
    pub(crate) app_data: PathBuf,
    pub(crate) cfg: EngineConfig,
    events: broadcast::Sender<EngineEvent>,
    settings: RwLock<Settings>,
    pub(crate) active: Mutex<Option<Session>>,
    pub(crate) starting: AtomicBool,
    /// Meetings being imported/recovered right now (excluded from `pending_recoveries`).
    pub(crate) inflight: Mutex<HashSet<MeetingId>>,
    /// Serialises read-modify-write cycles on meeting files.
    pub(crate) write_lock: Mutex<()>,
    text_embedder: Mutex<Option<Option<Box<dyn TextEmbedder>>>>,
    pub(crate) jobs_notify: Arc<Notify>,
    pub(crate) outstanding: Mutex<HashMap<MeetingId, u32>>,
    pub(crate) pending_reindex: Mutex<HashSet<MeetingId>>,
    pub(crate) shutdown: CancellationToken,
    pub(crate) cancel_offline: Arc<AtomicBool>,
}

impl Engine {
    /// Opens (creating if needed) the store, resets jobs a previous process left running and starts
    /// the job runner and the retention sweeper.
    pub async fn open(config: EngineConfig) -> Result<Arc<Engine>> {
        let cfg = config.clone();
        let (store, settings) =
            tokio::task::spawn_blocking(move || -> Result<(Store, Settings)> {
                std::fs::create_dir_all(&cfg.app_data_dir)?;
                std::fs::create_dir_all(cfg.app_data_dir.join("recording"))?;
                let db = cfg.app_data_dir.join("index.sqlite");
                let mut store = Store::open(&cfg.library_dir, &db)?;
                let settings: Settings = store
                    .get_setting(SETTINGS_KEY)?
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default();
                if let Some(dir) = settings.library_dir.clone()
                    && dir != cfg.library_dir
                {
                    store = Store::open(&dir, &db)?;
                }
                if store.list_meetings(1, 0)?.is_empty() {
                    let n = store.scan()?;
                    if n > 0 {
                        tracing::info!("rebuilt the index from {n} meeting folders");
                    }
                }
                let reset = store.reset_running_on_startup()?;
                if reset > 0 {
                    tracing::info!("requeued {reset} jobs interrupted by the last shutdown");
                }
                Ok((store, settings))
            })
            .await
            .map_err(join_error)??;

        let (events, _) = broadcast::channel(2048);
        let engine = Arc::new(Engine {
            store: Arc::new(store),
            ports: config.ports.clone(),
            app_data: config.app_data_dir.clone(),
            cfg: config,
            events,
            settings: RwLock::new(settings),
            active: Mutex::new(None),
            starting: AtomicBool::new(false),
            inflight: Mutex::new(HashSet::new()),
            write_lock: Mutex::new(()),
            text_embedder: Mutex::new(None),
            jobs_notify: Arc::new(Notify::new()),
            outstanding: Mutex::new(HashMap::new()),
            pending_reindex: Mutex::new(HashSet::new()),
            shutdown: CancellationToken::new(),
            cancel_offline: Arc::new(AtomicBool::new(false)),
        });
        crate::jobs::spawn_runner(&engine);
        crate::retention::spawn_sweeper(&engine);
        engine.jobs_notify.notify_one();
        Ok(engine)
    }

    /// Subscribes to engine events. Slow receivers lose the oldest events (`Lagged`).
    pub fn events(&self) -> broadcast::Receiver<EngineEvent> {
        self.events.subscribe()
    }

    pub(crate) fn emit(&self, event: EngineEvent) {
        let _ = self.events.send(event);
    }

    pub(crate) fn events_sender(&self) -> broadcast::Sender<EngineEvent> {
        self.events.clone()
    }

    /// Emits a toast-style notice in the UI language: `ru` or `en` text.
    pub(crate) fn notice(&self, level: NoticeLevel, ru: &str, en: &str) {
        self.emit(EngineEvent::Notice {
            level,
            message: self.tr(ru, en).to_owned(),
        });
    }

    /// Picks the Russian or English text by the UI language setting.
    pub(crate) fn tr<'a>(&self, ru: &'a str, en: &'a str) -> &'a str {
        if self.settings().russian() { ru } else { en }
    }

    /// Current settings.
    pub fn settings(&self) -> Settings {
        self.settings.read().map(|s| s.clone()).unwrap_or_default()
    }

    /// Replaces and persists the settings.
    pub async fn set_settings(self: &Arc<Self>, new: Settings) -> Result<()> {
        let old = self.settings();
        let value = serde_json::to_value(&new).map_err(|e| Error::Storage(e.to_string()))?;
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || store.set_setting(SETTINGS_KEY, &value))
            .await
            .map_err(join_error)??;
        if let Ok(mut s) = self.settings.write() {
            *s = new.clone();
        }
        if old.models.text_embedding != new.models.text_embedding
            && let Ok(mut slot) = self.text_embedder.lock()
        {
            *slot = None;
        }
        if old.library_dir != new.library_dir {
            self.notice(
                NoticeLevel::Info,
                "Папка библиотеки изменится после перезапуска.",
                "The library folder changes after a restart.",
            );
        }
        Ok(())
    }

    /// Edits the settings in place and persists them.
    pub async fn update_settings(
        self: &Arc<Self>,
        f: impl FnOnce(&mut Settings),
    ) -> Result<Settings> {
        let mut s = self.settings();
        f(&mut s);
        self.set_settings(s.clone()).await?;
        Ok(s)
    }

    /// Stops background work. Sessions in progress are left on disk for recovery.
    pub fn shutdown(&self) {
        self.shutdown.cancel();
        self.cancel_offline
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Runs blocking work (store IO, CPU) off the async runtime.
    pub(crate) async fn blocking<T: Send + 'static>(
        self: &Arc<Self>,
        f: impl FnOnce(&Engine) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let me = Arc::clone(self);
        tokio::task::spawn_blocking(move || f(&me))
            .await
            .map_err(join_error)?
    }

    pub(crate) fn recording_root(&self) -> PathBuf {
        self.app_data.join("recording")
    }

    pub(crate) fn tmp_dir(&self) -> PathBuf {
        self.app_data.join("tmp")
    }

    /// Folder holding the meeting folders.
    pub fn library_root(&self) -> PathBuf {
        self.store.library_root().to_path_buf()
    }

    /// Runs `f` with the shared text embedder (loaded on first use); `None` when not installed.
    pub(crate) fn with_text_embedder<T>(
        &self,
        f: impl FnOnce(&mut dyn TextEmbedder) -> Result<T>,
    ) -> Result<Option<T>> {
        let mut slot = self
            .text_embedder
            .lock()
            .map_err(|_| Error::Storage("embedder lock poisoned".into()))?;
        if slot.is_none() {
            *slot = Some(self.ports.text_embedder.create()?);
        }
        match slot.as_mut().and_then(|e| e.as_mut()) {
            Some(embedder) => f(embedder.as_mut()).map(Some),
            None => Ok(None),
        }
    }

    pub(crate) fn mark_outstanding(&self, id: MeetingId, delta: i32) {
        if let Ok(mut map) = self.outstanding.lock() {
            let n = map.entry(id).or_insert(0);
            *n = (*n as i32 + delta).max(0) as u32;
            if *n == 0 {
                map.remove(&id);
            }
        }
    }

    /// Queues a job for `id` and wakes the runner.
    pub(crate) fn enqueue_job(
        &self,
        id: MeetingId,
        kind: &str,
        payload: &crate::jobs::JobPayload,
    ) -> Result<i64> {
        let value = serde_json::to_value(payload).map_err(|e| Error::Storage(e.to_string()))?;
        let job = self.store.enqueue_job(id, kind, &value)?;
        self.mark_outstanding(id, 1);
        self.jobs_notify.notify_one();
        Ok(job)
    }

    /// Waits until the meeting exists, all its jobs finished and its status is `Ready` or `Failed`.
    pub async fn wait_ready(
        self: &Arc<Self>,
        id: MeetingId,
        timeout: Duration,
    ) -> Result<MeetingStatus> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let busy = self
                .outstanding
                .lock()
                .map(|m| m.get(&id).copied().unwrap_or(0) > 0)
                .unwrap_or(false);
            if !busy {
                let store = Arc::clone(&self.store);
                let found = tokio::task::spawn_blocking(move || store.get_meeting(id))
                    .await
                    .map_err(join_error)?;
                match found {
                    Ok(rec)
                        if matches!(
                            rec.meeting.status,
                            MeetingStatus::Ready | MeetingStatus::Failed { .. }
                        ) =>
                    {
                        return Ok(rec.meeting.status);
                    }
                    Ok(_) | Err(Error::NotFound(_)) => {}
                    Err(e) => return Err(e),
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Invalid(format!("meeting {id} not ready in time")));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

pub(crate) fn join_error(e: tokio::task::JoinError) -> Error {
    Error::Storage(format!("background task failed: {e}"))
}
