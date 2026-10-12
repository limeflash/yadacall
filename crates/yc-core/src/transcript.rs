use serde::{Deserialize, Serialize};

use crate::Ms;
use crate::meeting::TrackKind;

/// Well-known speaker keys. Diarised remote speakers use `s1`, `s2`, …
pub const SPEAKER_ME: &str = "me";
pub const SPEAKER_THEM: &str = "them";

/// One recognised stretch of speech, on the meeting timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub start_ms: Ms,
    pub end_ms: Ms,
    pub track: TrackKind,
    /// Speaker key (see [`SPEAKER_ME`], [`SPEAKER_THEM`], `s1`…); names live in `Meeting::participants`.
    pub speaker: String,
    pub text: String,
    /// Word timings, when the engine provides them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<Word>,
    /// Mean token confidence 0..=1 when available; used by hallucination filters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Word {
    pub start_ms: Ms,
    pub end_ms: Ms,
    pub text: String,
}

/// A reply in the dialogue: consecutive segments of one speaker glued together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub speaker: String,
    pub start_ms: Ms,
    pub end_ms: Ms,
    pub text: String,
}

/// Contents of `transcript.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub segments: Vec<Segment>,
}

/// Output of a speech recogniser for one chunk of audio. Times are relative to the chunk start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recognized {
    pub start_ms: Ms,
    pub end_ms: Ms,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<Word>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}
