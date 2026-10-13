//! Prebuilt llama.cpp runtime pack selection, extraction, and smoke testing.

use std::{
    fs::File,
    path::{Component, Path, PathBuf},
    process::Command,
    time::Duration,
};

use crate::{
    Error, Result, benchmark,
    calibration::{Calibration, CalibrationMetric, Measurement},
    catalog::{ModelFile, ModelKind, ModelSpec, RuntimeSpec, RuntimeSupportAsset},
    downloader::{DownloadControl, Downloader, model_storage_dir},
    hw, predictor,
};
use serde::Serialize;

/// The llama.cpp repository's public release API.
pub const LLAMA_CPP_RELEASE_API: &str =
    "https://api.github.com/repos/ggml-org/llama.cpp/releases/latest";

/// Runtime backend users can request on the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeBackend {
    /// Apple Silicon Metal archive.
    Metal,
    /// Vulkan archive.
    Vulkan,
    /// NVIDIA CUDA archive.
    Cuda,
    /// CPU-only archive.
    Cpu,
}

impl std::str::FromStr for RuntimeBackend {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "metal" => Ok(Self::Metal),
            "vulkan" => Ok(Self::Vulkan),
            "cuda" => Ok(Self::Cuda),
            "cpu" => Ok(Self::Cpu),
            _ => Err(Error::Invalid(format!(
                "unsupported runtime backend `{value}`"
            ))),
        }
    }
}

/// Select the matching runtime entry for this host and requested backend.
pub fn select<'a>(
    runtimes: &'a [RuntimeSpec],
    platform: &str,
    backend: RuntimeBackend,
) -> Result<&'a RuntimeSpec> {
    let backend = match backend {
        RuntimeBackend::Metal => "metal",
        RuntimeBackend::Vulkan => "vulkan",
        RuntimeBackend::Cuda => "cuda",
        RuntimeBackend::Cpu => "cpu",
    };
    runtimes
        .iter()
        .find(|runtime| runtime.platform == platform && runtime.backend == backend)
        .ok_or_else(|| Error::NotFound(format!("{backend} runtime for {platform}")))
}

/// Current platform runtime key for the CLI.
pub const fn current_platform() -> &'static str {
    if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        "macos-arm64"
    } else if cfg!(target_os = "windows") && cfg!(target_arch = "x86_64") {
        "windows-x64"
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        "linux-x64"
    } else {
        "unsupported"
    }
}

/// Result from a real llama-server generation smoke benchmark.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmBenchmark {
    /// Catalog model id.
    pub model_id: String,
    /// Runtime backend used for this request.
    pub backend: String,
    /// Analytical estimate before recording this run.
    pub predicted_tok_s: f64,
    /// Analytical prompt-processing estimate before recording this run.
    pub predicted_prefill_tok_s: f64,
    /// llama.cpp's reported generation throughput.
    pub measured_tok_s: f64,
    /// llama.cpp's reported prompt-processing throughput.
    pub measured_prefill_tok_s: f64,
    /// Number of prompt tokens used for the prefill measurement.
    pub prompt_tokens: u64,
    /// llama.cpp-reported prompt-processing duration in milliseconds.
    pub prompt_ms: f64,
    /// Number of generated tokens used in the measurement.
    pub generated_tokens: u64,
    /// llama.cpp-reported generation duration in milliseconds.
    pub generation_ms: f64,
}

