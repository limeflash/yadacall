//! `yadacall runtime` sidecar installation and real generation benchmark.

use std::path::PathBuf;

use anyhow::Context;
use clap::{Args, Subcommand, ValueEnum};
use yc_hub::{
    catalog,
    downloader::{DownloadControl, Downloader},
    runtime::{self, RuntimeBackend},
};

/// Runtime command options.
#[derive(Debug, Args)]
pub struct RuntimeArgs {
    /// Runtime action.
    #[command(subcommand)]
    pub action: RuntimeAction,
}

/// Runtime action.
#[derive(Debug, Subcommand)]
pub enum RuntimeAction {
    /// Download the matching runtime pack and smoke-test `llama-server --version`.
    Pull {
        /// Choose the backend for this host.
        #[arg(long, value_enum)]
        backend: Option<BackendArg>,
    },
    /// Start llama-server once, measure a short completion, and save calibration.
    Bench {
        /// GGUF model id that has already been downloaded.
        model: String,
        /// Runtime backend to use (defaults to the host's fastest available pack).
        #[arg(long, value_enum)]
        backend: Option<BackendArg>,
        /// Maximum generated tokens for the smoke completion.
        #[arg(long, default_value_t = 64)]
        tokens: u32,
    },
}

/// CLI runtime backend value.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum BackendArg {
    /// Apple Metal.
    Metal,
    /// Vulkan.
    Vulkan,
    /// NVIDIA CUDA.
    Cuda,
    /// CPU only.
    Cpu,
}

impl From<BackendArg> for RuntimeBackend {
    fn from(value: BackendArg) -> Self {
        match value {
            BackendArg::Metal => Self::Metal,
            BackendArg::Vulkan => Self::Vulkan,
            BackendArg::Cuda => Self::Cuda,
            BackendArg::Cpu => Self::Cpu,
        }
    }
}

/// Dispatch runtime management actions.
pub async fn run(args: RuntimeArgs) -> anyhow::Result<()> {
    match args.action {
        RuntimeAction::Pull { backend } => pull(backend).await,
        RuntimeAction::Bench {
            model,
            backend,
            tokens,
        } => benchmark(&model, backend, tokens).await,
    }
}

async fn pull(backend: Option<BackendArg>) -> anyhow::Result<()> {
    let catalog = catalog::embedded()?;
    let platform = runtime::current_platform();
    let backend = backend.map(Into::into).unwrap_or_else(default_backend);
    let pack = runtime::select(&catalog.runtimes, platform, backend)?;
    let data_dir = app_data_dir()?;
    let downloader = Downloader::new(&data_dir)?;
    let executable =
        runtime::pull(&data_dir, pack, &downloader, &DownloadControl::default()).await?;
    println!(
        "Installed {} ({}) and verified llama-server --version at {}",
        pack.id,
        pack.release_tag,
        executable.display()
    );
    Ok(())
}

async fn benchmark(model_id: &str, backend: Option<BackendArg>, tokens: u32) -> anyhow::Result<()> {
    let catalog = catalog::embedded()?;
    let model = catalog::model(&catalog, model_id)?;
    let platform = runtime::current_platform();
    let backend = backend.map(Into::into).unwrap_or_else(default_backend);
    let pack = runtime::select(&catalog.runtimes, platform, backend)?;
    let data_dir = app_data_dir()?;
    let runtime_dir = data_dir.join("runtimes").join(&pack.id);
    let executable = runtime::find_llama_server(&runtime_dir).with_context(|| {
        format!(
            "run `yadacall runtime pull --backend {} first",
            backend_name(backend)
        )
    })?;
    let result =
        runtime::benchmark_model(&data_dir, model, &executable, backend_name(backend), tokens)
            .await?;
    println!(
        "{} [{}]: predicted {:.1} tok/s, measured {:.1} tok/s ({} tokens, {:.0} ms); saved calibration to {}",
        result.model_id,
        result.backend,
        result.predicted_tok_s,
        result.measured_tok_s,
        result.generated_tokens,
        result.generation_ms,
        data_dir.join("calibration.json").display()
    );
    Ok(())
}

fn default_backend() -> RuntimeBackend {
    if runtime::current_platform() == "macos-arm64" {
        RuntimeBackend::Metal
    } else {
        RuntimeBackend::Cpu
    }
}

fn backend_name(backend: RuntimeBackend) -> &'static str {
    match backend {
        RuntimeBackend::Metal => "metal",
        RuntimeBackend::Vulkan => "vulkan",
        RuntimeBackend::Cuda => "cuda",
        RuntimeBackend::Cpu => "cpu",
    }
}

fn app_data_dir() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("YC_DATA_DIR") {
        let path = PathBuf::from(path);
        std::fs::create_dir_all(&path).context("create configured YadaCall data directory")?;
        return Ok(path);
    }
    let root = dirs::data_local_dir().context("cannot resolve the local app data directory")?;
    let path = root.join("YadaCall");
    std::fs::create_dir_all(&path).context("create YadaCall app data directory")?;
    Ok(path)
}
