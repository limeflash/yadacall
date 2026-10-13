//! GigaAM and Parakeet ONNX speech recognizers.

mod common;
mod gigaam;
mod parakeet;

use std::{path::Path, str::FromStr};

use yc_core::{Error, Result, ports::SpeechRecognizer};

use crate::Backend;

/// Supported ASR model variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsrKind {
    /// Russian-first GigaAM v3 end-to-end RNNT model.
    GigaAm,
    /// Russian GigaAM v3 end-to-end CTC model.
    GigaAmCtc,
    /// Multilingual Parakeet TDT 0.6B v3 model.
    Parakeet,
}

/// Catalog engine families supported by the ASR adapters, independent of `yc-hub`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsrCatalogEngine {
    /// GigaAM v3 transducer.
    GigaAmRnnt,
    /// GigaAM CTC model.
    GigaAmCtc,
    /// Parakeet TDT model.
    ParakeetTdt,
}

/// Optional artifact filenames supplied by a model catalog entry.
#[derive(Debug, Clone, Default)]
pub struct AsrFileNames {
    /// Downloaded ONNX preprocessor graph, used by Parakeet.
    pub preprocessor: Option<String>,
    /// Encoder graph, used by RNNT or Parakeet.
    pub encoder: Option<String>,
    /// Decoder/predictor graph, used by GigaAM RNNT.
    pub decoder: Option<String>,
    /// Joint graph, used by GigaAM RNNT.
    pub joint: Option<String>,
    /// Combined decoder/joint graph, used by Parakeet.
    pub decoder_joint: Option<String>,
    /// Single ONNX graph, used by CTC.
    pub model: Option<String>,
    /// Token vocabulary file.
    pub vocabulary: Option<String>,
}

/// Model identity and optional artifact names taken from the catalog.
#[derive(Debug, Clone)]
pub struct AsrModelConfig {
    /// Stable catalog id returned by the recognizer's `ModelInfo`.
    pub id: String,
    /// Optional local artifact names.
    pub files: AsrFileNames,
}

impl AsrKind {
    /// Stable CLI name for this model type.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GigaAm => "gigaam",
            Self::GigaAmCtc => "gigaam-ctc",
            Self::Parakeet => "parakeet",
        }
    }
}

/// Maps a catalog engine and artifact names to the configured ASR loader settings.
pub fn asr_config(
    engine: AsrCatalogEngine,
    id: impl Into<String>,
    files: AsrFileNames,
) -> (AsrKind, AsrModelConfig) {
    let kind = match engine {
        AsrCatalogEngine::GigaAmRnnt => AsrKind::GigaAm,
        AsrCatalogEngine::GigaAmCtc => AsrKind::GigaAmCtc,
        AsrCatalogEngine::ParakeetTdt => AsrKind::Parakeet,
    };
    (
        kind,
        AsrModelConfig {
            id: id.into(),
            files,
        },
    )
}

impl std::fmt::Display for AsrKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AsrKind {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "gigaam" | "gigaam-rnnt" => Ok(Self::GigaAm),
            "gigaam-ctc" => Ok(Self::GigaAmCtc),
            "parakeet" => Ok(Self::Parakeet),
            _ => Err(format!("unknown ASR engine `{value}`")),
        }
    }
}

/// ASR model factory. `model_dir` points directly at the Hugging Face repository files.
pub struct AsrEngine;

impl AsrEngine {
    /// Loads an ASR implementation from an already-downloaded model repository directory.
    pub fn load(
        kind: AsrKind,
        model_dir: &Path,
        backend: Backend,
    ) -> Result<Box<dyn SpeechRecognizer>> {
        let id = match kind {
            AsrKind::GigaAm => "gigaam-v3-e2e-rnnt-int8",
            AsrKind::GigaAmCtc => "gigaam-v3-e2e-ctc-int8",
            AsrKind::Parakeet => "parakeet-tdt-0.6b-v3-int8",
        };
        Self::load_configured(
            kind,
            model_dir,
            backend,
            AsrModelConfig {
                id: id.to_owned(),
                files: AsrFileNames::default(),
            },
        )
    }

    /// Loads an ASR model using its catalog identity and optional local filenames.
    pub fn load_configured(
        kind: AsrKind,
        model_dir: &Path,
        backend: Backend,
        config: AsrModelConfig,
    ) -> Result<Box<dyn SpeechRecognizer>> {
        match kind {
            AsrKind::GigaAm => Ok(Box::new(gigaam::GigaAmRnnt::load_configured(
                model_dir, backend, config,
            )?)),
            AsrKind::GigaAmCtc => Ok(Box::new(gigaam::GigaAmCtc::load_configured(
                model_dir, backend, config,
            )?)),
            AsrKind::Parakeet => Ok(Box::new(parakeet::ParakeetTdt::load_configured(
                model_dir, backend, config,
            )?)),
        }
    }
}

fn required_file(dir: &Path, name: &str) -> Result<std::path::PathBuf> {
    let path = dir.join(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(Error::ModelMissing(path.display().to_string()))
    }
}
