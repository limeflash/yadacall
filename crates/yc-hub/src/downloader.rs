//! Resumable, hash-verified asynchronous downloads with mirror fallback.

use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use futures_util::{StreamExt, future::join_all};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, Semaphore, mpsc, oneshot, watch};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::{
    Error, Result,
    catalog::{ModelFile, ModelSpec},
};
use yc_core::events::{DownloadState, EngineEvent};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(125);
const SLOW_WINDOW: Duration = Duration::from_secs(15);
const SLOW_BYTES_PER_SECOND: u64 = 100 * 1024;
const MIN_DISK_HEADROOM: u64 = 128 * 1024 * 1024;
const MIN_SEGMENTED_SIZE: u64 = 32 * 1024 * 1024;
const MIN_CHUNK_SIZE: u64 = 16 * 1024 * 1024;
const MAX_CHUNK_SIZE: u64 = 32 * 1024 * 1024;
const STATE_CHECKPOINT_BYTES: u64 = 4 * 1024 * 1024;
const DEFAULT_CONNECTIONS: usize = 6;
const MAX_CONNECTIONS: usize = 16;

/// Download progress callback. Events are throttled to approximately eight per second.
pub type ProgressCallback = Arc<dyn Fn(EngineEvent) + Send + Sync>;

/// Tuning options for HTTP model downloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadOptions {
    /// Maximum number of active HTTP connections shared by this downloader.
    pub connections: usize,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        let connections = std::env::var("YADACALL_DOWNLOAD_CONNECTIONS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| (1..=MAX_CONNECTIONS).contains(value))
            .unwrap_or(DEFAULT_CONNECTIONS);
        Self { connections }
    }
}

/// Controls a download session. Pausing preserves partial data and resuming continues the same file.
#[derive(Clone, Default)]
pub struct DownloadControl {
    cancel: CancellationToken,
    pause: Arc<PauseState>,
}

struct PauseState {
    paused: AtomicBool,
    changed: watch::Sender<bool>,
}

impl Default for PauseState {
    fn default() -> Self {
        let (changed, _) = watch::channel(false);
        Self {
            paused: AtomicBool::new(false),
            changed,
        }
    }
}

impl DownloadControl {
    /// Pause after the next received chunk and preserve the partial file.
    pub fn pause(&self) {
        self.pause.paused.store(true, Ordering::Release);
        self.pause.changed.send_replace(true);
    }

    /// Resume a paused download.
    pub fn resume(&self) {
        self.pause.paused.store(false, Ordering::Release);
        self.pause.changed.send_replace(false);
    }

    /// Cancel the download. The downloader removes its partial data and state file.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    async fn wait_until_resumed(&self) -> Result<bool> {
        let mut paused = false;
        let mut changes = self.pause.changed.subscribe();
        loop {
            if self.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if !self.pause.paused.load(Ordering::Acquire) {
                return Ok(paused);
            }
            paused = true;
            tokio::select! {
                _ = self.cancel.cancelled() => return Err(Error::Cancelled),
                _ = changes.changed() => {}
            }
        }
    }
}

/// Owns the app data model/runtime directory and HTTP client.
#[derive(Clone)]
pub struct Downloader {
    root: PathBuf,
    client: reqwest::Client,
    progress: Option<ProgressCallback>,
    options: DownloadOptions,
    connections: Arc<Semaphore>,
}

