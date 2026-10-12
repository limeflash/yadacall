//! Resumable, hash-verified asynchronous downloads with mirror fallback.

use std::{
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{io::AsyncWriteExt, sync::Notify};
use tokio_util::sync::CancellationToken;

use crate::{
    Error, Result,
    catalog::{ModelFile, ModelSpec},
};
use yc_core::events::{DownloadState, EngineEvent};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(125);
const SLOW_WINDOW: Duration = Duration::from_secs(15);
const SLOW_BYTES_PER_SECOND: u64 = 200 * 1024;
const MIN_DISK_HEADROOM: u64 = 128 * 1024 * 1024;

/// Download progress callback. Events are throttled to approximately eight per second.
pub type ProgressCallback = Arc<dyn Fn(EngineEvent) + Send + Sync>;

/// Controls a download session. Pausing preserves partial data, resuming continues the same file,
/// and cancelling leaves a resumable `.part` file on disk.
#[derive(Clone, Default)]
pub struct DownloadControl {
    cancel: CancellationToken,
    pause: Arc<PauseState>,
}

#[derive(Default)]
struct PauseState {
    paused: AtomicBool,
    changed: Notify,
}

impl DownloadControl {
    /// Pause after the next received chunk and preserve the partial file.
    pub fn pause(&self) {
        self.pause.paused.store(true, Ordering::Release);
        self.pause.changed.notify_waiters();
    }

    /// Resume a paused download.
    pub fn resume(&self) {
        self.pause.paused.store(false, Ordering::Release);
        self.pause.changed.notify_waiters();
    }

    /// Cancel the download and leave its partial file available for a later resume.
    pub fn cancel(&self) {
        self.cancel.cancel();
        self.pause.changed.notify_waiters();
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    async fn wait_until_resumed(&self) -> Result<bool> {
        let mut paused = false;
        loop {
            if self.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if !self.pause.paused.load(Ordering::Acquire) {
                return Ok(paused);
            }
            paused = true;
            let notified = self.pause.changed.notified();
            if !self.pause.paused.load(Ordering::Acquire) {
                continue;
            }
            tokio::select! {
                _ = self.cancel.cancelled() => return Err(Error::Cancelled),
                _ = notified => {}
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
        let mut bytes_to_download = 0u64;
        for file in model.files.iter().filter(|file| !file.optional) {
            let target = model_dir.join(safe_relative_path(&file.path)?);
            if self.file_is_installed(&target, file, true).await? {
                continue;
            }
            let part = append_suffix(&target, ".part");
            let done = tokio::fs::metadata(part)
                .await
                .map(|metadata| metadata.len())
                .unwrap_or_default();
            bytes_to_download = bytes_to_download.saturating_add(file.size.saturating_sub(done));
        }
        self.preflight_space(&model_dir, bytes_to_download)?;
        for file in model.files.iter().filter(|file| !file.optional) {
            self.download_file(&model.id, file, &model_dir, control)
                .await?;
        }
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
        let relative = safe_relative_path(&spec.path)?;
        let target = install_dir.join(relative);
        if self.file_is_installed(&target, spec, true).await? {
            return Ok(target);
        }
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let part = append_suffix(&target, ".part");
        let state_path = append_suffix(&part, ".json");
        let mut done = tokio::fs::metadata(&part)
            .await
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        if !state_matches(&state_path, item_id, spec).await {
            if done > 0 {
                tokio::fs::remove_file(&part).await?;
            }
            let _ = tokio::fs::remove_file(&state_path).await;
            done = 0;
        }
        self.check_disk_space(&target, spec.size.saturating_sub(done))?;
        if spec.urls.is_empty() {
            return Err(Error::Invalid(format!(
                "file `{}` has no download URL",
                spec.path
            )));
        }
        self.write_state(&state_path, item_id, spec, done).await?;

        let mut last_error = Error::Network("no URL could be reached".to_owned());
        for url in &spec.urls {
            for attempt in 0..3u32 {
                match self
                    .download_url(
                        item_id,
                        spec,
                        url,
                        PartialDownloadPaths {
                            part: &part,
                            state: &state_path,
                        },
                        done,
                        control,
                    )
                    .await
                {
                    Ok(new_done) => {
                        done = new_done;
                        if done < spec.size {
                            last_error = Error::Network(format!(
                                "incomplete download of `{}` ({done}/{} bytes)",
                                spec.path, spec.size
                            ));
                            break;
                        }
                        self.emit(item_id, done, spec.size, 0, DownloadState::Verifying);
                        let actual_hash = hash_file_async(&part).await?;
                        if actual_hash != spec.sha256.to_ascii_lowercase() {
                            let _ = tokio::fs::remove_file(&part).await;
                            let _ = tokio::fs::remove_file(&state_path).await;
                            self.emit(
                                item_id,
                                0,
                                spec.size,
                                0,
                                DownloadState::Failed {
                                    message: "SHA-256 mismatch".to_owned(),
                                    resumable: false,
                                },
                            );
                            return Err(Error::Integrity(target));
                        }
                        atomic_install(&part, &target).await?;
                        let _ = tokio::fs::remove_file(&state_path).await;
                        self.emit(item_id, spec.size, spec.size, 0, DownloadState::Done);
                        return Ok(target);
                    }
                    Err(Error::Cancelled) => {
                        self.emit(
                            item_id,
                            done,
                            spec.size,
                            0,
                            DownloadState::Failed {
                                message: "cancelled; partial file is resumable".to_owned(),
                                resumable: true,
                            },
                        );
                        return Err(Error::Cancelled);
                    }
                    Err(error) => {
                        let slow_mirror = error
                            .to_string()
                            .starts_with("network error: mirror too slow");
                        last_error = error;
                        if !slow_mirror && attempt < 2 {
                            let delay = Duration::from_millis(300 * (1 << attempt));
                            tokio::select! {
                                _ = control.cancel.cancelled() => return Err(Error::Cancelled),
                                _ = tokio::time::sleep(delay) => {}
                            }
                        }
                        if slow_mirror {
                            break;
                        }
                    }
                }
                done = tokio::fs::metadata(&part)
                    .await
                    .map(|metadata| metadata.len())
                    .unwrap_or_default();
                self.write_state(&state_path, item_id, spec, done).await?;
            }
        }
        self.emit(
            item_id,
            done,
            spec.size,
            0,
            DownloadState::Failed {
                message: last_error.to_string(),
                resumable: done > 0,
            },
        );
        Err(last_error)
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

    async fn download_url(
        &self,
        item_id: &str,
        spec: &ModelFile,
        url: &str,
        paths: PartialDownloadPaths<'_>,
        mut done: u64,
        control: &DownloadControl,
    ) -> Result<u64> {
        if control.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let mut request = self.client.get(url);
        if done > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={done}-"));
        }
        let response = request
            .send()
            .await
            .map_err(network_error)?
            .error_for_status()
            .map_err(network_error)?;
        if done > 0 && response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            done = 0;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(done > 0)
            .truncate(done == 0)
            .open(paths.part)
            .await?;
        let mut stream = response.bytes_stream();
        let mut last_event = Instant::now() - PROGRESS_INTERVAL;
        let mut window_start = Instant::now();
        let mut window_bytes = 0u64;
        let mut last_speed = 0u64;

        loop {
            if control.pause.paused.load(Ordering::Acquire) {
                self.emit(item_id, done, spec.size, last_speed, DownloadState::Paused);
                control.wait_until_resumed().await?;
                last_event = Instant::now() - PROGRESS_INTERVAL;
                window_start = Instant::now();
                window_bytes = 0;
                self.emit(item_id, done, spec.size, last_speed, DownloadState::Running);
            }
            let next = tokio::select! {
                _ = control.cancel.cancelled() => return Err(Error::Cancelled),
                _ = control.pause.changed.notified(), if control.pause.paused.load(Ordering::Acquire) => {
                    self.emit(item_id, done, spec.size, last_speed, DownloadState::Paused);
                    control.wait_until_resumed().await?;
                    last_event = Instant::now() - PROGRESS_INTERVAL;
                    window_start = Instant::now();
                    window_bytes = 0;
                    self.emit(item_id, done, spec.size, last_speed, DownloadState::Running);
                    continue;
                }
                chunk = stream.next() => chunk,
            };
            let Some(chunk) = next else {
                break;
            };
            let chunk = chunk.map_err(network_error)?;
            if control.pause.paused.load(Ordering::Acquire) {
                self.emit(item_id, done, spec.size, last_speed, DownloadState::Paused);
            }
            if control.wait_until_resumed().await? {
                last_event = Instant::now() - PROGRESS_INTERVAL;
                window_start = Instant::now();
                window_bytes = 0;
                self.emit(item_id, done, spec.size, last_speed, DownloadState::Running);
            }
            file.write_all(&chunk).await?;
            done = done.saturating_add(chunk.len() as u64);
            window_bytes = window_bytes.saturating_add(chunk.len() as u64);
            self.write_state(paths.state, item_id, spec, done).await?;
            let now = Instant::now();
            if now.duration_since(last_event) >= PROGRESS_INTERVAL {
                let elapsed = now.duration_since(window_start).as_secs_f64().max(0.001);
                last_speed = (window_bytes as f64 / elapsed) as u64;
                self.emit(item_id, done, spec.size, last_speed, DownloadState::Running);
                last_event = now;
            }
            if now.duration_since(window_start) >= SLOW_WINDOW {
                let elapsed = now.duration_since(window_start).as_secs_f64().max(0.001);
                let speed = window_bytes as f64 / elapsed;
                if speed < SLOW_BYTES_PER_SECOND as f64 {
                    return Err(Error::Network(format!(
                        "mirror too slow ({:.0} KB/s), switching to next URL",
                        speed / 1024.0
                    )));
                }
                window_start = now;
                window_bytes = 0;
            }
        }
        file.flush().await?;
        file.sync_all().await?;
        Ok(done)
    }

    async fn write_state(
        &self,
        state_path: &Path,
        item_id: &str,
        spec: &ModelFile,
        done: u64,
    ) -> Result<()> {
        let Some(parent) = state_path.parent() else {
            return Err(Error::Invalid(
                "download state needs a parent directory".to_owned(),
            ));
        };
        tokio::fs::create_dir_all(parent).await?;
        let state = PartState {
            item_id: item_id.to_owned(),
            path: spec.path.clone(),
            sha256: spec.sha256.clone(),
            size: spec.size,
            downloaded_bytes: done,
            saved_at_unix: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        let data = serde_json::to_vec(&state)
            .map_err(|error| Error::Storage(format!("serialize download state: {error}")))?;
        let temp = append_suffix(state_path, ".tmp");
        tokio::fs::write(&temp, data).await?;
        atomic_install(&temp, state_path).await?;
        Ok(())
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

    fn emit(&self, item_id: &str, done: u64, total: u64, bytes_per_sec: u64, state: DownloadState) {
        if let Some(callback) = &self.progress {
            callback(EngineEvent::Download {
                item: item_id.to_owned(),
                done_bytes: done,
                total_bytes: total,
                bytes_per_sec,
                state,
            });
        }
    }
}

#[derive(Clone, Copy)]
struct PartialDownloadPaths<'a> {
    part: &'a Path,
    state: &'a Path,
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

async fn state_matches(path: &Path, item_id: &str, spec: &ModelFile) -> bool {
    let Ok(bytes) = tokio::fs::read(path).await else {
        return false;
    };
    let Ok(state) = serde_json::from_slice::<PartState>(&bytes) else {
        return false;
    };
    state.item_id == item_id
        && state.path == spec.path
        && state.sha256 == spec.sha256
        && state.size == spec.size
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartState {
    item_id: String,
    path: String,
    sha256: String,
    size: u64,
    downloaded_bytes: u64,
    saved_at_unix: u64,
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::Arc, time::Duration};

    use sha2::{Digest, Sha256};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };

    use super::{DownloadControl, Downloader, ModelFile, ProgressCallback, model_storage_dir};
    use yc_core::events::{DownloadState, EngineEvent};

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
    async fn resumes_from_existing_partial_file_after_cancel() {
        let bytes = (0..4096)
            .map(|value| (value % 251) as u8)
            .collect::<Vec<_>>();
        let split = bytes.len() / 2;
        let expected_hash = hex::encode(Sha256::digest(&bytes));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_bytes = bytes.clone();
        let server = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            let _ = read_headers(&mut first).await;
            first
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        server_bytes.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            first.write_all(&server_bytes[..split]).await.unwrap();
            first.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(250)).await;
            drop(first);

            let (mut second, _) = listener.accept().await.unwrap();
            let request = read_headers(&mut second).await;
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains(&format!("range: bytes={split}-"))
            );
            let tail = &server_bytes[split..];
            second
                .write_all(
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                        tail.len(), split, server_bytes.len() - 1, server_bytes.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            second.write_all(tail).await.unwrap();
            second.flush().await.unwrap();
        });

        let root = temp_dir("resume");
        let first_control = DownloadControl::default();
        let callback_control = first_control.clone();
        let progress: ProgressCallback = Arc::new(move |event| {
            if matches!(
                event,
                EngineEvent::Download {
                    state: DownloadState::Running,
                    ..
                }
            ) {
                callback_control.cancel();
            }
        });
        let downloader = Downloader::with_progress(&root, Some(progress)).unwrap();
        let spec = ModelFile {
            path: "payload.bin".to_owned(),
            size: bytes.len() as u64,
            sha256: expected_hash,
            urls: vec![format!("http://{address}/payload")],
            hf_repo: None,
            github_release_api: None,
            github_asset: None,
            optional: false,
        };
        let install = root.join("models/test");
        let first = downloader
            .download_file("test", &spec, &install, &first_control)
            .await;
        assert!(matches!(first, Err(crate::Error::Cancelled)));

        let path = downloader
            .download_file("test", &spec, &install, &DownloadControl::default())
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(&path).await.unwrap(), bytes);
        server.await.unwrap();
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn rejects_a_download_with_the_wrong_hash() {
        let bytes = b"actual payload".to_vec();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_bytes = bytes.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = read_headers(&mut stream).await;
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        server_bytes.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(&server_bytes).await.unwrap();
            stream.flush().await.unwrap();
        });
        let root = temp_dir("bad-hash");
        let downloader = Downloader::new(&root).unwrap();
        let spec = ModelFile {
            path: "payload.bin".to_owned(),
            size: bytes.len() as u64,
            sha256: "0".repeat(64),
            urls: vec![format!("http://{address}/payload")],
            hf_repo: None,
            github_release_api: None,
            github_asset: None,
            optional: false,
        };
        let result = downloader
            .download_file(
                "test",
                &spec,
                &root.join("models/test"),
                &DownloadControl::default(),
            )
            .await;
        assert!(matches!(result, Err(crate::Error::Integrity(_))));
        server.await.unwrap();
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    async fn read_headers(stream: &mut TcpStream) -> String {
        let mut request = Vec::new();
        let mut buffer = [0u8; 512];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).await.unwrap();
            assert!(
                count > 0,
                "connection closed before request headers arrived"
            );
            request.extend_from_slice(&buffer[..count]);
        }
        String::from_utf8_lossy(&request).to_ascii_lowercase()
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("yadacall-hub-{tag}-{}-{stamp}", std::process::id()))
    }
}
