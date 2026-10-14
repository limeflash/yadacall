//! Sidecar supervisor for a downloaded `llama-server` runtime.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use rand::RngCore;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    sync::{Mutex as AsyncMutex, OwnedRwLockReadGuard, RwLock, watch},
    task::JoinHandle,
};
use yc_core::{Error, Result};

use crate::{openai::OpenAiCompatibleProvider, types::*};

/// Runtime/model paths and resource policy for a local llama-server process.
#[derive(Debug, Clone)]
pub struct LocalServerConfig {
    /// Extracted runtime pack containing `llama-server`.
    pub runtime_dir: PathBuf,
    /// GGUF model file downloaded by `yc-hub`.
    pub model_path: PathBuf,
    /// KV-cache context size. Defaults to 32768 tokens.
    pub context_size: u32,
    /// Pass `--reasoning off` when supported by this llama-server build.
    pub reasoning_off: bool,
    /// Defaults applied to local completion requests when a prompt has no override.
    pub sampling: LocalSampling,
    /// Time to wait for `/health` after process start.
    pub startup_timeout: Duration,
    /// Stop the server after no provider calls for this duration. Defaults to five minutes.
    pub idle_timeout: Duration,
}

impl LocalServerConfig {
    /// Create a local server configuration with the documented context and idle defaults.
    pub fn new(runtime_dir: impl Into<PathBuf>, model_path: impl Into<PathBuf>) -> Self {
        Self {
            runtime_dir: runtime_dir.into(),
            model_path: model_path.into(),
            context_size: 32_768,
            reasoning_off: false,
            sampling: LocalSampling::default(),
            startup_timeout: Duration::from_secs(120),
            idle_timeout: Duration::from_secs(5 * 60),
        }
    }
}

/// Optional sampling defaults applied only by the local llama-server provider.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LocalSampling {
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// Nucleus sampling probability.
    pub top_p: Option<f32>,
    /// Top-k sampling limit.
    pub top_k: Option<u32>,
    /// Minimum probability cutoff.
    pub min_p: Option<f32>,
    /// Presence penalty.
    pub presence_penalty: Option<f32>,
}

/// Startup state suitable for showing runtime progress in the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalServerProgress {
    /// Runtime binary and model are being started.
    Starting,
    /// Process is running but the health endpoint is not ready yet.
    Loading,
    /// The server is accepting requests.
    Ready { port: u16 },
}

struct Inner {
    config: LocalServerConfig,
    executable: PathBuf,
    port: u16,
    api_key: String,
    child: AsyncMutex<Option<Child>>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    last_activity: Mutex<Instant>,
    in_flight: AtomicUsize,
    request_gate: Arc<RwLock<()>>,
    stop: watch::Sender<bool>,
    #[cfg(windows)]
    job: Mutex<Option<std::os::windows::io::OwnedHandle>>,
}

/// Running, supervised llama-server sidecar.
pub struct LocalServer {
    inner: Arc<Inner>,
    monitor: Option<JoinHandle<()>>,
}

impl LocalServer {
    /// Start the server and wait for `/health` to become ready.
    pub async fn start(
        config: LocalServerConfig,
        progress: impl Fn(LocalServerProgress) + Send + Sync + 'static,
    ) -> Result<Self> {
        if !config.model_path.is_file() {
            return Err(Error::ModelMissing(config.model_path.display().to_string()));
        }
        let executable = find_server(&config.runtime_dir)?;
        let help = server_help(&executable).await?;
        let port = free_port().await?;
        let mut key_bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut key_bytes);
        let api_key = key_bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let (stop, _) = watch::channel(false);
        let inner = Arc::new(Inner {
            config,
            executable,
            port,
            api_key,
            child: AsyncMutex::new(None),
            stderr: Arc::new(Mutex::new(VecDeque::with_capacity(80))),
            last_activity: Mutex::new(Instant::now()),
            in_flight: AtomicUsize::new(0),
            request_gate: Arc::new(RwLock::new(())),
            stop,
            #[cfg(windows)]
            job: Mutex::new(None),
        });