impl Downloader {
    /// Create a downloader rooted at the app data directory.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        Self::with_progress(root, None)
    }

    /// Create a downloader with an optional engine-event sink.
    pub fn with_progress(
        root: impl Into<PathBuf>,
        progress: Option<ProgressCallback>,
    ) -> Result<Self> {
        Self::with_progress_and_options(root, progress, DownloadOptions::default())
    }

    /// Create a downloader with explicit connection options.
    pub fn with_options(root: impl Into<PathBuf>, options: DownloadOptions) -> Result<Self> {
        Self::with_progress_and_options(root, None, options)
    }

    /// Create a downloader with an event sink and explicit connection options.
    pub fn with_progress_and_options(
        root: impl Into<PathBuf>,
        progress: Option<ProgressCallback>,
        options: DownloadOptions,
    ) -> Result<Self> {
        if !(1..=MAX_CONNECTIONS).contains(&options.connections) {
            return Err(Error::Invalid(format!(
                "download connections must be between 1 and {MAX_CONNECTIONS}"
            )));
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(90))
            .user_agent(concat!("YadaCall/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| Error::Network(error.to_string()))?;
        Ok(Self {
            root: root.into(),
            client,
            progress,
            options,
            connections: Arc::new(Semaphore::new(options.connections)),
        })
    }

    /// App data root used by this downloader.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Download all required files for a model into `models/<id>/`.
    pub async fn download_model(
        &self,
        model: &ModelSpec,
        control: &DownloadControl,
    ) -> Result<PathBuf> {
        let model_dir = model_storage_dir(&self.root, model)?;
        tokio::fs::create_dir_all(&model_dir).await?;
        let required = model
            .files
            .iter()
            .filter(|file| !file.optional)
            .cloned()
            .collect::<Vec<_>>();
        let total_bytes = required
            .iter()
            .map(|file| file.size)
            .fold(0u64, u64::saturating_add);
        let tracker = Arc::new(ProgressTracker::new(
            model.id.clone(),
            total_bytes,
            self.progress.clone(),
        ));
        let mut pending = Vec::new();
        let mut bytes_to_download = 0u64;
        for file in &required {
            let target = model_dir.join(safe_relative_path(&file.path)?);
            if self.file_is_installed(&target, file, true).await? {
                tracker.set_file(&file.path, file.size);
                continue;
            }
            let part = append_suffix(&target, ".part");
            let state_path = append_suffix(&part, ".json");
            let done = existing_download_bytes(&state_path, &part, &model.id, file).await;
            bytes_to_download = bytes_to_download.saturating_add(file.size.saturating_sub(done));
            pending.push(file.clone());
        }
        self.preflight_space(&model_dir, bytes_to_download)?;
        let downloads = pending.into_iter().map(|file| {
            let downloader = self.clone();
            let item_id = model.id.clone();
            let install_dir = model_dir.clone();
            let control = control.clone();
            let tracker = tracker.clone();
            async move {
                downloader
                    .download_file_inner(&item_id, &file, &install_dir, &control, tracker)
                    .await
            }
        });
        for result in join_all(downloads).await {
            result?;
        }
        tracker.status(DownloadState::Done, true);
        Ok(model_dir)
    }

    /// Download an artifact to a specified installation directory.
    pub async fn download_file(
        &self,
        item_id: &str,
        spec: &ModelFile,
        install_dir: &Path,
        control: &DownloadControl,
    ) -> Result<PathBuf> {
        let tracker = Arc::new(ProgressTracker::new(
            item_id.to_owned(),
            spec.size,
            self.progress.clone(),
        ));
        let result = self
            .download_file_inner(item_id, spec, install_dir, control, tracker.clone())
            .await;
        if result.is_ok() {
            tracker.status(DownloadState::Done, true);
        }
        result
    }

    async fn download_file_inner(
        &self,
        item_id: &str,
        spec: &ModelFile,
        install_dir: &Path,
        control: &DownloadControl,
        tracker: Arc<ProgressTracker>,
    ) -> Result<PathBuf> {
        let relative = safe_relative_path(&spec.path)?;
        let target = install_dir.join(relative);
        if self.file_is_installed(&target, spec, true).await? {
            tracker.set_file(&spec.path, spec.size);
            return Ok(target);
        }
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let part = append_suffix(&target, ".part");
        let state_path = append_suffix(&part, ".json");
        let (state, is_valid_state) = load_part_state(&state_path, &part, item_id, spec).await?;
        if !is_valid_state {
            remove_file_if_present(&part).await?;
            remove_file_if_present(&state_path).await?;
        }
        let state = if is_valid_state {
            state
        } else {
            PartState::new(item_id, spec)
        };
        let done = state.downloaded_bytes();
        tracker.set_file(&spec.path, done);
        self.check_disk_space(&target, spec.size.saturating_sub(done))?;
        if spec.urls.is_empty() {
            return Err(Error::Invalid(format!(
                "file `{}` has no download URL",
                spec.path
            )));
        }
        let coordinator = StateCoordinator::new(state, &state_path).await?;
        let (writer_tx, writer_task) = spawn_state_writer(state_path.clone());
        let coordinator = coordinator.with_writer(writer_tx);
        let mut last_error = Error::Network("no URL could be reached".to_owned());
        let mut finished = false;
        let mut terminal = false;
        for url in &spec.urls {
            let mut refreshed = false;
            for attempt in 0..3u32 {
                let result = self
                    .transfer_from_url(url, spec, &part, control, &coordinator, tracker.clone())
                    .await;
                match result {
                    Ok(()) => {
                        finished = true;
                        break;
                    }
                    Err(TransferFailure::Cancelled) => {
                        last_error = Error::Cancelled;
                        finished = false;
                        terminal = true;
                        break;
                    }
                    Err(TransferFailure::SizeMismatch(message)) => {
                        last_error = Error::Integrity(target.clone());
                        let _ = message;
                        finished = false;
                        terminal = true;
                        break;
                    }
                    Err(TransferFailure::ExpiredSignature) if !refreshed => {
                        refreshed = true;
                        continue;
                    }
                    Err(TransferFailure::ExpiredSignature) => {
                        last_error = Error::Network(
                            "download URL remained expired after re-resolving".to_owned(),
                        );
                        break;
                    }
                    Err(TransferFailure::RangeUnsupported) => {
                        last_error =
                            Error::Network("server stopped honoring byte ranges".to_owned());
                        break;
                    }
                    Err(TransferFailure::SlowMirror(error))
                    | Err(TransferFailure::Mirror(error)) => {
                        last_error = error;
                        break;
                    }
                    Err(TransferFailure::Retryable(error)) => {
                        last_error = error;
                        if attempt < 2 {
                            let delay = Duration::from_millis(300 * (1 << attempt));
                            tokio::select! {
                                _ = control.cancel.cancelled() => {
                                    last_error = Error::Cancelled;
                                    break;
                                }
                                _ = tokio::time::sleep(delay) => {}
                            }
                            continue;
                        }
                        break;
                    }
                    Err(TransferFailure::Fatal(error)) => {
                        last_error = error;
                        terminal = true;
                        break;
                    }
                    Err(TransferFailure::Aborted) => {
                        last_error = Error::Network("download worker stopped".to_owned());
                        terminal = true;
                        break;
                    }
                }
            }
            if finished || terminal {
                break;
            }
        }

        coordinator.persist().await?;
        let done = coordinator.downloaded_bytes()?;
        drop(coordinator);
        if let Err(error) = writer_task.await {
            return Err(Error::Storage(format!(
                "download state writer failed: {error}"
            )));
        }

        if matches!(last_error, Error::Cancelled) {
            remove_file_if_present(&part).await?;
            remove_file_if_present(&state_path).await?;
            tracker.status(
                DownloadState::Failed {
                    message: "cancelled".to_owned(),
                    resumable: false,
                },
                true,
            );
            return Err(Error::Cancelled);
        }
        if matches!(last_error, Error::Integrity(_)) {
            remove_file_if_present(&part).await?;
            remove_file_if_present(&state_path).await?;
            tracker.status(
                DownloadState::Failed {
                    message: "download size did not match catalog".to_owned(),
                    resumable: false,
                },
                true,
            );
            return Err(Error::Integrity(target));
        }
        if !finished || done != spec.size {
            let error = if finished {
                Error::Network(format!(
                    "incomplete download of `{}` ({done}/{} bytes)",
                    spec.path, spec.size
                ))
            } else {
                last_error
            };
            tracker.status(
                DownloadState::Failed {
                    message: error.to_string(),
                    resumable: done > 0,
                },
                true,
            );
            return Err(error);
        }

        self.verify_and_install(&target, &part, &state_path, spec, tracker)
            .await
    }

    /// Verify installed model artifacts. Hashes may be skipped for a quick startup check.
    pub async fn installed(&self, model: &ModelSpec, verify_sha256: bool) -> Result<bool> {
        let directory = model_storage_dir(&self.root, model)?;
        for file in model.files.iter().filter(|file| !file.optional) {
            let path = directory.join(safe_relative_path(&file.path)?);
            if !self.file_is_installed(&path, file, verify_sha256).await? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Remove a downloaded model directory.
    pub async fn remove(&self, model_id: &str) -> Result<()> {
        remove_directory(&self.root.join("models").join(model_id)).await
    }

    /// Remove the repository-keyed model directory for a catalog entry.
    pub async fn remove_model(&self, model: &ModelSpec) -> Result<()> {
        remove_directory(&model_storage_dir(&self.root, model)?).await
    }

    /// Remove a downloaded runtime directory.
    pub async fn remove_runtime(&self, runtime_id: &str) -> Result<()> {
        remove_directory(&self.root.join("runtimes").join(runtime_id)).await
    }

    /// Check that a group of artifacts plus installer headroom fits on the target volume.
    pub fn preflight_space(&self, directory: &Path, bytes: u64) -> Result<()> {
        std::fs::create_dir_all(directory)?;
        let available = fs2::available_space(directory)?;
        let needed = bytes.saturating_add(MIN_DISK_HEADROOM);
        if available < needed {
            return Err(Error::DiskFull { needed, available });
        }
        Ok(())
    }

    async fn transfer_from_url(
        &self,
        url: &str,
        spec: &ModelFile,
        part: &Path,
        control: &DownloadControl,
        coordinator: &StateCoordinator,
        tracker: Arc<ProgressTracker>,
    ) -> std::result::Result<(), TransferFailure> {
        if control.is_cancelled() {
            return Err(TransferFailure::Cancelled);
        }
        if spec.size < MIN_SEGMENTED_SIZE {
            let (chunk_id, offset) = coordinator.prepare_single(spec.size).await?;
            prepare_part_file(part, spec.size)
                .await
                .map_err(TransferFailure::Fatal)?;
            return self
                .download_single(
                    url,
                    spec,
                    part,
                    chunk_id,
                    offset,
                    control,
                    coordinator,
                    tracker,
                )
                .await;
        }

        let resolved = self.probe_ranges(url, spec.size, control).await?;
        if !resolved.supports_ranges {
            let (chunk_id, offset) = coordinator.prepare_single(spec.size).await?;
            prepare_part_file(part, spec.size)
                .await
                .map_err(TransferFailure::Fatal)?;
            return self
                .download_single(
                    &resolved.final_url,
                    spec,
                    part,
                    chunk_id,
                    offset,
                    control,
                    coordinator,
                    tracker,
                )
                .await;
        }

        let tasks = coordinator
            .prepare_segment_tasks(spec.size, self.options.connections)
            .await?;
        prepare_part_file(part, spec.size)
            .await
            .map_err(TransferFailure::Fatal)?;
        if tasks.is_empty() {
            return Ok(());
        }
        match self
            .download_segments(
                &resolved.final_url,
                spec,
                part,
                tasks,
                control,
                coordinator,
                tracker.clone(),
            )
            .await
        {
            Err(TransferFailure::RangeUnsupported) => {
                let (chunk_id, offset) = coordinator.prepare_single(spec.size).await?;
                prepare_part_file(part, spec.size)
                    .await
                    .map_err(TransferFailure::Fatal)?;
                self.download_single(
                    &resolved.final_url,
                    spec,
                    part,
                    chunk_id,
                    offset,
                    control,
                    coordinator,
                    tracker,
                )
                .await
            }
            result => result,
        }
    }

    async fn probe_ranges(
        &self,
        url: &str,
        expected_size: u64,
        control: &DownloadControl,
    ) -> std::result::Result<ResolvedUrl, TransferFailure> {
        let response = tokio::select! {
            _ = control.cancel.cancelled() => return Err(TransferFailure::Cancelled),
            response = self.client.get(url)
                .header(reqwest::header::RANGE, "bytes=0-0")
                .send() => response.map_err(|error| TransferFailure::Retryable(network_error(error)))?,
        };
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            return Err(TransferFailure::ExpiredSignature);
        }
        let final_url = response.url().to_string();
        if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
            let range = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_content_range)
                .ok_or_else(|| {
                    TransferFailure::Retryable(Error::Network(
                        "range probe returned an invalid Content-Range".to_owned(),
                    ))
                })?;
            if range.start != 0 || range.end != 1 {
                return Err(TransferFailure::Retryable(Error::Network(
                    "range probe returned an unexpected byte range".to_owned(),
                )));
            }
            if range.total != expected_size {
                return Err(TransferFailure::SizeMismatch(format!(
                    "range probe reported {} bytes, expected {expected_size}",
                    range.total
                )));
            }
            return Ok(ResolvedUrl {
                final_url,
                supports_ranges: true,
            });
        }
        if response.status().is_success() {
            return Ok(ResolvedUrl {
                final_url,
                supports_ranges: false,
            });
        }
        Err(TransferFailure::Retryable(Error::Network(format!(
            "range probe returned HTTP {}",
            response.status()
        ))))
    }

    #[allow(clippy::too_many_arguments)]
    async fn download_segments(
        &self,
        url: &str,
        spec: &ModelFile,
        part: &Path,
        tasks: Vec<SegmentTask>,
        control: &DownloadControl,
        coordinator: &StateCoordinator,
        tracker: Arc<ProgressTracker>,
    ) -> std::result::Result<(), TransferFailure> {
        let queue = Arc::new(AsyncMutex::new(VecDeque::from(tasks)));
        let abort = CancellationToken::new();
        let _abort_on_drop = CancelOnDrop(abort.clone());
        let metrics = Arc::new(BatchSpeed::default());
        let monitor_abort = abort.clone();
        let monitor_control = control.clone();
        let monitor_metrics = metrics.clone();
        let monitor = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = monitor_abort.cancelled() => break,
                    _ = monitor_control.cancel.cancelled() => {
                        monitor_abort.cancel();
                        break;
                    }
                    _ = interval.tick() => {
                        if monitor_control.pause.paused.load(Ordering::Acquire) {
                            monitor_metrics.reset();
                        } else if monitor_metrics.is_slow() {
                            monitor_metrics.mark_slow();
                            monitor_abort.cancel();
                            break;
                        }
                    }
                }
            }
        });

        let mut workers = JoinSet::new();
        let worker_count = self.options.connections;
        for _ in 0..worker_count {
            let downloader = self.clone();
            let queue = queue.clone();
            let url = url.to_owned();
            let spec = spec.clone();
            let part = part.to_path_buf();
            let control = control.clone();
            let coordinator = coordinator.clone();
            let tracker = tracker.clone();
            let abort = abort.clone();
            let metrics = metrics.clone();
            workers.spawn(async move {
                segment_worker(
                    downloader,
                    url,
                    spec,
                    part,
                    queue,
                    control,
                    coordinator,
                    tracker,
                    abort,
                    metrics,
                )
                .await
            });
        }

        let mut failure = None;
        while let Some(result) = workers.join_next().await {
            match result {
                Ok(Some(error)) if failure.is_none() => {
                    failure = Some(error);
                    abort.cancel();
                }
                Ok(_) => {}
                Err(error) if failure.is_none() => {
                    failure = Some(TransferFailure::Fatal(Error::Storage(format!(
                        "download worker failed: {error}"
                    ))));
                    abort.cancel();
                }
                Err(_) => {}
            }
        }
        abort.cancel();
        let _ = monitor.await;
        if control.is_cancelled() {
            return Err(TransferFailure::Cancelled);
        }
        if let Some(error) = failure {
            return Err(error);
        }
        if metrics.was_slow() {
            return Err(TransferFailure::SlowMirror(Error::Network(
                "mirror remained below 100 KB/s for 15 seconds".to_owned(),
            )));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn download_single(
        &self,
        url: &str,
        spec: &ModelFile,
        part: &Path,
        chunk_id: u64,
        initial_offset: u64,
        control: &DownloadControl,
        coordinator: &StateCoordinator,
        tracker: Arc<ProgressTracker>,
    ) -> std::result::Result<(), TransferFailure> {
        let file =
            open_positioned_file(part).map_err(|error| TransferFailure::Fatal(error.into()))?;
        let mut chunk_id = chunk_id;
        let mut offset = initial_offset;
        let mut failures = 0u32;
        let mut checkpoint = offset;

        while offset < spec.size {
            if control.is_cancelled() {
                coordinator
                    .persist()
                    .await
                    .map_err(TransferFailure::Fatal)?;
                return Err(TransferFailure::Cancelled);
            }
            if control.pause.paused.load(Ordering::Acquire) {
                pause_download(control, &tracker, coordinator)
                    .await
                    .map_err(TransferFailure::Fatal)?;
            }
            let mut pause_changes = control.pause.changed.subscribe();
            if control.pause.paused.load(Ordering::Acquire) {
                pause_download(control, &tracker, coordinator)
                    .await
                    .map_err(TransferFailure::Fatal)?;
                continue;
            }
            let permit = tokio::select! {
                _ = control.cancel.cancelled() => return Err(TransferFailure::Cancelled),
                _ = pause_changes.changed() => {
                    pause_download(control, &tracker, coordinator)
                        .await
                        .map_err(TransferFailure::Fatal)?;
                    continue;
                }
                permit = self.connections.clone().acquire_owned() => permit
                    .map_err(|error| TransferFailure::Fatal(Error::Storage(error.to_string())))?,
            };
            let mut request = self.client.get(url);
            if offset > 0 {
                request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
            }
            let mut pause_changes = control.pause.changed.subscribe();
            if control.pause.paused.load(Ordering::Acquire) {
                drop(permit);
                pause_download(control, &tracker, coordinator)
                    .await
                    .map_err(TransferFailure::Fatal)?;
                continue;
            }
            let response = tokio::select! {
                _ = control.cancel.cancelled() => return Err(TransferFailure::Cancelled),
                _ = pause_changes.changed() => {
                    drop(permit);
                    pause_download(control, &tracker, coordinator)
                        .await
                        .map_err(TransferFailure::Fatal)?;
                    continue;
                }
                response = request.send() => match response {
                    Ok(response) => response,
                    Err(error) => {
                        drop(permit);
                        failures += 1;
                        if failures >= 3 {
                            return Err(TransferFailure::Mirror(network_error(error)));
                        }
                        retry_delay(control, failures).await?;
                        continue;
                    }
                },
            };
            if response.status() == reqwest::StatusCode::FORBIDDEN {
                return Err(TransferFailure::ExpiredSignature);
            }
            if offset > 0 && response.status() == reqwest::StatusCode::OK {
                if response
                    .content_length()
                    .is_some_and(|length| length != spec.size)
                {
                    return Err(TransferFailure::SizeMismatch(
                        "server Content-Length did not match the catalog size".to_owned(),
                    ));
                }
                offset = 0;
                checkpoint = 0;
                chunk_id = coordinator
                    .reset_single(spec.size, 0)
                    .map_err(TransferFailure::Fatal)?
                    .id;
                coordinator
                    .persist()
                    .await
                    .map_err(TransferFailure::Fatal)?;
            } else if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
                let content_range = response
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(parse_content_range);
                let Some(range) = content_range else {
                    return Err(TransferFailure::Mirror(Error::Network(
                        "server returned invalid Content-Range".to_owned(),
                    )));
                };
                if range.total != spec.size {
                    return Err(TransferFailure::SizeMismatch(format!(
                        "server reported {} bytes, expected {}",
                        range.total, spec.size
                    )));
                }
                if range.start != offset {
                    return Err(TransferFailure::Mirror(Error::Network(
                        "server resumed from an unexpected offset".to_owned(),
                    )));
                }
                if response
                    .content_length()
                    .is_some_and(|length| length != range.end.saturating_sub(range.start))
                {
                    return Err(TransferFailure::SizeMismatch(
                        "server range length did not match Content-Range".to_owned(),
                    ));
                }
            } else if !response.status().is_success() {
                failures += 1;
                drop(permit);
                if failures >= 3 {
                    return Err(TransferFailure::Mirror(Error::Network(format!(
                        "server returned HTTP {}",
                        response.status()
                    ))));
                }
                retry_delay(control, failures).await?;
                continue;
            } else if response
                .content_length()
                .is_some_and(|length| length != spec.size)
            {
                return Err(TransferFailure::SizeMismatch(
                    "server Content-Length did not match the catalog size".to_owned(),
                ));
            }

            let mut stream = response.bytes_stream();
            let mut interrupted = false;
            loop {
                let mut pause_changes = control.pause.changed.subscribe();
                if control.pause.paused.load(Ordering::Acquire) {
                    interrupted = true;
                    break;
                }
                let next = tokio::select! {
                    _ = control.cancel.cancelled() => {
                        coordinator.persist().await.map_err(TransferFailure::Fatal)?;
                        return Err(TransferFailure::Cancelled);
                    }
                    _ = pause_changes.changed() => {
                        interrupted = true;
                        break;
                    }
                    chunk = stream.next() => chunk,
                };
                let Some(next) = next else {
                    break;
                };
                let chunk = match next {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        failures += 1;
                        if failures >= 3 {
                            coordinator
                                .persist()
                                .await
                                .map_err(TransferFailure::Fatal)?;
                            return Err(TransferFailure::Mirror(network_error(error)));
                        }
                        break;
                    }
                };
                if offset.saturating_add(chunk.len() as u64) > spec.size {
                    return Err(TransferFailure::SizeMismatch(
                        "server sent more bytes than the catalog size".to_owned(),
                    ));
                }
                write_positioned(&file, &chunk, offset)
                    .map_err(|error| TransferFailure::Fatal(error.into()))?;
                offset = offset.saturating_add(chunk.len() as u64);
                coordinator
                    .set_chunk_offset(chunk_id, offset)
                    .map_err(TransferFailure::Fatal)?;
                tracker.record(
                    &spec.path,
                    coordinator
                        .downloaded_bytes()
                        .map_err(TransferFailure::Fatal)?,
                    chunk.len() as u64,
                );
                if offset.saturating_sub(checkpoint) >= STATE_CHECKPOINT_BYTES
                    || offset == spec.size
                {
                    coordinator
                        .persist()
                        .await
                        .map_err(TransferFailure::Fatal)?;
                    checkpoint = offset;
                }
                failures = 0;
            }
            drop(permit);

            if interrupted {
                coordinator
                    .persist()
                    .await
                    .map_err(TransferFailure::Fatal)?;
                pause_download(control, &tracker, coordinator)
                    .await
                    .map_err(TransferFailure::Fatal)?;
                continue;
            }
            if offset == spec.size {
                coordinator
                    .persist()
                    .await
                    .map_err(TransferFailure::Fatal)?;
                return Ok(());
            }
            failures += 1;
            if failures >= 3 {
                coordinator
                    .persist()
                    .await
                    .map_err(TransferFailure::Fatal)?;
                return Err(TransferFailure::Mirror(Error::Network(format!(
                    "stream ended at {offset} of {} bytes",
                    spec.size
                ))));
            }
            retry_delay(control, failures).await?;
        }
        if initial_offset == spec.size {
            Ok(())
        } else {
            let _ = chunk_id;
            Ok(())
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn download_segment_chunk(
        &self,
        url: &str,
        spec: &ModelFile,
        part: &Path,
        task: SegmentTask,
        control: &DownloadControl,
        coordinator: &StateCoordinator,
        tracker: &ProgressTracker,
        abort: &CancellationToken,
        metrics: &BatchSpeed,
    ) -> std::result::Result<(), TransferFailure> {
        let file =
            open_positioned_file(part).map_err(|error| TransferFailure::Fatal(error.into()))?;
        let mut offset = task.offset;
        let mut checkpoint = offset;
        let mut failures = 0u32;

        while offset < task.end {
            if control.is_cancelled() {
                coordinator
                    .persist()
                    .await
                    .map_err(TransferFailure::Fatal)?;
                return Err(TransferFailure::Cancelled);
            }
            if abort.is_cancelled() {
                return Err(TransferFailure::Aborted);
            }
            if control.pause.paused.load(Ordering::Acquire) {
                coordinator
                    .persist()
                    .await
                    .map_err(TransferFailure::Fatal)?;
                pause_download(control, tracker, coordinator)
                    .await
                    .map_err(TransferFailure::Fatal)?;
                metrics.reset();
            }

            let mut pause_changes = control.pause.changed.subscribe();
            if control.pause.paused.load(Ordering::Acquire) {
                coordinator
                    .persist()
                    .await
                    .map_err(TransferFailure::Fatal)?;
                pause_download(control, tracker, coordinator)
                    .await
                    .map_err(TransferFailure::Fatal)?;
                metrics.reset();
                continue;
            }
            let permit = tokio::select! {
                _ = control.cancel.cancelled() => return Err(TransferFailure::Cancelled),
                _ = abort.cancelled() => return Err(TransferFailure::Aborted),
                _ = pause_changes.changed() => {
                    coordinator.persist().await.map_err(TransferFailure::Fatal)?;
                    pause_download(control, tracker, coordinator)
                        .await
                        .map_err(TransferFailure::Fatal)?;
                    metrics.reset();
                    continue;
                }
                permit = self.connections.clone().acquire_owned() => permit
                    .map_err(|error| TransferFailure::Fatal(Error::Storage(error.to_string())))?,
            };
            let range_end = task.end.saturating_sub(1);
            let request = self.client.get(url).header(
                reqwest::header::RANGE,
                format!("bytes={offset}-{range_end}"),
            );
            let mut pause_changes = control.pause.changed.subscribe();
            if control.pause.paused.load(Ordering::Acquire) {
                drop(permit);
                coordinator
                    .persist()
                    .await
                    .map_err(TransferFailure::Fatal)?;
                pause_download(control, tracker, coordinator)
                    .await
                    .map_err(TransferFailure::Fatal)?;
                metrics.reset();
                continue;
            }
            let response = tokio::select! {
                _ = control.cancel.cancelled() => return Err(TransferFailure::Cancelled),
                _ = abort.cancelled() => return Err(TransferFailure::Aborted),
                _ = pause_changes.changed() => {
                    drop(permit);
                    coordinator.persist().await.map_err(TransferFailure::Fatal)?;
                    pause_download(control, tracker, coordinator)
                        .await
                        .map_err(TransferFailure::Fatal)?;
                    metrics.reset();
                    continue;
                }
                response = request.send() => match response {
                    Ok(response) => response,
                    Err(error) => {
                        drop(permit);
                        failures += 1;
                        if failures >= 3 {
                            coordinator.persist().await.map_err(TransferFailure::Fatal)?;
                            return Err(TransferFailure::Mirror(network_error(error)));
                        }
                        retry_delay(control, failures).await?;
                        continue;
                    }
                },
            };
            if response.status() == reqwest::StatusCode::FORBIDDEN {
                return Err(TransferFailure::ExpiredSignature);
            }
            if response.status() == reqwest::StatusCode::OK
                || response.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE
            {
                return Err(TransferFailure::RangeUnsupported);
            }
            if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                failures += 1;
                drop(permit);
                if failures >= 3 {
                    coordinator
                        .persist()
                        .await
                        .map_err(TransferFailure::Fatal)?;
                    return Err(TransferFailure::Mirror(Error::Network(format!(
                        "range request returned HTTP {}",
                        response.status()
                    ))));
                }
                retry_delay(control, failures).await?;
                continue;
            }
            let content_range = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_content_range);
            let Some(content_range) = content_range else {
                return Err(TransferFailure::Mirror(Error::Network(
                    "server returned invalid Content-Range".to_owned(),
                )));
            };
            if content_range.total != spec.size {
                return Err(TransferFailure::SizeMismatch(format!(
                    "server reported {} bytes, expected {}",
                    content_range.total, spec.size
                )));
            }
            if content_range.start != offset || content_range.end != task.end {
                return Err(TransferFailure::RangeUnsupported);
            }

            let mut stream = response.bytes_stream();
            let mut interrupted = false;
            loop {
                let mut pause_changes = control.pause.changed.subscribe();
                if control.pause.paused.load(Ordering::Acquire) {
                    interrupted = true;
                    break;
                }
                let next = tokio::select! {
                    _ = control.cancel.cancelled() => {
                        coordinator.persist().await.map_err(TransferFailure::Fatal)?;
                        return Err(TransferFailure::Cancelled);
                    }
                    _ = abort.cancelled() => {
                        coordinator.persist().await.map_err(TransferFailure::Fatal)?;
                        return Err(TransferFailure::Aborted);
                    }
                    _ = pause_changes.changed() => {
                        interrupted = true;
                        break;
                    }
                    chunk = stream.next() => chunk,
                };
                let Some(next) = next else {
                    break;
                };
                let chunk = match next {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        failures += 1;
                        if failures >= 3 {
                            coordinator
                                .persist()
                                .await
                                .map_err(TransferFailure::Fatal)?;
                            return Err(TransferFailure::Mirror(network_error(error)));
                        }
                        break;
                    }
                };
                if offset.saturating_add(chunk.len() as u64) > task.end {
                    return Err(TransferFailure::RangeUnsupported);
                }
                write_positioned(&file, &chunk, offset)
                    .map_err(|error| TransferFailure::Fatal(error.into()))?;
                offset = offset.saturating_add(chunk.len() as u64);
                coordinator
                    .set_chunk_offset(task.id, offset)
                    .map_err(TransferFailure::Fatal)?;
                let downloaded = coordinator
                    .downloaded_bytes()
                    .map_err(TransferFailure::Fatal)?;
                tracker.record(&spec.path, downloaded, chunk.len() as u64);
                metrics.record(chunk.len() as u64);
                if offset.saturating_sub(checkpoint) >= STATE_CHECKPOINT_BYTES || offset == task.end
                {
                    coordinator
                        .persist()
                        .await
                        .map_err(TransferFailure::Fatal)?;
                    checkpoint = offset;
                }
                failures = 0;
            }
            drop(permit);

            if interrupted {
                coordinator
                    .persist()
                    .await
                    .map_err(TransferFailure::Fatal)?;
                pause_download(control, tracker, coordinator)
                    .await
                    .map_err(TransferFailure::Fatal)?;
                metrics.reset();
                continue;
            }
            if offset == task.end {
                coordinator
                    .persist()
                    .await
                    .map_err(TransferFailure::Fatal)?;
                return Ok(());
            }
            failures += 1;
            if failures >= 3 {
                coordinator
                    .persist()
                    .await
                    .map_err(TransferFailure::Fatal)?;
                return Err(TransferFailure::Mirror(Error::Network(format!(
                    "range stream ended at {offset} of {} bytes",
                    task.end
                ))));
            }
            retry_delay(control, failures).await?;
        }
        coordinator
            .persist()
            .await
            .map_err(TransferFailure::Fatal)?;
        Ok(())
    }

    async fn verify_and_install(
        &self,
        target: &Path,
        part: &Path,
        state_path: &Path,
        spec: &ModelFile,
        tracker: Arc<ProgressTracker>,
    ) -> Result<PathBuf> {
        sync_file_async(part).await?;
        let metadata = tokio::fs::metadata(part).await?;
        if metadata.len() != spec.size {
            remove_file_if_present(part).await?;
            remove_file_if_present(state_path).await?;
            tracker.status(
                DownloadState::Failed {
                    message: format!("size mismatch: received {} bytes", metadata.len()),
                    resumable: false,
                },
                true,
            );
            return Err(Error::Integrity(target.to_path_buf()));
        }
        tracker.status(DownloadState::Verifying, true);
        let actual_hash = match hash_file_async(part).await {
            Ok(hash) => hash,
            Err(error) => {
                remove_file_if_present(part).await?;
                remove_file_if_present(state_path).await?;
                tracker.status(
                    DownloadState::Failed {
                        message: format!("SHA-256 verification failed: {error}"),
                        resumable: false,
                    },
                    true,
                );
                return Err(error);
            }
        };
        if actual_hash != spec.sha256.to_ascii_lowercase() {
            remove_file_if_present(part).await?;
            remove_file_if_present(state_path).await?;
            tracker.status(
                DownloadState::Failed {
                    message: "SHA-256 mismatch".to_owned(),
                    resumable: false,
                },
                true,
            );
            return Err(Error::Integrity(target.to_path_buf()));
        }
        atomic_install(part, target).await?;
        remove_file_if_present(state_path).await?;
        Ok(target.to_path_buf())
    }

    async fn file_is_installed(
        &self,
        path: &Path,
        spec: &ModelFile,
        verify_sha256: bool,
    ) -> Result<bool> {
        let metadata = match tokio::fs::metadata(path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        if metadata.len() != spec.size {
            return Ok(false);
        }
        if verify_sha256 {
            Ok(hash_file_async(path).await? == spec.sha256.to_ascii_lowercase())
        } else {
            Ok(true)
        }
    }

    fn check_disk_space(&self, target: &Path, remaining: u64) -> Result<()> {
        let root = target.parent().unwrap_or(&self.root);
        self.preflight_space(root, remaining)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ByteRange {
    start: u64,
    /// Exclusive end offset.
    end: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InFlightChunk {
    id: u64,
    start: u64,
    /// Exclusive end offset.
    end: u64,
    /// Absolute offset of the next byte to request.
    offset: u64,
}

#[derive(Clone, Copy, Debug)]
struct SegmentTask {
    id: u64,
    end: u64,
    offset: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartState {
    /// Missing on v1 state files written by the original single-stream downloader.
    #[serde(default = "legacy_state_version")]
    version: u8,
    item_id: String,
    path: String,
    sha256: String,
    size: u64,
    #[serde(default)]
    completed_ranges: Vec<ByteRange>,
    #[serde(default)]
    in_flight_chunks: Vec<InFlightChunk>,
    /// Present only in legacy v1 state files; omitted when writing v2.
    #[serde(default, skip_serializing)]
    downloaded_bytes: u64,
    saved_at_unix: u64,
}

impl PartState {
    fn new(item_id: &str, spec: &ModelFile) -> Self {
        Self {
            version: 2,
            item_id: item_id.to_owned(),
            path: spec.path.clone(),
            sha256: spec.sha256.clone(),
            size: spec.size,
            completed_ranges: Vec::new(),
            in_flight_chunks: Vec::new(),
            downloaded_bytes: 0,
            saved_at_unix: unix_timestamp(),
        }
    }

    fn downloaded_bytes(&self) -> u64 {
        self.completed_ranges
            .iter()
            .map(|range| range.end.saturating_sub(range.start))
            .chain(self.in_flight_chunks.iter().map(|chunk| {
                chunk
                    .offset
                    .saturating_sub(chunk.start)
                    .min(chunk.end - chunk.start)
            }))
            .fold(0u64, u64::saturating_add)
            .min(self.size)
    }

    fn downloaded_ranges(&self) -> Vec<ByteRange> {
        let mut ranges = self.completed_ranges.clone();
        ranges.extend(self.in_flight_chunks.iter().filter_map(|chunk| {
            (chunk.offset > chunk.start).then_some(ByteRange {
                start: chunk.start,
                end: chunk.offset,
            })
        }));
        merge_ranges(ranges)
    }

    fn is_valid_v2(&self) -> bool {
        if self.version != 2 {
            return false;
        }
        let mut ranges = self.completed_ranges.clone();
        for chunk in &self.in_flight_chunks {
            if chunk.start >= chunk.end
                || chunk.end > self.size
                || chunk.offset < chunk.start
                || chunk.offset > chunk.end
            {
                return false;
            }
            ranges.push(ByteRange {
                start: chunk.start,
                end: chunk.end,
            });
        }
        if self
            .completed_ranges
            .iter()
            .any(|range| range.start >= range.end || range.end > self.size)
        {
            return false;
        }
        ranges.sort_by_key(|range| (range.start, range.end));
        ranges.windows(2).all(|pair| pair[0].end <= pair[1].start)
    }
}

fn legacy_state_version() -> u8 {
    1
}

#[derive(Debug)]
enum TransferFailure {
    Cancelled,
    ExpiredSignature,
    RangeUnsupported,
    SizeMismatch(String),
    Retryable(Error),
    Mirror(Error),
    SlowMirror(Error),
    Fatal(Error),
    Aborted,
}

struct CancelOnDrop(CancellationToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl From<Error> for TransferFailure {
    fn from(error: Error) -> Self {
        Self::Fatal(error)
    }
}

struct ResolvedUrl {
    final_url: String,
    supports_ranges: bool,
}

struct ParsedContentRange {
    start: u64,
    /// Exclusive end offset.
    end: u64,
    total: u64,
}

struct StateMessage {
    state: PartState,
    acknowledgement: oneshot::Sender<std::result::Result<(), String>>,
}

#[derive(Clone)]
struct StateCoordinator {
    state: Arc<Mutex<PartState>>,
    state_path: PathBuf,
    writer: Option<mpsc::UnboundedSender<StateMessage>>,
}

impl StateCoordinator {
    async fn new(state: PartState, state_path: &Path) -> Result<Self> {
        write_part_state(state_path, &state).await?;
        Ok(Self {
            state: Arc::new(Mutex::new(state)),
            state_path: state_path.to_path_buf(),
            writer: None,
        })
    }

    fn with_writer(mut self, writer: mpsc::UnboundedSender<StateMessage>) -> Self {
        self.writer = Some(writer);
        self
    }

    async fn persist(&self) -> Result<()> {
        let (snapshot, acknowledgement) = {
            let mut state = self.lock_state()?;
            state.saved_at_unix = unix_timestamp();
            let snapshot = state.clone();
            let acknowledgement = if let Some(writer) = &self.writer {
                let (sender, receiver) = oneshot::channel();
                writer
                    .send(StateMessage {
                        state: snapshot.clone(),
                        acknowledgement: sender,
                    })
                    .map_err(|_| Error::Storage("download state writer stopped".to_owned()))?;
                Some(receiver)
            } else {
                None
            };
            (snapshot, acknowledgement)
        };
        if let Some(acknowledgement) = acknowledgement {
            acknowledgement
                .await
                .map_err(|error| {
                    Error::Storage(format!("state write acknowledgement failed: {error}"))
                })?
                .map_err(Error::Storage)?;
        } else {
            write_part_state(&self.state_path, &snapshot).await?;
        }
        Ok(())
    }

    async fn prepare_single(&self, size: u64) -> Result<(u64, u64)> {
        let offset = {
            let state = self.lock_state()?;
            contiguous_prefix(&state.downloaded_ranges())
        };
        let task = self.reset_single(size, offset)?;
        self.persist().await?;
        Ok((task.id, task.offset))
    }

    async fn prepare_segment_tasks(
        &self,
        size: u64,
        connections: usize,
    ) -> Result<Vec<SegmentTask>> {
        let tasks = {
            let mut state = self.lock_state()?;
            let mut occupied = state.completed_ranges.clone();
            occupied.extend(state.in_flight_chunks.iter().map(|chunk| ByteRange {
                start: chunk.start,
                end: chunk.end,
            }));
            let occupied = merge_ranges(occupied);
            let mut tasks = state
                .in_flight_chunks
                .iter()
                .filter(|chunk| chunk.offset < chunk.end)
                .map(|chunk| SegmentTask {
                    id: chunk.id,
                    end: chunk.end,
                    offset: chunk.offset,
                })
                .collect::<Vec<_>>();
            let per_connection = size
                .saturating_add(connections as u64 - 1)
                .checked_div(connections as u64)
                .unwrap_or(size);
            let chunk_size = per_connection.clamp(MIN_CHUNK_SIZE, MAX_CHUNK_SIZE);
            let mut cursor = 0u64;
            let mut next_id = state
                .in_flight_chunks
                .iter()
                .map(|chunk| chunk.id)
                .max()
                .unwrap_or_default()
                .saturating_add(1);
            for range in occupied {
                if range.start > cursor {
                    add_missing_tasks(
                        cursor,
                        range.start,
                        chunk_size,
                        &mut next_id,
                        &mut state,
                        &mut tasks,
                    );
                }
                cursor = cursor.max(range.end);
            }
            if cursor < size {
                add_missing_tasks(
                    cursor,
                    size,
                    chunk_size,
                    &mut next_id,
                    &mut state,
                    &mut tasks,
                );
            }
            tasks
        };
        self.persist().await?;
        Ok(tasks)
    }

    fn reset_single(&self, size: u64, offset: u64) -> Result<SegmentTask> {
        let mut state = self.lock_state()?;
        let id = 1;
        let offset = offset.min(size);
        state.completed_ranges.clear();
        state.in_flight_chunks.clear();
        if offset == size && size > 0 {
            state.completed_ranges.push(ByteRange {
                start: 0,
                end: size,
            });
        } else if size > 0 {
            state.in_flight_chunks.push(InFlightChunk {
                id,
                start: 0,
                end: size,
                offset,
            });
        }
        Ok(SegmentTask {
            id,
            end: size,
            offset,
        })
    }

    fn set_chunk_offset(&self, id: u64, offset: u64) -> Result<()> {
        let mut state = self.lock_state()?;
        let Some(index) = state
            .in_flight_chunks
            .iter()
            .position(|chunk| chunk.id == id)
        else {
            return Err(Error::Storage(format!(
                "download state lost in-flight chunk {id}"
            )));
        };
        let chunk = &mut state.in_flight_chunks[index];
        if offset < chunk.offset || offset > chunk.end {
            return Err(Error::Storage(format!(
                "invalid offset {offset} for chunk {}..{}",
                chunk.start, chunk.end
            )));
        }
        chunk.offset = offset;
        if offset == chunk.end {
            let chunk = state.in_flight_chunks.remove(index);
            state.completed_ranges.push(ByteRange {
                start: chunk.start,
                end: chunk.end,
            });
            state.completed_ranges = merge_ranges(std::mem::take(&mut state.completed_ranges));
        }
        Ok(())
    }

    fn downloaded_bytes(&self) -> Result<u64> {
        Ok(self.lock_state()?.downloaded_bytes())
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, PartState>> {
        self.state
            .lock()
            .map_err(|error| Error::Storage(format!("download state lock failed: {error}")))
    }
}

fn add_missing_tasks(
    start: u64,
    end: u64,
    chunk_size: u64,
    next_id: &mut u64,
    state: &mut PartState,
    tasks: &mut Vec<SegmentTask>,
) {
    let mut offset = start;
    while offset < end {
        let chunk_end = offset.saturating_add(chunk_size).min(end);
        let id = *next_id;
        *next_id = next_id.saturating_add(1);
        let chunk = InFlightChunk {
            id,
            start: offset,
            end: chunk_end,
            offset,
        };
        state.in_flight_chunks.push(chunk);
        tasks.push(SegmentTask {
            id,
            end: chunk_end,
            offset,
        });
        offset = chunk_end;
    }
}

fn spawn_state_writer(
    state_path: PathBuf,
) -> (
    mpsc::UnboundedSender<StateMessage>,
    tokio::task::JoinHandle<()>,
) {
    let (sender, mut receiver) = mpsc::unbounded_channel::<StateMessage>();
    let task = tokio::spawn(async move {
        while let Some(message) = receiver.recv().await {
            let result = write_part_state(&state_path, &message.state)
                .await
                .map_err(|error| error.to_string());
            let _ = message.acknowledgement.send(result);
        }
    });
    (sender, task)
}

async fn write_part_state(state_path: &Path, state: &PartState) -> Result<()> {
    let Some(parent) = state_path.parent() else {
        return Err(Error::Invalid(
            "download state needs a parent directory".to_owned(),
        ));
    };
    tokio::fs::create_dir_all(parent).await?;
    let data = serde_json::to_vec(state)
        .map_err(|error| Error::Storage(format!("serialize download state: {error}")))?;
    let temp = append_suffix(state_path, ".tmp");
    tokio::fs::write(&temp, data).await?;
    atomic_install(&temp, state_path).await
}

#[derive(Default)]
struct ProgressData {
    files: HashMap<String, u64>,
    samples: VecDeque<(Instant, u64)>,
    speed_started: Option<Instant>,
    last_emit: Option<Instant>,
    last_state: Option<DownloadState>,
}

struct ProgressTracker {
    item_id: String,
    total: u64,
    callback: Option<ProgressCallback>,
    data: Mutex<ProgressData>,
}

impl ProgressTracker {
    fn new(item_id: String, total: u64, callback: Option<ProgressCallback>) -> Self {
        Self {
            item_id,
            total,
            callback,
            data: Mutex::new(ProgressData::default()),
        }
    }

    fn set_file(&self, file: &str, bytes: u64) {
        lock_unpoisoned(&self.data)
            .files
            .insert(file.to_owned(), bytes.min(self.total));
    }

    fn record(&self, file: &str, bytes: u64, delta: u64) {
        let now = Instant::now();
        let event = {
            let mut data = lock_unpoisoned(&self.data);
            data.files.insert(file.to_owned(), bytes.min(self.total));
            if delta > 0 {
                data.samples.push_back((now, delta));
                data.speed_started.get_or_insert(now);
            }
            trim_samples(&mut data.samples, now);
            let changed = data.last_state.as_ref() != Some(&DownloadState::Running);
            let due = data
                .last_emit
                .is_none_or(|last| now.duration_since(last) >= PROGRESS_INTERVAL);
            if !due && !changed {
                None
            } else {
                data.last_emit = Some(now);
                data.last_state = Some(DownloadState::Running);
                Some(EngineEvent::Download {
                    item: self.item_id.clone(),
                    done_bytes: data.files.values().copied().sum::<u64>().min(self.total),
                    total_bytes: self.total,
                    bytes_per_sec: progress_speed(&data, now),
                    state: DownloadState::Running,
                })
            }
        };
        self.send(event);
    }

    fn status(&self, state: DownloadState, force: bool) {
        let now = Instant::now();
        let event = {
            let mut data = lock_unpoisoned(&self.data);
            let changed = data.last_state.as_ref() != Some(&state);
            let due = data
                .last_emit
                .is_none_or(|last| now.duration_since(last) >= PROGRESS_INTERVAL);
            if !force && !changed && !due {
                None
            } else {
                data.last_emit = Some(now);
                data.last_state = Some(state.clone());
                Some(EngineEvent::Download {
                    item: self.item_id.clone(),
                    done_bytes: data.files.values().copied().sum::<u64>().min(self.total),
                    total_bytes: self.total,
                    bytes_per_sec: progress_speed(&data, now),
                    state,
                })
            }
        };
        self.send(event);
    }

    fn reset_speed_window(&self) {
        let mut data = lock_unpoisoned(&self.data);
        data.samples.clear();
        data.speed_started = Some(Instant::now());
    }

    fn send(&self, event: Option<EngineEvent>) {
        if let (Some(callback), Some(event)) = (&self.callback, event) {
            callback(event);
        }
    }
}

fn trim_samples(samples: &mut VecDeque<(Instant, u64)>, now: Instant) {
    while samples
        .front()
        .is_some_and(|(time, _)| now.duration_since(*time) > SLOW_WINDOW)
    {
        samples.pop_front();
    }
}

fn progress_speed(data: &ProgressData, now: Instant) -> u64 {
    let Some(started) = data.speed_started else {
        return 0;
    };
    let window_start = started.max(now.checked_sub(SLOW_WINDOW).unwrap_or(started));
    let elapsed = now.duration_since(window_start).as_secs_f64().max(1.0);
    let bytes = data
        .samples
        .iter()
        .filter(|(time, _)| *time >= window_start)
        .map(|(_, count)| *count)
        .sum::<u64>();
    (bytes as f64 / elapsed) as u64
}

struct BatchSpeed {
    data: Mutex<BatchSpeedData>,
    slow: AtomicBool,
}

struct BatchSpeedData {
    samples: VecDeque<(Instant, u64)>,
    started: Instant,
}

impl Default for BatchSpeed {
    fn default() -> Self {
        Self {
            data: Mutex::new(BatchSpeedData {
                samples: VecDeque::new(),
                started: Instant::now(),
            }),
            slow: AtomicBool::new(false),
        }
    }
}

impl BatchSpeed {
    fn record(&self, bytes: u64) {
        let now = Instant::now();
        let mut data = lock_unpoisoned(&self.data);
        data.samples.push_back((now, bytes));
        trim_samples(&mut data.samples, now);
    }

    fn reset(&self) {
        let mut data = lock_unpoisoned(&self.data);
        data.samples.clear();
        data.started = Instant::now();
    }

    fn is_slow(&self) -> bool {
        let now = Instant::now();
        let data = lock_unpoisoned(&self.data);
        if now.duration_since(data.started) < SLOW_WINDOW {
            return false;
        }
        let bytes = data
            .samples
            .iter()
            .filter(|(time, _)| now.duration_since(*time) <= SLOW_WINDOW)
            .map(|(_, count)| *count)
            .sum::<u64>();
        bytes / SLOW_WINDOW.as_secs() < SLOW_BYTES_PER_SECOND
    }

    fn mark_slow(&self) {
        self.slow.store(true, Ordering::Release);
    }

    fn was_slow(&self) -> bool {
        self.slow.load(Ordering::Acquire)
    }
}

#[allow(clippy::too_many_arguments)]
async fn segment_worker(
    downloader: Downloader,
    url: String,
    spec: ModelFile,
    part: PathBuf,
    queue: Arc<AsyncMutex<VecDeque<SegmentTask>>>,
    control: DownloadControl,
    coordinator: StateCoordinator,
    tracker: Arc<ProgressTracker>,
    abort: CancellationToken,
    metrics: Arc<BatchSpeed>,
) -> Option<TransferFailure> {
    loop {
        if control.is_cancelled() {
            return Some(TransferFailure::Cancelled);
        }
        if abort.is_cancelled() {
            return None;
        }
        let task = queue.lock().await.pop_front();
        let task = task?;
        match downloader
            .download_segment_chunk(
                &url,
                &spec,
                &part,
                task,
                &control,
                &coordinator,
                &tracker,
                &abort,
                &metrics,
            )
            .await
        {
            Ok(()) => {}
            Err(TransferFailure::Aborted) => return None,
            Err(error) => {
                abort.cancel();
                return Some(error);
            }
        }
    }
}

async fn pause_download(
    control: &DownloadControl,
    tracker: &ProgressTracker,
    coordinator: &StateCoordinator,
) -> Result<()> {
    tracker.status(DownloadState::Paused, false);
    coordinator.persist().await?;
    control.wait_until_resumed().await?;
    tracker.reset_speed_window();
    tracker.status(DownloadState::Running, false);
    Ok(())
}

async fn retry_delay(
    control: &DownloadControl,
    attempt: u32,
) -> std::result::Result<(), TransferFailure> {
    let delay = Duration::from_millis(200u64.saturating_mul(1 << attempt.saturating_sub(1)));
    tokio::select! {
        _ = control.cancel.cancelled() => Err(TransferFailure::Cancelled),
        _ = tokio::time::sleep(delay) => Ok(()),
    }
}

fn merge_ranges(mut ranges: Vec<ByteRange>) -> Vec<ByteRange> {
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<ByteRange> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

fn contiguous_prefix(ranges: &[ByteRange]) -> u64 {
    merge_ranges(ranges.to_vec())
        .first()
        .filter(|range| range.start == 0)
        .map_or(0, |range| range.end)
}

fn parse_content_range(value: &str) -> Option<ParsedContentRange> {
    let range = value.strip_prefix("bytes ")?;
    let (range, total) = range.split_once('/')?;
    let (start, inclusive_end) = range.split_once('-')?;
    let start = start.parse::<u64>().ok()?;
    let inclusive_end = inclusive_end.parse::<u64>().ok()?;
    Some(ParsedContentRange {
        start,
        end: inclusive_end.checked_add(1)?,
        total: total.parse::<u64>().ok()?,
    })
}

async fn prepare_part_file(path: &Path, size: u64) -> Result<()> {
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
        .await?;
    file.set_len(size).await?;
    Ok(())
}

fn open_positioned_file(path: &Path) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
}

fn write_positioned(file: &File, mut bytes: &[u8], mut offset: u64) -> io::Result<()> {
    while !bytes.is_empty() {
        #[cfg(unix)]
        let written = {
            use std::os::unix::fs::FileExt;
            file.write_at(bytes, offset)?
        };
        #[cfg(windows)]
        let written = {
            use std::os::windows::fs::FileExt;
            file.seek_write(bytes, offset)?
        };
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "positioned download write returned zero bytes",
            ));
        }
        offset = offset.saturating_add(written as u64);
        bytes = &bytes[written..];
    }
    Ok(())
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

async fn load_part_state(
    state_path: &Path,
    part_path: &Path,
    item_id: &str,
    spec: &ModelFile,
) -> Result<(PartState, bool)> {
    let empty = PartState::new(item_id, spec);
    let part_metadata = match tokio::fs::metadata(part_path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((empty, false)),
        Err(error) => return Err(error.into()),
    };
    let bytes = match tokio::fs::read(state_path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((empty, false)),
        Err(error) => return Err(error.into()),
    };
    let Ok(mut state) = serde_json::from_slice::<PartState>(&bytes) else {
        return Ok((empty, false));
    };
    if state.item_id != item_id
        || state.path != spec.path
        || state.sha256 != spec.sha256
        || state.size != spec.size
    {
        return Ok((empty, false));
    }
    if state.version < 2 {
        let downloaded = state
            .downloaded_bytes
            .min(part_metadata.len())
            .min(spec.size);
        state.version = 2;
        state.completed_ranges = if downloaded == 0 {
            Vec::new()
        } else {
            vec![ByteRange {
                start: 0,
                end: downloaded,
            }]
        };
        state.in_flight_chunks.clear();
        state.downloaded_bytes = 0;
        return Ok((state, true));
    }
    if !state.is_valid_v2() || state.downloaded_bytes() > part_metadata.len() {
        return Ok((empty, false));
    }
    Ok((state, true))
}

async fn existing_download_bytes(
    state_path: &Path,
    part_path: &Path,
    item_id: &str,
    spec: &ModelFile,
) -> u64 {
    load_part_state(state_path, part_path, item_id, spec)
        .await
        .map(|(state, valid)| if valid { state.downloaded_bytes() } else { 0 })
        .unwrap_or_default()
}

async fn remove_file_if_present(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Resolve a catalog model to its shared-cache directory, keyed by upstream repository.
pub fn model_storage_dir(root: &Path, model: &ModelSpec) -> Result<PathBuf> {
    let storage_key =
        if let Some(repo) = model.files.iter().find_map(|file| file.hf_repo.as_deref()) {
            let mut parts = repo.split('/');
            let owner = parts.next().unwrap_or_default();
            let name = parts.next().unwrap_or_default();
            let valid_part = |part: &str| {
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            };
            if parts.next().is_some() || !valid_part(owner) || !valid_part(name) {
                return Err(Error::Invalid(format!(
                    "invalid Hugging Face repository `{repo}`"
                )));
            }
            format!("{owner}__{name}")
        } else {
            let id = model.id.as_str();
            if id.is_empty()
                || id == "."
                || id == ".."
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            {
                return Err(Error::Invalid(format!("invalid model id `{id}`")));
            }
            id.to_owned()
        };
    Ok(root.join("models").join(storage_key))
}

/// Check that all files are present and have their catalog lengths, without hashing them.
pub async fn installed(root: &Path, model: &ModelSpec) -> Result<bool> {
    let downloader = Downloader::new(root)?;
    downloader.installed(model, false).await
}

/// Remove an installed model directory.
pub async fn remove(root: &Path, model_id: &str) -> Result<()> {
    Downloader::new(root)?.remove(model_id).await
}

async fn hash_file_async(path: &Path) -> Result<String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 1024 * 1024];
        loop {
            use std::io::Read;
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        Ok::<String, std::io::Error>(hex::encode(hasher.finalize()))
    })
    .await
    .map_err(|error| Error::Storage(format!("hash worker failed: {error}")))?
    .map_err(Error::Io)
}

async fn sync_file_async(path: &Path) -> Result<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?
            .sync_all()
    })
    .await
    .map_err(|error| Error::Storage(format!("download sync worker failed: {error}")))??;
    Ok(())
}

