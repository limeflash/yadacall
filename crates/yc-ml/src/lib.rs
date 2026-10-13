//! On-device ONNX Runtime adapters for YadaCall.
//!
//! Third-party model and preprocessor provenance and licenses are listed in
//! `MODEL_SOURCES.md` at the crate root.

pub mod asr;
mod backend;
pub mod fbank;
pub mod speaker;
pub mod text_embedding;
pub mod vad;

pub use asr::{AsrCatalogEngine, AsrEngine, AsrFileNames, AsrKind, AsrModelConfig, asr_config};
pub use backend::Backend;
pub use speaker::CampPlusEmbedder;
pub use text_embedding::{OnnxTextEmbedder, Pooling, TextEmbeddingConfig};
pub use vad::SileroVad;