        progress(LocalServerProgress::Starting);
        let child = spawn_server(&inner, &help).await?;
        *inner.child.lock().await = Some(child);
        wait_until_healthy(&inner, &progress).await?;
        let monitor = tokio::spawn(supervise(inner.clone(), help));
        Ok(Self {
            inner,
            monitor: Some(monitor),
        })
    }

    /// Get a provider bound to the random localhost endpoint.
    pub fn provider(&self) -> Result<Box<dyn ChatProvider>> {
        let provider = OpenAiCompatibleProvider::new(
            format!("http://127.0.0.1:{}/v1", self.inner.port),
            Some(self.inner.api_key.clone()),
            "local",
            model_label(&self.inner.config.model_path),
        )?;
        Ok(Box::new(ActivityProvider {
            inner: self.inner.clone(),
            provider,
            sampling: self.inner.config.sampling,
        }))
    }

    /// Local OpenAI-compatible base URL (excluding `/v1`).
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.inner.port)
    }

    /// Last 80 stderr lines, useful when startup or inference fails.
    pub fn stderr_tail(&self) -> Vec<String> {
        stderr_tail(&self.inner.stderr)
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        let _ = self.inner.stop.send(true);
        self.monitor.take();
    }
}

struct ActivityProvider {
    inner: Arc<Inner>,
    provider: OpenAiCompatibleProvider,
    sampling: LocalSampling,
}

#[async_trait::async_trait]
impl ChatProvider for ActivityProvider {
    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse> {
        let _guard = RequestGuard::new(self.inner.clone()).await;
        self.provider
            .complete(with_local_sampling(request, self.sampling))
            .await
    }

    async fn stream(&self, request: ChatRequest) -> Result<ChatStream> {
        let guard = RequestGuard::new(self.inner.clone()).await;
        let stream = self
            .provider
            .stream(with_local_sampling(request, self.sampling))
            .await?;
        Ok(track_stream(stream, guard))
    }

    fn provider_id(&self) -> &str {
        self.provider.provider_id()
    }
    fn model(&self) -> &str {
        self.provider.model()
    }

    async fn count_tokens(&self, text: &str) -> Result<Option<usize>> {
        let _guard = RequestGuard::new(self.inner.clone()).await;
        self.provider.count_tokens(text).await
    }
}

struct RequestGuard {
    inner: Arc<Inner>,
    _gate: OwnedRwLockReadGuard<()>,
}

impl RequestGuard {
    async fn new(inner: Arc<Inner>) -> Self {
        let gate = inner.request_gate.clone().read_owned().await;
        inner.in_flight.fetch_add(1, Ordering::AcqRel);
        touch(&inner);
        Self { inner, _gate: gate }
    }
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.inner.in_flight.fetch_sub(1, Ordering::AcqRel);
        touch(&self.inner);
    }
}

fn touch(inner: &Inner) {
    if let Ok(mut last_activity) = inner.last_activity.lock() {
        *last_activity = Instant::now();
    }
}

fn track_stream(mut stream: ChatStream, guard: RequestGuard) -> ChatStream {
    Box::pin(async_stream::stream! {
        let _guard = guard;
        while let Some(item) = futures_util::StreamExt::next(&mut stream).await {
            yield item;
        }
    })
}

fn with_local_sampling(mut request: ChatRequest, sampling: LocalSampling) -> ChatRequest {
    request.temperature = request.temperature.or(sampling.temperature);
    request.top_p = request.top_p.or(sampling.top_p);
    request.top_k = request.top_k.or(sampling.top_k);
    request.min_p = request.min_p.or(sampling.min_p);
    request.presence_penalty = request.presence_penalty.or(sampling.presence_penalty);
    request
}

async fn supervise(inner: Arc<Inner>, help: String) {
    let mut stop = inner.stop.subscribe();
    let mut restarted = false;
    loop {
        tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() { kill_child(&inner).await; return; }
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
        if let Some(_gate) = idle_stop_guard(&inner).await {
            tracing::debug!("stopping idle llama-server");
            kill_child(&inner).await;
            return;
        }
        let exit = {
            let mut child = inner.child.lock().await;
            match child.as_mut() {
                Some(child) => child.try_wait().ok().flatten(),
                None => return,
            }
        };
        if exit.is_some() {
            if restarted {
                tracing::error!(
                    "llama-server exited after its one allowed restart: {}",
                    stderr_string(&inner.stderr)
                );
                return;
            }
            restarted = true;
            tracing::warn!("llama-server exited; restarting once");
            match spawn_server(&inner, &help).await {
                Ok(child) => {
                    *inner.child.lock().await = Some(child);
                    if wait_until_healthy(&inner, &|_| {}).await.is_err() {
                        tracing::error!(
                            "llama-server restart failed: {}",
                            stderr_string(&inner.stderr)
                        );
                        kill_child(&inner).await;
                        return;
                    }
                }
                Err(error) => {
                    tracing::error!("llama-server restart could not start: {error}");
                    return;
                }
            }
        }
    }
}

