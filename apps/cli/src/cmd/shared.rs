use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::ValueEnum;
use yc_ml::{AsrKind, Backend};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum EngineArg {
    Gigaam,
    Parakeet,
}

impl EngineArg {
    pub const fn kind(self) -> AsrKind {
        match self {
            Self::Gigaam => AsrKind::GigaAm,
            Self::Parakeet => AsrKind::Parakeet,
        }
    }

    pub const fn model_directory(self) -> &'static str {
        match self {
            Self::Gigaam => "istupakov__gigaam-v3-onnx",
            Self::Parakeet => "istupakov__parakeet-tdt-0.6b-v3-onnx",
        }
    }

    pub const fn repo_directory(self) -> &'static str {
        match self {
            Self::Gigaam => "gigaam-v3-onnx",
            Self::Parakeet => "parakeet-tdt-0.6b-v3-onnx",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum BackendArg {
    Auto,
    Cpu,
    Coreml,
}

impl BackendArg {
    pub const fn backend(self) -> Backend {
        match self {
            Self::Auto => Backend::Auto,
            Self::Cpu => Backend::Cpu,
            Self::Coreml => Backend::CoreMl,
        }
    }
}

pub fn resolve_models_dir(supplied: Option<PathBuf>) -> PathBuf {
    supplied
        .or_else(|| std::env::var_os("YC_MODELS_DIR").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".cache/yadacall-dev/models"))
        })
        .unwrap_or_else(|| PathBuf::from("models"))
}

pub fn resolve_engine_dir(models_dir: &Path, engine: EngineArg) -> anyhow::Result<PathBuf> {
    let required_file = match engine {
        EngineArg::Gigaam => "v3_e2e_rnnt_encoder.int8.onnx",
        EngineArg::Parakeet => "encoder-model.int8.onnx",
    };
    let direct = models_dir.join(required_file);
    if direct.is_file() {
        return Ok(models_dir.to_path_buf());
    }
    let candidates = [
        models_dir.join(engine.model_directory()),
        models_dir.join(engine.repo_directory()),
    ];
    candidates
        .into_iter()
        .find(|candidate| candidate.join(required_file).is_file())
        .with_context(|| {
            format!(
                "model `{}` is missing under {}",
                engine.model_directory(),
                models_dir.display()
            )
        })
}

pub fn fail_for_unsupported_backend(backend: BackendArg) -> anyhow::Result<()> {
    if matches!(backend, BackendArg::Coreml) && !cfg!(target_os = "macos") {
        bail!("CoreML is available only on macOS");
    }
    Ok(())
}