/// Download, verify, and extract a runtime archive into `runtimes/<id>/`.
pub async fn pull(
    root: &Path,
    runtime: &RuntimeSpec,
    downloader: &Downloader,
    control: &DownloadControl,
) -> Result<PathBuf> {
    if !runtime.url.starts_with("https://") {
        return Err(Error::Invalid("runtime URL must use HTTPS".to_owned()));
    }
    let artifact_name = runtime
        .asset_name
        .rsplit('/')
        .next()
        .unwrap_or(&runtime.asset_name);
    let archive_file = ModelFile {
        path: artifact_name.to_owned(),
        size: runtime.size,
        sha256: runtime.sha256.clone(),
        urls: vec![runtime.url.clone()],
        hf_repo: None,
        github_release_api: None,
        github_asset: None,
        optional: false,
    };
    let temporary = root.join(".runtime-downloads").join(&runtime.id);
    let required_bytes = runtime.size.saturating_add(
        runtime
            .extra_assets
            .iter()
            .map(|asset| asset.size)
            .sum::<u64>(),
    );
    downloader.preflight_space(&temporary, required_bytes)?;
    let archive_path = downloader
        .download_file(&runtime.id, &archive_file, &temporary, control)
        .await?;
    let install_dir = root.join("runtimes").join(&runtime.id);
    let zip_path = archive_path.clone();
    let extract_dir = install_dir.clone();
    let archive_name = artifact_name.to_owned();
    tokio::task::spawn_blocking(move || {
        extract_archive_flattened(&zip_path, &extract_dir, &archive_name)
    })
    .await
    .map_err(|error| Error::Storage(format!("runtime extraction worker failed: {error}")))??;
    tokio::fs::remove_file(&archive_path).await?;
    for support in &runtime.extra_assets {
        let support_file = support_as_file(support);
        let support_path = downloader
            .download_file(&runtime.id, &support_file, &temporary, control)
            .await?;
        let support_archive = support_path.clone();
        let support_dir = install_dir.clone();
        let support_name = support.asset_name.clone();
        tokio::task::spawn_blocking(move || {
            extract_archive_flattened(&support_archive, &support_dir, &support_name)
        })
        .await
        .map_err(|error| {
            Error::Storage(format!("runtime support extraction worker failed: {error}"))
        })??;
        tokio::fs::remove_file(&support_path).await?;
    }
    if let Some(parent) = archive_path.parent() {
        let _ = tokio::fs::remove_dir(parent).await;
    }
    let executable = find_llama_server(&install_dir)?;
    let output = Command::new(&executable)
        .arg("--version")
        .output()
        .map_err(|error| Error::Model {
            model: runtime.id.clone(),
            message: format!("could not start llama-server: {error}"),
        })?;
    if !output.status.success() {
        return Err(Error::Model {
            model: runtime.id.clone(),
            message: format!(
                "llama-server --version exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(executable)
}

/// Start the local server, generate a short completion, and persist an LLM calibration sample.
pub async fn benchmark_model(
    root: &Path,
    model: &ModelSpec,
    executable: &Path,
    backend: &str,
    max_tokens: u32,
) -> Result<LlmBenchmark> {
    let model_file = model
        .files
        .iter()
        .find(|file| file.path.ends_with(".gguf"))
        .ok_or_else(|| Error::Invalid(format!("model `{}` has no GGUF file", model.id)))?;
    let model_path = model_storage_dir(root, model)?.join(&model_file.path);
    if !model_path.is_file() {
        return Err(Error::ModelMissing(model.id.clone()));
    }
    let port_listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = port_listener.local_addr()?.port();
    drop(port_listener);
    let mut server = tokio::process::Command::new(executable)
        .args(["--host", "127.0.0.1", "--port", &port.to_string(), "-m"])
        .arg(&model_path)
        .args([
            "-ngl",
            "999",
            "-c",
            "8192",
            "--cache-type-k",
            "q8_0",
            "--cache-type-v",
            "q8_0",
        ])
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| Error::Model {
            model: model.id.clone(),
            message: format!("start llama-server: {error}"),
        })?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|error| Error::Network(error.to_string()))?;
    let base_url = format!("http://127.0.0.1:{port}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        if tokio::time::Instant::now() >= deadline {
            let _ = server.kill().await;
            return Err(Error::Model {
                model: model.id.clone(),
                message: "llama-server did not become ready within 120 seconds".to_owned(),
            });
        }
        if server.try_wait().map_err(Error::Io)?.is_some() {
            return Err(Error::Model {
                model: model.id.clone(),
                message: "llama-server exited before becoming ready".to_owned(),
            });
        }
        if client
            .get(format!("{base_url}/health"))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    // About 1.3–1.7k tokens depending on the model tokenizer: long enough for stable prompt
    // timings, but short enough to keep a 2B CPU-only benchmark close to a few seconds.
    let prompt = benchmark_prompt();
    let response = client
        .post(format!("{base_url}/v1/chat/completions"))
        .json(&serde_json::json!({
            // Instruction-tuned models stop at once on a raw prompt; go through the chat template.
            "messages": [{
                "role": "user",
                "content": prompt
            }],
            "max_tokens": max_tokens.max(8),
            "temperature": 0.0,
            "cache_prompt": false
        }))
        .send()
        .await
        .map_err(|error| Error::Network(format!("llama-server completion: {error}")))?
        .error_for_status()
        .map_err(|error| Error::Network(format!("llama-server completion: {error}")))?;
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|error| Error::Network(format!("parse llama-server response: {error}")))?;
    let measured = body
        .pointer("/timings/predicted_per_second")
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| Error::Model {
            model: model.id.clone(),
            message: "llama-server response did not include timings.predicted_per_second"
                .to_owned(),
        })?;
    let measured_prefill = body
        .pointer("/timings/prompt_per_second")
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| Error::Model {
            model: model.id.clone(),
            message: "llama-server response did not include timings.prompt_per_second".to_owned(),
        })?;
    if !(measured.is_finite()
        && measured > 0.0
        && measured_prefill.is_finite()
        && measured_prefill > 0.0)
    {
        let _ = server.kill().await;
        return Err(Error::Model {
            model: model.id.clone(),
            message: "llama-server reported no prompt or generation speed".to_owned(),
        });
    }
    let prompt_tokens = body
        .pointer("/timings/prompt_n")
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            body.pointer("/usage/prompt_tokens")
                .and_then(serde_json::Value::as_u64)
        })
        .unwrap_or_default();
    let prompt_ms = body
        .pointer("/timings/prompt_ms")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_default();
    let generated_tokens = body
        .pointer("/timings/predicted_n")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let generation_ms = body
        .pointer("/timings/predicted_ms")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_default();
    let _ = server.kill().await;

    let models_dir = root.join("models");
    let profile = hw::probe(&models_dir)?;
    let benchmark = tokio::task::spawn_blocking(benchmark::run)
        .await
        .map_err(|error| Error::Storage(format!("benchmark worker failed: {error}")))?;
    let calibration_path = root.join("calibration.json");
    let mut calibration = Calibration::load(&calibration_path)?;
    let prediction = predictor::predict(model, &profile, &benchmark, &Calibration::default());
    let predicted = prediction.est_tok_s.unwrap_or_default();
    let predicted_prefill = prediction.est_prefill_tok_s.unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    calibration.record(Measurement {
        model_id: model.id.clone(),
        kind: ModelKind::Llm,
        metric: CalibrationMetric::Primary,
        backend: backend.to_owned(),
        predicted,
        measured,
        measured_at_unix: now,
    });
    calibration.record(Measurement {
        model_id: model.id.clone(),
        kind: ModelKind::Llm,
        metric: CalibrationMetric::Prefill,
        backend: backend.to_owned(),
        predicted: predicted_prefill,
        measured: measured_prefill,
        measured_at_unix: now,
    });
    calibration.save(&calibration_path)?;
    Ok(LlmBenchmark {
        model_id: model.id.clone(),
        backend: backend.to_owned(),
        predicted_tok_s: predicted,
        predicted_prefill_tok_s: predicted_prefill,
        measured_tok_s: measured,
        measured_prefill_tok_s: measured_prefill,
        prompt_tokens,
        prompt_ms,
        generated_tokens,
        generation_ms,
    })
}

