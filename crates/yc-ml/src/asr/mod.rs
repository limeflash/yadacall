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
        match kind {
            AsrKind::GigaAm => Ok(Box::new(gigaam::GigaAmRnnt::load(model_dir, backend)?)),
            AsrKind::GigaAmCtc => Ok(Box::new(gigaam::GigaAmCtc::load(model_dir, backend)?)),
            AsrKind::Parakeet => Ok(Box::new(parakeet::ParakeetTdt::load(model_dir, backend)?)),
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