fn safe_relative_path(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(Error::Invalid(
            "download paths must be relative and stay in the install directory".to_owned(),
        ));
    }
    Ok(path.to_path_buf())
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

async fn remove_directory(path: &Path) -> Result<()> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

async fn atomic_install(source: &Path, destination: &Path) -> Result<()> {
    let source = source.to_path_buf();
    let destination = destination.to_path_buf();
    tokio::task::spawn_blocking(move || atomic_replace(&source, &destination))
        .await
        .map_err(|error| Error::Storage(format!("atomic install worker failed: {error}")))??;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn atomic_replace(source: &Path, destination: &Path) -> Result<()> {
    std::fs::rename(source, destination)?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn atomic_replace(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        },
        core::PCWSTR,
    };

    let source: Vec<u16> = source.as_os_str().encode_wide().chain([0]).collect();
    let destination: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        MoveFileExW(
            PCWSTR::from_raw(source.as_ptr()),
            PCWSTR::from_raw(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|error| Error::Storage(format!("atomic file replacement failed: {error}")))?;
    Ok(())
}

fn network_error(error: reqwest::Error) -> Error {
    Error::Network(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        path::{Path, PathBuf},
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use sha2::{Digest, Sha256};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        sync::mpsc,
    };
    use tokio_util::sync::CancellationToken;

    use super::{
        DownloadControl, DownloadOptions, Downloader, ModelFile, ProgressCallback,
        model_storage_dir,
    };
    use crate::catalog::{ModelCost, ModelKind, ModelSpec, Requirements};
    use yc_core::events::{DownloadState, EngineEvent};

    const LARGE_FILE_SIZE: usize = 32 * 1024 * 1024 + 129;

    #[test]
    fn catalog_models_use_repository_keyed_shared_cache_directories() {
        let catalog = crate::catalog::embedded().expect("embedded catalog parses");
        let model = crate::catalog::model(&catalog, "gemma-4-e2b-q4-0").expect("Gemma entry");
        let path = model_storage_dir(Path::new("/cache"), model).expect("safe repository key");
        assert_eq!(
            path,
            Path::new("/cache/models/ggml-org__gemma-4-E2B-it-GGUF")
        );
    }

    #[tokio::test]
    async fn segmented_downloads_are_parallel_and_hash_verified() {
        let bytes = fixture_bytes(LARGE_FILE_SIZE);
        let digest = digest(&bytes);
        let server = TestServer::start(
            bytes.clone(),
            TestServerOptions {
                header_delay: Duration::from_millis(80),
                ..TestServerOptions::default()
            },
        )
        .await;
        let root = temp_dir("parallel");
        let downloader = Downloader::with_options(&root, DownloadOptions { connections: 3 })
            .expect("downloader");
        let spec = spec_for(
            "payload.bin",
            bytes.len() as u64,
            digest,
            vec![server.url()],
        );

        let path = downloader
            .download_file("test", &spec, &root, &DownloadControl::default())
            .await
            .expect("download");
        assert_eq!(
            tokio::fs::read(&path).await.expect("installed bytes"),
            bytes
        );
        assert!(server.state.peak_active.load(Ordering::Acquire) >= 2);
        assert!(server.state.peak_active.load(Ordering::Acquire) <= 3);
        assert!(
            server
                .state
                .requests
                .lock()
                .expect("request list")
                .iter()
                .filter(|request| request.contains("range: bytes="))
                .count()
                >= 4
        );

        server.stop().await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn model_files_download_concurrently_under_the_shared_connection_cap() {
        let bytes = fixture_bytes(LARGE_FILE_SIZE);
        let digest = digest(&bytes);
        let server = TestServer::start(
            bytes.clone(),
            TestServerOptions {
                header_delay: Duration::from_millis(80),
                ..TestServerOptions::default()
            },
        )
        .await;
        let root = temp_dir("model-files");
        let model = ModelSpec {
            id: "two-files".to_owned(),
            kind: ModelKind::Llm,
            name: "Two fixture files".to_owned(),
            description_ru: String::new(),
            description_en: String::new(),
            languages: Vec::new(),
            vendor: "Fixture".to_owned(),
            family: "Fixture".to_owned(),
            tags: Vec::new(),
            license: "MIT".to_owned(),
            license_url: None,
            license_custom: false,
            homepage: String::new(),
            quality_rank: 1,
            default_tiers: Vec::new(),
            files: vec![
                spec_for(
                    "first.bin",
                    bytes.len() as u64,
                    digest.clone(),
                    vec![server.url_for("first")],
                ),
                spec_for(
                    "second.bin",
                    bytes.len() as u64,
                    digest,
                    vec![server.url_for("second")],
                ),
            ],
            requires: Requirements::default(),
            cost: ModelCost::default(),
            engine: None,
            engine_files: None,
            llm: None,
            todo: None,
        };
        let downloader = Downloader::with_options(&root, DownloadOptions { connections: 3 })
            .expect("downloader");
        let model_dir = downloader
            .download_model(&model, &DownloadControl::default())
            .await
            .expect("model download");
        assert_eq!(
            tokio::fs::read(model_dir.join("first.bin"))
                .await
                .expect("first file"),
            bytes
        );
        assert_eq!(
            tokio::fs::read(model_dir.join("second.bin"))
                .await
                .expect("second file"),
            bytes
        );
        assert!(server.state.peak_distinct_paths.load(Ordering::Acquire) >= 2);
        assert!(server.state.peak_active.load(Ordering::Acquire) <= 3);

        server.stop().await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn paused_segmented_download_resumes_from_saved_chunk_offsets() {
        let bytes = fixture_bytes(LARGE_FILE_SIZE);
        let server = TestServer::start(
            bytes.clone(),
            TestServerOptions {
                body_delay: Duration::from_millis(2),
                ..TestServerOptions::default()
            },
        )
        .await;
        let root = temp_dir("pause");
        let install = root.join("models/test");
        let control = DownloadControl::default();
        let callback_control = control.clone();
        let pause_requested = Arc::new(AtomicBool::new(false));
        let requested_flag = pause_requested.clone();
        let (paused_sender, mut paused_receiver) = mpsc::unbounded_channel();
        let progress: ProgressCallback = Arc::new(move |event| {
            if let EngineEvent::Download {
                done_bytes, state, ..
            } = event
            {
                if matches!(state, DownloadState::Running)
                    && done_bytes >= 1024 * 1024
                    && !requested_flag.swap(true, Ordering::AcqRel)
                {
                    callback_control.pause();
                }
                if matches!(state, DownloadState::Paused) {
                    let _ = paused_sender.send(done_bytes);
                }
            }
        });
        let downloader = Downloader::with_progress_and_options(
            &root,
            Some(progress),
            DownloadOptions { connections: 3 },
        )
        .expect("downloader");
        let spec = spec_for(
            "payload.bin",
            bytes.len() as u64,
            digest(&bytes),
            vec![server.url()],
        );
        let download_control = control.clone();
        let download = tokio::spawn(async move {
            downloader
                .download_file("test", &spec, &install, &download_control)
                .await
        });

        let paused_done = tokio::time::timeout(Duration::from_secs(10), paused_receiver.recv())
            .await
            .expect("pause event timeout")
            .expect("pause event");
        assert!(paused_done >= 1024 * 1024);
        let state_path = append_suffix(&root.join("models/test/payload.bin.part"), ".json");
        let state: serde_json::Value =
            serde_json::from_slice(&tokio::fs::read(&state_path).await.expect("part state"))
                .expect("v2 state");
        assert_eq!(state["version"], 2);
        let in_flight = state["inFlightChunks"]
            .as_array()
            .expect("in-flight chunks");
        let offset = in_flight
            .iter()
            .map(|chunk| chunk["offset"].as_u64().expect("offset"))
            .find(|offset| *offset > 0)
            .expect("saved in-flight offset");
        control.resume();

        let path = download
            .await
            .expect("download task")
            .expect("resumed download");
        assert_eq!(tokio::fs::read(path).await.expect("installed bytes"), bytes);
        assert!(
            server
                .state
                .requests
                .lock()
                .expect("request list")
                .iter()
                .any(|request| request.contains(&format!("range: bytes={offset}-")))
        );

        server.stop().await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn legacy_v1_prefix_is_migrated_to_v2_and_resumed() {
        let bytes = fixture_bytes(8192);
        let prefix = 3072usize;
        let server = TestServer::start(bytes.clone(), TestServerOptions::default()).await;
        let root = temp_dir("v1-migration");
        tokio::fs::create_dir_all(&root)
            .await
            .expect("install directory");
        let part = root.join("payload.bin.part");
        tokio::fs::write(&part, &bytes[..prefix])
            .await
            .expect("legacy partial");
        let spec = spec_for(
            "payload.bin",
            bytes.len() as u64,
            digest(&bytes),
            vec![server.url()],
        );
        let state_path = append_suffix(&part, ".json");
        let legacy = serde_json::json!({
            "itemId": "legacy",
            "path": "payload.bin",
            "sha256": spec.sha256,
            "size": spec.size,
            "downloadedBytes": prefix,
            "savedAtUnix": 1
        });
        tokio::fs::write(
            &state_path,
            serde_json::to_vec(&legacy).expect("legacy json"),
        )
        .await
        .expect("legacy state");

        let control = DownloadControl::default();
        control.pause();
        let downloader = Downloader::new(&root).expect("downloader");
        let spec_for_task = spec.clone();
        let root_for_task = root.clone();
        let control_for_task = control.clone();
        let download = tokio::spawn(async move {
            downloader
                .download_file("legacy", &spec_for_task, &root_for_task, &control_for_task)
                .await
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let migrated: serde_json::Value =
            serde_json::from_slice(&tokio::fs::read(&state_path).await.expect("migrated state"))
                .expect("v2 state");
        assert_eq!(migrated["version"], 2);
        assert_eq!(migrated["inFlightChunks"][0]["offset"], prefix as u64);
        control.resume();

        let path = download
            .await
            .expect("download task")
            .expect("resumed download");
        assert_eq!(tokio::fs::read(path).await.expect("installed bytes"), bytes);
        assert!(
            server
                .state
                .requests
                .lock()
                .expect("request list")
                .iter()
                .any(|request| request.contains(&format!("range: bytes={prefix}-")))
        );

        server.stop().await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_403_on_one_range_re_resolves_the_download_url() {
        let bytes = fixture_bytes(LARGE_FILE_SIZE);
        let server = TestServer::start(
            bytes.clone(),
            TestServerOptions {
                forbidden_once: true,
                ..TestServerOptions::default()
            },
        )
        .await;
        let root = temp_dir("expired-signature");
        let downloader = Downloader::with_options(&root, DownloadOptions { connections: 3 })
            .expect("downloader");
        let spec = spec_for(
            "payload.bin",
            bytes.len() as u64,
            digest(&bytes),
            vec![server.url()],
        );

        let path = downloader
            .download_file("test", &spec, &root, &DownloadControl::default())
            .await
            .expect("download after URL refresh");
        assert_eq!(tokio::fs::read(path).await.expect("installed bytes"), bytes);
        assert_eq!(server.state.forbidden_responses.load(Ordering::Acquire), 1);
        assert!(
            server
                .state
                .requests
                .lock()
                .expect("request list")
                .iter()
                .filter(|request| request.contains("range: bytes=0-0"))
                .count()
                >= 2
        );

        server.stop().await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn repeated_range_failures_switch_to_the_next_mirror() {
        let bytes = fixture_bytes(LARGE_FILE_SIZE);
        let bad_mirror = TestServer::start(
            bytes.clone(),
            TestServerOptions {
                fail_ranges: true,
                ..TestServerOptions::default()
            },
        )
        .await;
        let good_mirror = TestServer::start(bytes.clone(), TestServerOptions::default()).await;
        let root = temp_dir("mirror-switch");
        let downloader = Downloader::with_options(&root, DownloadOptions { connections: 3 })
            .expect("downloader");
        let spec = spec_for(
            "payload.bin",
            bytes.len() as u64,
            digest(&bytes),
            vec![bad_mirror.url(), good_mirror.url()],
        );

        let path = downloader
            .download_file("test", &spec, &root, &DownloadControl::default())
            .await
            .expect("download from second mirror");
        assert_eq!(tokio::fs::read(path).await.expect("installed bytes"), bytes);
        assert!(bad_mirror.state.failed_ranges.load(Ordering::Acquire) >= 3);
        assert!(
            good_mirror
                .state
                .requests
                .lock()
                .expect("request list")
                .iter()
                .any(|request| request.contains("range: bytes="))
        );

        bad_mirror.stop().await;
        good_mirror.stop().await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn range_unsupported_server_uses_the_single_stream_fallback() {
        let bytes = fixture_bytes(LARGE_FILE_SIZE);
        let server = TestServer::start(
            bytes.clone(),
            TestServerOptions {
                supports_ranges: false,
                ..TestServerOptions::default()
            },
        )
        .await;
        let root = temp_dir("no-ranges");
        let downloader = Downloader::with_options(&root, DownloadOptions { connections: 3 })
            .expect("downloader");
        let spec = spec_for(
            "payload.bin",
            bytes.len() as u64,
            digest(&bytes),
            vec![server.url()],
        );

        let path = downloader
            .download_file("test", &spec, &root, &DownloadControl::default())
            .await
            .expect("single-stream fallback");
        assert_eq!(tokio::fs::read(path).await.expect("installed bytes"), bytes);
        let (probe_found, full_request_found) = {
            let requests = server.state.requests.lock().expect("request list");
            (
                requests
                    .first()
                    .is_some_and(|request| request.contains("range: bytes=0-0")),
                requests
                    .iter()
                    .any(|request| !request.contains("range: bytes=")),
            )
        };
        assert!(probe_found);
        assert!(full_request_found);
        server.stop().await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn hash_failure_removes_partial_data_and_state() {
        let bytes = b"actual payload".to_vec();
        let server = TestServer::start(bytes.clone(), TestServerOptions::default()).await;
        let root = temp_dir("bad-hash");
        let failure_resumable = Arc::new(Mutex::new(None));
        let callback_failure = failure_resumable.clone();
        let progress: ProgressCallback = Arc::new(move |event| {
            if let EngineEvent::Download {
                state: DownloadState::Failed { resumable, .. },
                ..
            } = event
            {
                *callback_failure.lock().expect("failure state") = Some(resumable);
            }
        });
        let downloader = Downloader::with_progress(&root, Some(progress)).expect("downloader");
        let spec = spec_for(
            "payload.bin",
            bytes.len() as u64,
            "0".repeat(64),
            vec![server.url()],
        );
        let result = downloader
            .download_file("test", &spec, &root, &DownloadControl::default())
            .await;
        assert!(matches!(result, Err(crate::Error::Integrity(_))));
        assert_eq!(
            *failure_resumable.lock().expect("failure state"),
            Some(false)
        );
        assert!(!root.join("payload.bin.part").exists());
        assert!(!root.join("payload.bin.part.json").exists());

        server.stop().await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn catalog_size_mismatch_is_reported_as_non_resumable() {
        let bytes = fixture_bytes(LARGE_FILE_SIZE);
        let server = TestServer::start(bytes.clone(), TestServerOptions::default()).await;
        let root = temp_dir("bad-size");
        let failure_resumable = Arc::new(Mutex::new(None));
        let callback_failure = failure_resumable.clone();
        let progress: ProgressCallback = Arc::new(move |event| {
            if let EngineEvent::Download {
                state: DownloadState::Failed { resumable, .. },
                ..
            } = event
            {
                *callback_failure.lock().expect("failure state") = Some(resumable);
            }
        });
        let downloader = Downloader::with_progress_and_options(
            &root,
            Some(progress),
            DownloadOptions { connections: 3 },
        )
        .expect("downloader");
        let spec = spec_for(
            "payload.bin",
            bytes.len() as u64 + 1,
            digest(&bytes),
            vec![server.url()],
        );

        let result = downloader
            .download_file("test", &spec, &root, &DownloadControl::default())
            .await;
        assert!(matches!(result, Err(crate::Error::Integrity(_))));
        assert_eq!(
            *failure_resumable.lock().expect("failure state"),
            Some(false)
        );
        assert!(!root.join("payload.bin.part").exists());
        assert!(!root.join("payload.bin.part.json").exists());

        server.stop().await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn cancellation_removes_state_and_partial_file() {
        let bytes = fixture_bytes(4 * 1024 * 1024);
        let server = TestServer::start(
            bytes.clone(),
            TestServerOptions {
                body_delay: Duration::from_millis(3),
                ..TestServerOptions::default()
            },
        )
        .await;
        let root = temp_dir("cancel");
        let control = DownloadControl::default();
        let callback_control = control.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_once = cancelled.clone();
        let progress: ProgressCallback = Arc::new(move |event| {
            if let EngineEvent::Download {
                done_bytes,
                state: DownloadState::Running,
                ..
            } = event
                && done_bytes > 0
                && !cancelled_once.swap(true, Ordering::AcqRel)
            {
                callback_control.cancel();
            }
        });
        let downloader = Downloader::with_progress(&root, Some(progress)).expect("downloader");
        let spec = spec_for(
            "payload.bin",
            bytes.len() as u64,
            digest(&bytes),
            vec![server.url()],
        );
        let result = downloader
            .download_file("test", &spec, &root, &control)
            .await;
        assert!(matches!(result, Err(crate::Error::Cancelled)));
        assert!(!root.join("payload.bin.part").exists());
        assert!(!root.join("payload.bin.part.json").exists());

        server.stop().await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[derive(Clone, Copy)]
    struct TestServerOptions {
        supports_ranges: bool,
        fail_ranges: bool,
        forbidden_once: bool,
        header_delay: Duration,
        body_delay: Duration,
    }

    impl Default for TestServerOptions {
        fn default() -> Self {
            Self {
                supports_ranges: true,
                fail_ranges: false,
                forbidden_once: false,
                header_delay: Duration::ZERO,
                body_delay: Duration::ZERO,
            }
        }
    }

    struct TestServerState {
        bytes: Arc<Vec<u8>>,
        options: TestServerOptions,
        requests: Mutex<Vec<String>>,
        active_paths: Mutex<HashSet<String>>,
        active: AtomicUsize,
        peak_active: AtomicUsize,
        peak_distinct_paths: AtomicUsize,
        forbidden_responses: AtomicUsize,
        failed_ranges: AtomicUsize,
        forbidden_sent: AtomicBool,
    }

    struct ActiveConnection(Arc<TestServerState>);

    impl Drop for ActiveConnection {
        fn drop(&mut self) {
            self.0.active.fetch_sub(1, Ordering::AcqRel);
        }
    }

    struct ActivePath(Arc<TestServerState>, String);

    impl Drop for ActivePath {
        fn drop(&mut self) {
            self.0
                .active_paths
                .lock()
                .expect("active request paths")
                .remove(&self.1);
        }
    }

    struct TestServer {
        address: std::net::SocketAddr,
        state: Arc<TestServerState>,
        shutdown: CancellationToken,
        task: tokio::task::JoinHandle<()>,
    }

    impl TestServer {
        async fn start(bytes: Vec<u8>, options: TestServerOptions) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind test server");
            let address = listener.local_addr().expect("test server address");
            let shutdown = CancellationToken::new();
            let state = Arc::new(TestServerState {
                bytes: Arc::new(bytes),
                options,
                requests: Mutex::new(Vec::new()),
                active_paths: Mutex::new(HashSet::new()),
                active: AtomicUsize::new(0),
                peak_active: AtomicUsize::new(0),
                peak_distinct_paths: AtomicUsize::new(0),
                forbidden_responses: AtomicUsize::new(0),
                failed_ranges: AtomicUsize::new(0),
                forbidden_sent: AtomicBool::new(false),
            });
            let listener_state = state.clone();
            let listener_shutdown = shutdown.clone();
            let task = tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = listener_shutdown.cancelled() => break,
                        accepted = listener.accept() => {
                            let Ok((stream, _)) = accepted else {
                                break;
                            };
                            let state = listener_state.clone();
                            tokio::spawn(async move {
                                let active = state.active.fetch_add(1, Ordering::AcqRel) + 1;
                                state.peak_active.fetch_max(active, Ordering::AcqRel);
                                let _guard = ActiveConnection(state.clone());
                                let _ = serve_connection(stream, state).await;
                            });
                        }
                    }
                }
            });
            Self {
                address,
                state,
                shutdown,
                task,
            }
        }

        fn url(&self) -> String {
            self.url_for("payload")
        }

        fn url_for(&self, path: &str) -> String {
            format!("http://{}/{}", self.address, path.trim_start_matches('/'))
        }

        async fn stop(self) {
            self.shutdown.cancel();
            self.task.await.expect("test server listener");
        }
    }

    async fn serve_connection(
        mut stream: TcpStream,
        state: Arc<TestServerState>,
    ) -> std::io::Result<()> {
        let request = read_headers(&mut stream).await?;
        let request_path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
        let distinct_paths = {
            let mut active_paths = state.active_paths.lock().expect("active request paths");
            active_paths.insert(request_path.clone());
            active_paths.len()
        };
        state
            .peak_distinct_paths
            .fetch_max(distinct_paths, Ordering::AcqRel);
        let _path_guard = ActivePath(state.clone(), request_path);
        state
            .requests
            .lock()
            .expect("request list")
            .push(request.clone());
        let range = request
            .lines()
            .find_map(|line| line.strip_prefix("range: "))
            .and_then(|value| value.strip_prefix("bytes="))
            .and_then(parse_range);
        let is_probe = range.is_some_and(|(start, end)| start == 0 && end == 0);

        if !state.options.header_delay.is_zero() {
            tokio::time::sleep(state.options.header_delay).await;
        }
        if state.options.forbidden_once
            && range.is_some()
            && !is_probe
            && !state.forbidden_sent.swap(true, Ordering::AcqRel)
        {
            state.forbidden_responses.fetch_add(1, Ordering::AcqRel);
            stream
                .write_all(
                    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await?;
            return Ok(());
        }
        if state.options.fail_ranges && range.is_some() && !is_probe {
            state.failed_ranges.fetch_add(1, Ordering::AcqRel);
            stream
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await?;
            return Ok(());
        }

        if !state.options.supports_ranges && range.is_some() {
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                state.bytes.len()
            );
            stream.write_all(headers.as_bytes()).await?;
            if is_probe {
                return Ok(());
            }
            return write_body(&mut stream, &state.bytes, state.options.body_delay).await;
        }

        if let Some((start, end_inclusive)) = range {
            if end_inclusive < start || start >= state.bytes.len() as u64 {
                stream
                    .write_all(b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await?;
                return Ok(());
            }
            let end = end_inclusive
                .saturating_add(1)
                .min(state.bytes.len() as u64);
            let body = &state.bytes[start as usize..end as usize];
            let headers = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                body.len(),
                start,
                end - 1,
                state.bytes.len()
            );
            stream.write_all(headers.as_bytes()).await?;
            return write_body(&mut stream, body, state.options.body_delay).await;
        }

        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            state.bytes.len()
        );
        stream.write_all(headers.as_bytes()).await?;
        write_body(&mut stream, &state.bytes, state.options.body_delay).await
    }

    async fn write_body(
        stream: &mut TcpStream,
        bytes: &[u8],
        delay: Duration,
    ) -> std::io::Result<()> {
        for chunk in bytes.chunks(64 * 1024) {
            stream.write_all(chunk).await?;
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }
        stream.flush().await
    }

    async fn read_headers(stream: &mut TcpStream) -> std::io::Result<String> {
        let mut request = Vec::new();
        let mut buffer = [0u8; 512];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).await?;
            if count == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "connection closed before request headers arrived",
                ));
            }
            request.extend_from_slice(&buffer[..count]);
        }
        Ok(String::from_utf8_lossy(&request).to_ascii_lowercase())
    }

    fn parse_range(value: &str) -> Option<(u64, u64)> {
        let (start, end) = value.split_once('-')?;
        Some((start.parse().ok()?, end.parse().ok()?))
    }

    fn spec_for(path: &str, size: u64, sha256: String, urls: Vec<String>) -> ModelFile {
        ModelFile {
            path: path.to_owned(),
            size,
            sha256,
            urls,
            hf_repo: None,
            hf_path: None,
            github_release_api: None,
            github_asset: None,
            optional: false,
        }
    }

    fn fixture_bytes(size: usize) -> Vec<u8> {
        (0..size)
            .map(|index| ((index.wrapping_mul(37) + index / 17) % 251) as u8)
            .collect()
    }

    fn digest(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
        let mut value = path.as_os_str().to_os_string();
        value.push(suffix);
        PathBuf::from(value)
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("yadacall-hub-{tag}-{}-{stamp}", std::process::id()))
    }
}