fn benchmark_prompt() -> String {
    let notes = "The product team reviewed customer feedback, launch risks, owners, delivery dates, and follow-up actions during the weekly planning meeting. ";
    let mut prompt = notes.repeat(60);
    prompt.push_str(
        "Summarize the decisions, risks, owners, and next steps from these meeting notes.",
    );
    prompt
}

/// Extract a zip pack, strip a common top-level directory, and preserve Unix executable bits.
pub fn extract_zip_flattened(archive_path: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination)?;
    let archive = File::open(archive_path)?;
    let mut zip = zip::ZipArchive::new(archive)
        .map_err(|error| Error::Storage(format!("invalid runtime archive: {error}")))?;
    let common_root = common_top_level(&mut zip);
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| Error::Storage(format!("read runtime archive entry: {error}")))?;
        let Some(enclosed) = entry.enclosed_name() else {
            continue;
        };
        let relative = strip_common_root(&enclosed, common_root.as_deref());
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = destination.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            // Symlink entry: its content is the link target (e.g. `libllama.0.dylib -> libllama.0.6.0.dylib`).
            let mut link = String::new();
            std::io::Read::read_to_string(&mut entry, &mut link)?;
            create_archive_symlink(&target, Path::new(link.trim()), destination)?;
            continue;
        }
        let mut output = File::create(&target)?;
        std::io::copy(&mut entry, &mut output)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode & 0o777))?;
        }
    }
    Ok(())
}

/// Recreates a symlink from an archive. Runtime packs ship versioned dylibs/so files reached through
/// links (`libllama-common.0.dylib -> libllama-common.0.6.0.dylib`); without them the server can't
/// load. Only relative targets that stay inside `destination` are accepted.
fn create_archive_symlink(link_path: &Path, link_target: &Path, destination: &Path) -> Result<()> {
    let escapes = link_target.is_absolute()
        || link_target.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        });
    let resolved = link_path.parent().unwrap_or(destination).join(link_target);
    if escapes || !resolved.starts_with(destination) {
        return Err(Error::Storage(format!(
            "runtime archive link {} points outside the pack",
            link_path.display()
        )));
    }
    let _ = std::fs::remove_file(link_path);
    #[cfg(unix)]
    std::os::unix::fs::symlink(link_target, link_path)?;
    #[cfg(not(unix))]
    {
        // Windows packs don't rely on links; if one appears, materialise it as a copy when possible.
        if resolved.is_file() {
            std::fs::copy(&resolved, link_path)?;
        }
    }
    Ok(())
}