async fn idle_stop_guard(inner: &Arc<Inner>) -> Option<tokio::sync::OwnedRwLockWriteGuard<()>> {
    let idle = inner
        .last_activity
        .lock()
        .map(|last| last.elapsed())
        .unwrap_or_default();
    if idle < inner.config.idle_timeout || inner.in_flight.load(Ordering::Acquire) != 0 {
        return None;
    }

    // Exclude a request starting between the idle check and process termination.
    let gate = inner.request_gate.clone().write_owned().await;
    let idle = inner
        .last_activity
        .lock()
        .map(|last| last.elapsed())
        .unwrap_or_default();
    if inner.in_flight.load(Ordering::Acquire) == 0 && idle >= inner.config.idle_timeout {
        Some(gate)
    } else {
        None
    }
}

async fn spawn_server(inner: &Inner, help: &str) -> Result<Child> {
    let mut command = Command::new(&inner.executable);
    command
        .arg("-m")
        .arg(&inner.config.model_path)
        .args(["--host", "127.0.0.1", "--port"])
        .arg(inner.port.to_string())
        .args(["--api-key", &inner.api_key])
        .args(["-c", &inner.config.context_size.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    add_optional_flag(
        &mut command,
        help,
        &["-ngl", "--n-gpu-layers", "--gpu-layers"],
        Some("999"),
    );
    add_optional_flag(&mut command, help, &["--jinja"], None);
    add_optional_flag(&mut command, help, &["-fa", "--flash-attn"], Some("auto"));
    add_optional_flag(&mut command, help, &["--cache-type-k"], Some("q8_0"));
    add_optional_flag(&mut command, help, &["--cache-type-v"], Some("q8_0"));
    add_optional_flag(&mut command, help, &["--parallel"], Some("1"));
    add_optional_flag(&mut command, help, &["--no-webui"], None);
    if inner.config.reasoning_off {
        add_optional_flag(&mut command, help, &["--reasoning"], Some("off"));
    }
    let mut child = command.spawn().map_err(|error| Error::Model {
        model: "llama-server".to_owned(),
        message: format!("could not spawn {}: {error}", inner.executable.display()),
    })?;
    #[cfg(windows)]
    attach_job(inner, &child)?;
    if let Some(stderr) = child.stderr.take() {
        let tail = inner.stderr.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(mut tail) = tail.lock() {
                    tail.push_back(line);
                    while tail.len() > 80 {
                        tail.pop_front();
                    }
                }
            }
        });
    }
    Ok(child)
}

fn add_optional_flag(command: &mut Command, help: &str, names: &[&str], value: Option<&str>) {
    if let Some(name) = names.iter().find(|name| help.contains(**name)) {
        command.arg(name);
        if let Some(value) = value {
            command.arg(value);
        }
    }
}

async fn server_help(executable: &Path) -> Result<String> {
    let output = Command::new(executable)
        .arg("--help")
        .output()
        .await
        .map_err(|error| Error::Model {
            model: "llama-server".to_owned(),
            message: format!("could not inspect supported command-line flags: {error}"),
        })?;
    let mut help = String::from_utf8_lossy(&output.stdout).into_owned();
    help.push_str(&String::from_utf8_lossy(&output.stderr));
    for mandatory in ["-m", "--host", "--port", "--api-key", "-c"] {
        if !help.contains(mandatory) {
            return Err(Error::Model {
                model: "llama-server".to_owned(),
                message: format!("pinned runtime does not support required flag `{mandatory}`"),
            });
        }
    }
    Ok(help)
}

