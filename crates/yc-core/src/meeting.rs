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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    /// Stable bookmark identifier. Older meeting files receive an id when loaded.
    pub id: String,
    pub at_ms: Ms,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Whether this bookmark was added after recording finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_after: Option<bool>,
    /// Relative image path inside this meeting folder, reserved for screenshot support.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<String>,
}

impl<'de> Deserialize<'de> for Bookmark {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            #[serde(default)]
            id: Option<String>,
            at_ms: Ms,
            #[serde(default)]
            label: Option<String>,
            #[serde(default)]
            created_after: Option<bool>,
            #[serde(default)]
            screenshot: Option<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let id = wire.id.filter(|id| !id.is_empty()).unwrap_or_else(|| {
            // Stable for a legacy timestamp, so repeated reads never change its identity.
            Uuid::from_u128(0x7f00_0000_0000_7000_8000_0000_0000_0000u128 | u128::from(wire.at_ms))
                .to_string()
        });
        Ok(Self {
            id,
            at_ms: wire.at_ms,
            label: wire.label,
            created_after: wire.created_after,
            screenshot: wire.screenshot,
        })
    }
}

/// A user-managed folder in the meeting library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    /// Stable folder identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Optional accent palette key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// User-controlled ordering value.
    pub order: u32,
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
    /// Calendar invite names ranked as possible identities for this speaker.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<String>,
}

/// A person invited to the calendar event that started a meeting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantHint {
    /// Display name from the calendar invite.
    pub name: String,
    /// Email address, when the calendar provider included it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
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
    /// Whether the current title is an automatically generated placeholder awaiting a summary.
    #[serde(default)]
    pub title_is_auto: bool,
    pub started_at: DateTime<Utc>,
    pub duration_ms: Ms,
    /// BCP-47-ish language hint used for transcription (`ru`, `en`, `auto`).
    pub language: String,
    /// Expected number of remote speakers; `None` = detect automatically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_speakers: Option<u8>,
    /// Whether `expected_speakers` was explicitly chosen by the user instead of inferred from an invite.
    #[serde(default)]
    pub expected_speakers_explicit: bool,
    pub tracks: Vec<TrackInfo>,
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
    /// Whether this meeting is kept outside audio retention.
    #[serde(default)]
    pub favorite: bool,
    /// User folder id, or absent for «Без папки».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    #[serde(default)]
    pub participants: Vec<Participant>,
    /// Calendar invitees supplied when recording started.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participant_hints: Vec<ParticipantHint>,
    /// Catalog id of the ASR model that produced the transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_model: Option<String>,
    pub status: MeetingStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_meeting_defaults_participant_hints_and_suggestions() {
        let mut meeting: Meeting = serde_json::from_value(serde_json::json!({
            "id": "0190a000-0000-7000-8000-000000000001",
            "title": "Синк",
            "startedAt": "2026-10-12T10:00:00Z",
            "durationMs": 1000,
            "language": "ru",
            "tracks": [],
            "participants": [{ "key": "s1", "name": "Спикер 1" }],
            "status": { "state": "ready" }
        }))
        .expect("legacy meeting loads");

        assert!(meeting.participant_hints.is_empty());
        assert!(!meeting.title_is_auto);
        assert!(!meeting.expected_speakers_explicit);
        assert!(meeting.participants[0].suggestions.is_empty());
        assert!(!meeting.favorite);
        assert_eq!(meeting.folder_id, None);

        meeting.participant_hints.push(ParticipantHint {
            name: "Анна Ковалёва".into(),
            email: Some("anna@example.com".into()),
        });
        meeting.participants[0]
            .suggestions
            .push("Анна Ковалёва".into());
        meeting.title_is_auto = true;
        meeting.expected_speakers = Some(4);
        meeting.expected_speakers_explicit = true;
        let json = serde_json::to_value(&meeting).expect("meeting serializes");
        assert_eq!(json["participantHints"][0]["email"], "anna@example.com");
        assert_eq!(json["participants"][0]["suggestions"][0], "Анна Ковалёва");
        assert_eq!(json["titleIsAuto"], true);
        assert_eq!(json["expectedSpeakersExplicit"], true);
    }

    #[test]
    fn legacy_bookmarks_receive_ids_and_new_fields_round_trip() {
        let mut meeting: Meeting = serde_json::from_value(serde_json::json!({
            "id":"0190a000-0000-7000-8000-000000000001", "title":"Синк",
            "startedAt":"2026-10-12T10:00:00Z", "durationMs":1000, "language":"ru",
            "tracks":[], "bookmarks":[{"atMs":25,"label":"важно"}], "status":{"state":"ready"}
        }))
        .expect("legacy format loads");
        assert!(!meeting.bookmarks[0].id.is_empty());
        let loaded_again: Meeting = serde_json::from_value(serde_json::json!({
            "id":"0190a000-0000-7000-8000-000000000001", "title":"Синк",
            "startedAt":"2026-10-12T10:00:00Z", "durationMs":1000, "language":"ru",
            "tracks":[], "bookmarks":[{"atMs":25,"label":"важно"}], "status":{"state":"ready"}
        }))
        .expect("legacy format reloads");
        assert_eq!(meeting.bookmarks[0].id, loaded_again.bookmarks[0].id);
        meeting.favorite = true;
        meeting.folder_id = Some("work".into());
        let json = serde_json::to_value(&meeting).expect("serialize");
        assert_eq!(json["favorite"], true);
        assert_eq!(json["folderId"], "work");
        assert_eq!(
            json["bookmarks"][0]["createdAfter"],
            serde_json::Value::Null
        );
    }
}