/// Extract a zip or gzip-compressed tar archive while stripping its common top-level directory.
pub fn extract_archive_flattened(
    archive_path: &Path,
    destination: &Path,
    name: &str,
) -> Result<()> {
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        extract_tar_gz_flattened(archive_path, destination)
    } else {
        extract_zip_flattened(archive_path, destination)
    }
}

fn extract_tar_gz_flattened(archive_path: &Path, destination: &Path) -> Result<()> {
    let decoder = flate2::read::GzDecoder::new(File::open(archive_path)?);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| Error::Storage(format!("invalid runtime tar archive: {error}")))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| Error::Storage(format!("read runtime tar entry: {error}")))?;
        if let Ok(path) = entry.path() {
            paths.push(path.into_owned());
        }
    }
    let common_root = common_path_component(&paths);
    drop(archive);
    let decoder = flate2::read::GzDecoder::new(File::open(archive_path)?);
    let mut archive = tar::Archive::new(decoder);
    std::fs::create_dir_all(destination)?;
    for entry in archive
        .entries()
        .map_err(|error| Error::Storage(format!("read runtime tar entries: {error}")))?
    {
        let mut entry =
            entry.map_err(|error| Error::Storage(format!("read runtime tar entry: {error}")))?;
        let raw_path = entry
            .path()
            .map_err(|error| Error::Storage(format!("runtime tar path: {error}")))?
            .into_owned();
        let Some(relative) = safe_archive_path(&raw_path) else {
            continue;
        };
        let relative = strip_common_root(&relative, common_root.as_deref());
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = destination.join(relative);
        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if entry.header().entry_type().is_symlink() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if let Ok(Some(link)) = entry.link_name() {
                create_archive_symlink(&target, &link, destination)?;
            }
            continue;
        }
        if !entry.header().entry_type().is_file() {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut output = File::create(&target)?;
        std::io::copy(&mut entry, &mut output)?;
        #[cfg(unix)]
        if let Ok(mode) = entry.header().mode() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode & 0o777))?;
        }
    }
    Ok(())
}

/// Find `llama-server` inside an extracted runtime directory.
pub fn find_llama_server(root: &Path) -> Result<PathBuf> {
    let executable = if cfg!(target_os = "windows") {
        "llama-server.exe"
    } else {
        "llama-server"
    };
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.file_name().is_some_and(|name| name == executable) {
                return Ok(path);
            }
        }
    }
    Err(Error::NotFound(format!(
        "{executable} in {}",
        root.display()
    )))
}

fn common_top_level(zip: &mut zip::ZipArchive<File>) -> Option<String> {
    let mut root: Option<String> = None;
    for index in 0..zip.len() {
        let entry = zip.by_index(index).ok()?;
        let enclosed = entry.enclosed_name()?;
        let first = enclosed.components().next()?;
        let Component::Normal(name) = first else {
            return None;
        };
        let name = name.to_string_lossy().to_string();
        match &root {
            None => root = Some(name),
            Some(previous) if *previous == name => {}
            Some(_) => return None,
        }
    }
    root
}

fn strip_common_root(path: &Path, root: Option<&str>) -> PathBuf {
    if let Some(root) = root {
        let mut components = path.components();
        if components
            .next()
            .is_some_and(|component| component.as_os_str() == root)
        {
            return components.as_path().to_path_buf();
        }
    }
    path.to_path_buf()
}

fn common_path_component(paths: &[PathBuf]) -> Option<String> {
    let mut root: Option<String> = None;
    for path in paths {
        let first = path.components().next()?;
        let Component::Normal(name) = first else {
            return None;
        };
        let name = name.to_string_lossy().into_owned();
        match &root {
            None => root = Some(name),
            Some(previous) if *previous == name => {}
            Some(_) => return None,
        }
    }
    root
}

fn safe_archive_path(path: &Path) -> Option<PathBuf> {
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        None
    } else {
        Some(path.to_path_buf())
    }
}

fn support_as_file(asset: &RuntimeSupportAsset) -> ModelFile {
    ModelFile {
        path: asset.asset_name.clone(),
        size: asset.size,
        sha256: asset.sha256.clone(),
        urls: vec![asset.url.clone()],
        hf_repo: None,
        github_release_api: None,
        github_asset: None,
        optional: false,
    }
}