async fn wait_until_healthy(inner: &Inner, progress: &impl Fn(LocalServerProgress)) -> Result<()> {
    let client = reqwest::Client::new();
    let endpoint = format!("http://127.0.0.1:{}/health", inner.port);
    let deadline = Instant::now() + inner.config.startup_timeout;
    progress(LocalServerProgress::Loading);
    loop {
        if Instant::now() >= deadline {
            return Err(Error::Model {
                model: model_label(&inner.config.model_path),
                message: format!(
                    "llama-server did not become healthy; stderr tail: {}",
                    stderr_string(&inner.stderr)
                ),
            });
        }
        let exit_status = {
            let mut child = inner.child.lock().await;
            match child.as_mut() {
                Some(child) => child.try_wait().map_err(|error| Error::Model {
                    model: "llama-server".to_owned(),
                    message: format!("could not inspect server process: {error}"),
                })?,
                None => None,
            }
        };
        if let Some(status) = exit_status {
            return Err(Error::Model {
                model: model_label(&inner.config.model_path),
                message: format!(
                    "llama-server exited before becoming healthy ({status}); stderr tail: {}",
                    stderr_string(&inner.stderr)
                ),
            });
        }
        let healthy = if let Ok(response) = client
            .get(&endpoint)
            .bearer_auth(&inner.api_key)
            .send()
            .await
        {
            response.status().is_success()
                && response
                    .json::<serde_json::Value>()
                    .await
                    .is_ok_and(|body| body["status"].as_str() == Some("ok"))
        } else {
            false
        };
        if healthy {
            progress(LocalServerProgress::Ready { port: inner.port });
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn kill_child(inner: &Inner) {
    if let Some(mut child) = inner.child.lock().await.take() {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

async fn free_port() -> Result<u16> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    Ok(listener.local_addr()?.port())
}

fn find_server(runtime_dir: &Path) -> Result<PathBuf> {
    let candidates = if runtime_dir.is_file() {
        vec![runtime_dir.to_path_buf()]
    } else {
        let mut found = Vec::new();
        let mut pending = vec![runtime_dir.to_path_buf()];
        while let Some(directory) = pending.pop() {
            let entries = std::fs::read_dir(&directory).map_err(Error::Io)?;
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    found.push(path);
                }
            }
        }
        found
    };
    candidates
        .into_iter()
        .find(|path| {
            let filename = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if cfg!(windows) {
                filename.eq_ignore_ascii_case("llama-server.exe")
            } else {
                filename == "llama-server"
            }
        })
        .ok_or_else(|| Error::ModelMissing(format!("llama-server in {}", runtime_dir.display())))
}

fn stderr_tail(stderr: &Mutex<VecDeque<String>>) -> Vec<String> {
    stderr
        .lock()
        .map(|tail| tail.iter().cloned().collect())
        .unwrap_or_default()
}

fn stderr_string(stderr: &Mutex<VecDeque<String>>) -> String {
    stderr_tail(stderr).join("\n")
}

fn model_label(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("local-llm")
        .to_owned()
}

#[cfg(windows)]
fn attach_job(inner: &Inner, child: &Child) -> Result<()> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        return Err(Error::Model {
            model: "llama-server".to_owned(),
            message: "could not create Windows child-process job".to_owned(),
        });
    }
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let set = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &mut limits as *mut _ as *mut _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    let assigned = match child.raw_handle() {
        Some(process) => unsafe { AssignProcessToJobObject(job, process as _) },
        None => 0,
    };
    if set == 0 || assigned == 0 {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(job);
        }
        return Err(Error::Model {
            model: "llama-server".to_owned(),
            message: "could not attach llama-server to a Windows job".to_owned(),
        });
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(job as _) };
    if let Ok(mut current) = inner.job.lock() {
        *current = Some(handle);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex, atomic::Ordering},
        time::Duration,
    };

    use super::{
        Inner, LocalSampling, LocalServerConfig, RequestGuard, add_optional_flag, idle_stop_guard,
        track_stream, with_local_sampling,
    };
    use crate::{ChatDelta, ChatMessage, ChatRequest, ChatStream};
    use futures_util::{StreamExt, stream};
    use tokio::process::Command;

    fn test_inner(idle_timeout: Duration) -> Arc<Inner> {
        let (stop, _) = tokio::sync::watch::channel(false);
        let mut config = LocalServerConfig::new("runtime", "model");
        config.idle_timeout = idle_timeout;
        Arc::new(Inner {
            config,
            executable: "llama-server".into(),
            port: 0,
            api_key: String::new(),
            child: tokio::sync::Mutex::new(None),
            stderr: Arc::new(Mutex::new(VecDeque::new())),
            last_activity: Mutex::new(std::time::Instant::now()),
            in_flight: std::sync::atomic::AtomicUsize::new(0),
            request_gate: Arc::new(tokio::sync::RwLock::new(())),
            stop,
            #[cfg(windows)]
            job: Mutex::new(None),
        })
    }

    #[tokio::test]
    async fn long_request_prevents_idle_stop_and_resets_idle_clock_on_completion() {
        let inner = test_inner(Duration::from_millis(30));
        let guard = RequestGuard::new(inner.clone()).await;

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(idle_stop_guard(&inner).await.is_none());
        assert_eq!(inner.in_flight.load(Ordering::Acquire), 1);

        drop(guard);
        assert!(idle_stop_guard(&inner).await.is_none());
        tokio::time::sleep(Duration::from_millis(35)).await;
        assert!(idle_stop_guard(&inner).await.is_some());
        assert_eq!(inner.in_flight.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn streamed_request_stays_in_flight_until_stream_ends_or_is_dropped() {
        let inner = test_inner(Duration::from_millis(20));
        let guard = RequestGuard::new(inner.clone()).await;
        let response: ChatStream = Box::pin(stream::iter([Ok(ChatDelta {
            text: "token".to_owned(),
            stop_reason: None,
            usage: None,
        })]));
        let mut tracked = track_stream(response, guard);
        assert_eq!(inner.in_flight.load(Ordering::Acquire), 1);
        assert!(tracked.next().await.is_some());
        assert_eq!(inner.in_flight.load(Ordering::Acquire), 1);
        assert!(tracked.next().await.is_none());
        assert_eq!(inner.in_flight.load(Ordering::Acquire), 0);

        let guard = RequestGuard::new(inner.clone()).await;
        let pending: ChatStream = Box::pin(stream::pending());
        let tracked = track_stream(pending, guard);
        drop(tracked);
        assert_eq!(inner.in_flight.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn request_errors_and_cancelled_futures_release_the_guard() {
        let inner = test_inner(Duration::from_secs(1));
        let result: Result<(), &'static str> = async {
            let _guard = RequestGuard::new(inner.clone()).await;
            Err("request failed")
        }
        .await;
        assert_eq!(result, Err("request failed"));
        assert_eq!(inner.in_flight.load(Ordering::Acquire), 0);

        let request_inner = inner.clone();
        let task = tokio::spawn(async move {
            let _guard = RequestGuard::new(request_inner).await;
            std::future::pending::<()>().await;
        });
        while inner.in_flight.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
        task.abort();
        let _ = task.await;
        assert_eq!(inner.in_flight.load(Ordering::Acquire), 0);
    }

    #[test]
    fn includes_supported_optional_flags_and_drops_unknown_ones() {
        let mut command = Command::new("fake");
        let help = "-ngl N --jinja --flash-attn TYPE --no-webui --reasoning [on|off|auto]";
        add_optional_flag(&mut command, help, &["-ngl", "--n-gpu-layers"], Some("999"));
        add_optional_flag(&mut command, help, &["--jinja"], None);
        add_optional_flag(&mut command, help, &["-fa", "--flash-attn"], Some("auto"));
        add_optional_flag(&mut command, help, &["--cache-type-k"], Some("q8_0"));
        add_optional_flag(&mut command, help, &["--no-webui"], None);
        add_optional_flag(&mut command, help, &["--reasoning"], Some("off"));
        assert_eq!(
            command
                .as_std()
                .get_args()
                .map(|value| value.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            [
                "-ngl",
                "999",
                "--jinja",
                "--flash-attn",
                "auto",
                "--no-webui",
                "--reasoning",
                "off"
            ]
        );
    }

    #[test]
    fn local_sampling_fills_only_unspecified_request_values() {
        let request = ChatRequest {
            system: String::new(),
            messages: vec![ChatMessage::user("summary")],
            max_tokens: 100,
            temperature: Some(0.2),
            top_p: None,
            top_k: None,
            min_p: None,
            presence_penalty: None,
            json_schema: None,
            stop: Vec::new(),
        };
        let applied = with_local_sampling(
            request,
            LocalSampling {
                temperature: Some(0.7),
                top_p: Some(0.8),
                top_k: Some(20),
                min_p: Some(0.0),
                presence_penalty: Some(1.5),
            },
        );
        assert_eq!(applied.temperature, Some(0.2));
        assert_eq!(applied.top_p, Some(0.8));
        assert_eq!(applied.top_k, Some(20));
        assert_eq!(applied.min_p, Some(0.0));
        assert_eq!(applied.presence_penalty, Some(1.5));
    }

    #[test]
    fn reasoning_flag_is_omitted_for_older_runtimes() {
        let mut command = tokio::process::Command::new("fake");
        add_optional_flag(&mut command, "--jinja", &["--reasoning"], Some("off"));
        assert_eq!(command.as_std().get_args().count(), 0);
    }
}
