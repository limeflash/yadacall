//! Port traits implemented by adapter crates. Kept synchronous and `Send`: inference runs on
//! dedicated blocking threads owned by `yc-engine`, never on the async runtime.

use crate::transcript::Recognized;
use crate::Result;

/// Static facts about a loaded model, shown in the UI and stored with results.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    /// Catalog id, e.g. `gigaam-v3-e2e-rnnt-int8`.
    pub id: String,
    /// Execution backend actually in use, e.g. `coreml`, `cuda`, `directml`, `cpu`.
    pub backend: String,
}

/// Offline speech recogniser for one speech region (≤ ~25 s) of 16 kHz mono f32 audio.
/// Long audio is split by VAD before it gets here.
pub trait SpeechRecognizer: Send {
    fn info(&self) -> &ModelInfo;
    /// Longest chunk the model handles well; callers split longer regions.
    fn max_chunk_ms(&self) -> u64;
    /// `language` is a hint (`ru`, `en`, `auto`); engines that are single-language ignore it.
    fn recognize(&mut self, pcm: &[f32], language: &str) -> Result<Vec<Recognized>>;
}

/// A speech region found by VAD, in samples at 16 kHz relative to the stream start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeechRegion {
    pub start: u64,
    pub end: u64,
}

/// Streaming voice-activity detector. Feed audio in any chunk size; closed regions come out as
/// soon as trailing silence (or the max length) is reached.
pub trait VoiceActivityDetector: Send {
    fn push(&mut self, pcm: &[f32]) -> Result<Vec<SpeechRegion>>;
    /// Close any open region at end of stream.
    fn flush(&mut self) -> Vec<SpeechRegion>;
    fn reset(&mut self);
}

/// Fixed-size voice embedding of an utterance (for diarisation and recognising known voices).
pub trait SpeakerEmbedder: Send {
    fn info(&self) -> &ModelInfo;
    fn dims(&self) -> usize;
    /// Returns an L2-normalised embedding.
    fn embed(&mut self, pcm: &[f32]) -> Result<Vec<f32>>;
}

/// Text embedder for semantic search. Implementations apply model-specific prefixes/instructions
/// themselves, so callers only say whether they embed queries or passages.
pub trait TextEmbedder: Send {
    fn info(&self) -> &ModelInfo;
    fn dims(&self) -> usize;
    /// Returns L2-normalised embeddings, one per input.
    fn embed(&mut self, texts: &[&str], kind: EmbedKind) -> Result<Vec<Vec<f32>>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedKind {
    Query,
    Passage,
}
