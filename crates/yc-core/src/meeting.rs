use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::Ms;

/// Time-sortable meeting identifier (UUID v7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MeetingId(pub Uuid);

impl MeetingId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for MeetingId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for MeetingId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for MeetingId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

/// The two sources of a call. The track a phrase came from is the strongest speaker signal we have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackKind {
    /// Microphone — the user ("Я").
    Mic,
    /// System audio — the other side ("Собеседник" / several people in a group call).
    System,
}

impl TrackKind {
    pub const ALL: [TrackKind; 2] = [TrackKind::Mic, TrackKind::System];

    /// Stable short name used for file names (`mic.flac`, `them.flac`).
    pub fn file_stem(self) -> &'static str {
        match self {
            TrackKind::Mic => "mic",
            TrackKind::System => "them",
        }
    }
}

/// Per-track facts needed to put both tracks on one timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackInfo {
    pub kind: TrackKind,
    /// Human-readable device name ("MacBook Pro Microphone", "Zoom").
    pub device: String,
    /// Offset of the track's first sample from the meeting start, by the system clock.
    pub offset_ms: Ms,
    /// Samples written at [`crate::SAMPLE_RATE`].
    pub samples: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    pub at_ms: Ms,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Participant {
    /// Speaker key used in segments (`me`, `them`, `s1`, `s2`, …).
    pub key: String,
    pub name: String,
    /// Known voice profile this participant was matched to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_id: Option<String>,
}

/// Where a meeting is in its life cycle. Mirrors the job pipeline, coarse enough for the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "state"
)]
pub enum MeetingStatus {
    Recording,
    /// Post-call processing. `progress` is 0.0..=1.0 across all remaining stages.
    Processing {
        stage: Stage,
        progress: f32,
    },
    Ready,
    Failed {
        stage: Stage,
        message: String,
        retryable: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Transcribe,
    Diarize,
    Index,
    Summarize,
}

/// Contents of `meeting.json` — the source of truth for a meeting's metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meeting {
    pub id: MeetingId,
    pub title: String,
    pub started_at: DateTime<Utc>,
    pub duration_ms: Ms,
    /// BCP-47-ish language hint used for transcription (`ru`, `en`, `auto`).
    pub language: String,
    /// Expected number of remote speakers; `None` = detect automatically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_speakers: Option<u8>,
    pub tracks: Vec<TrackInfo>,
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
    #[serde(default)]
    pub participants: Vec<Participant>,
    /// Catalog id of the ASR model that produced the transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_model: Option<String>,
    pub status: MeetingStatus,
}
