//! Narrow traits through which `yc-engine` reaches everything it does not implement itself.
//!
//! The real adapters (`yc-ml`, `yc-hub`, `yc-llm`, `yc-audio`) are wired in by the application;
//! tests use the deterministic fakes in [`crate::testing`]. Model-facing traits are the synchronous
//! `yc_core::ports` ones; the engine owns the threads they run on.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use crossbeam_channel::Receiver;
use tokio::sync::mpsc;
use yc_core::Result;
use yc_core::events::Level;
use yc_core::meeting::{Bookmark, MeetingId, Participant, TrackInfo, TrackKind};
use yc_core::ports::{SpeakerEmbedder, SpeechRecognizer, TextEmbedder, VoiceActivityDetector};
use yc_core::summary::Summary;
use yc_core::transcript::Turn;

pub use yc_audio::{AudioFrame, RecorderConfig, RecorderEvent};

/// Loads the speech recogniser for a meeting language. Called on a worker thread; may be slow
/// (model load), so the engine does it concurrently with the start of the recording.
pub trait AsrFactory: Send + Sync {
    /// `language` is `ru`, `en` or `auto`. Fails with `Error::ModelMissing` when nothing is installed.
    fn load(&self, language: &str) -> Result<Box<dyn SpeechRecognizer>>;
}

/// Creates one streaming VAD per track (and one per offline pass).
pub trait VadFactory: Send + Sync {
    fn create(&self) -> Result<Box<dyn VoiceActivityDetector>>;
}

/// Creates the speaker-embedding model; `Ok(None)` when it is not installed (diarisation is skipped).
pub trait SpeakerEmbedderFactory: Send + Sync {
    /// Cheap check (no model load) used to decide whether to queue a `diarize` job.
    fn is_available(&self) -> bool {
        true
    }
    fn create(&self) -> Result<Option<Box<dyn SpeakerEmbedder>>>;
}

/// Creates the text-embedding model; `Ok(None)` when it is not installed (search stays lexical).
pub trait TextEmbedderFactory: Send + Sync {
    /// Cheap check (no model load) for "is a text-embedding model installed".
    fn is_available(&self) -> bool {
        true
    }
    fn create(&self) -> Result<Option<Box<dyn TextEmbedder>>>;
}

/// A speaker-embedding factory for builds without the model.
pub struct NoSpeakerEmbedder;
impl SpeakerEmbedderFactory for NoSpeakerEmbedder {
    fn is_available(&self) -> bool {
        false
    }
    fn create(&self) -> Result<Option<Box<dyn SpeakerEmbedder>>> {
        Ok(None)
    }
}

/// A text-embedding factory for builds without the model.
pub struct NoTextEmbedder;
impl TextEmbedderFactory for NoTextEmbedder {
    fn is_available(&self) -> bool {
        false
    }
    fn create(&self) -> Result<Option<Box<dyn TextEmbedder>>> {
        Ok(None)
    }
}

/// Everything the summariser sees. Built from the finished (diarised) transcript.
#[derive(Debug, Clone)]
pub struct SummaryInput {
    pub meeting_id: MeetingId,
    pub title: String,
    /// Language the transcript is in (`ru`, `en`, …); the summary should be written in it too.
    pub language: String,
    pub duration_ms: u64,
    pub turns: Vec<Turn>,
    /// Speaker keys used in `turns` with their display names.
    pub participants: Vec<Participant>,
    /// Moments the user flagged during the call.
    pub bookmarks: Vec<Bookmark>,
}

/// Produces `summary.json` content from a transcript (LLM-backed).
#[async_trait]
pub trait Summarizer: Send + Sync {
    /// Whether a provider is configured and usable; when false no `summarize` job is queued.
    fn is_configured(&self) -> bool {
        true
    }
    /// `progress` takes 0.0..=1.0. Errors with `Error::Llm`/`Error::Network` are retried.
    async fn summarize(
        &self,
        input: SummaryInput,
        progress: &(dyn Fn(f32) + Send + Sync),
    ) -> Result<Summary>;
}

/// One passage of a past meeting offered to the answerer as evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct Excerpt {
    pub meeting_id: MeetingId,
    pub meeting_title: String,
    pub started_at: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub speakers: Vec<String>,
    pub text: String,
}

/// A question over the library with its retrieved evidence.
#[derive(Debug, Clone)]
pub struct AnswerRequest {
    pub question: String,
    /// UI/meeting language to answer in.
    pub language: String,
    pub excerpts: Vec<Excerpt>,
}

/// Streamed pieces of an answer.
#[derive(Debug, Clone, PartialEq)]
pub enum AnswerEvent {
    /// What is about to leave the device ("6 text excerpts sent to Claude, no audio"); emitted first.
    Egress(String),
    /// Next piece of the answer text.
    Token(String),
    /// Indices into [`AnswerRequest::excerpts`] the answer relied on.
    Citations(Vec<usize>),
    /// The answer is complete.
    Done,
    /// The answer failed; no further events follow.
    Failed(String),
}

/// Answers questions over search hits, streaming the text.
#[async_trait]
pub trait Answerer: Send + Sync {
    /// Sends events on `out` until done; returns early (without error) if `out` is closed.
    async fn answer(&self, request: AnswerRequest, out: mpsc::Sender<AnswerEvent>) -> Result<()>;
}

/// A running capture session. Implemented over `yc_audio::RecorderHandle` by [`crate::YadaRecorder`]
/// and by a synthetic source in tests.
pub trait RecordingHandle: Send {
    fn pause(&self);
    fn resume(&self);
    /// Latest input level of `kind`, `None` if the track is not recorded.
    fn level(&self, kind: TrackKind) -> Option<Level>;
    /// 16 kHz mono frames for live VAD/ASR.
    fn frames(&self, kind: TrackKind) -> Option<Receiver<AudioFrame>>;
    fn events(&self) -> Receiver<RecorderEvent>;
    fn elapsed(&self) -> Duration;
    fn started_at(&self) -> SystemTime;
    /// Stops capture, flushes the tracks to disk and returns their summaries.
    fn stop(self: Box<Self>) -> Result<Vec<TrackInfo>>;
}

/// Starts capture sessions.
pub trait RecorderBackend: Send + Sync {
    fn start(&self, config: RecorderConfig) -> Result<Box<dyn RecordingHandle>>;
}

/// The adapters an [`crate::Engine`] is wired with.
#[derive(Clone)]
pub struct Ports {
    pub asr: Arc<dyn AsrFactory>,
    pub vad: Arc<dyn VadFactory>,
    pub speaker: Arc<dyn SpeakerEmbedderFactory>,
    pub text_embedder: Arc<dyn TextEmbedderFactory>,
    pub summarizer: Option<Arc<dyn Summarizer>>,
    pub answerer: Option<Arc<dyn Answerer>>,
    pub recorder: Arc<dyn RecorderBackend>,
}
