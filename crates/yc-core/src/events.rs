//! Events the engine publishes. The desktop shell forwards them to the UI; the CLI prints them.

use serde::{Deserialize, Serialize};

use crate::Ms;
use crate::meeting::{MeetingId, MeetingStatus, TrackKind};
use crate::transcript::Segment;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum EngineEvent {
    /// Recording started, paused, resumed or stopped.
    Recorder {
        meeting: MeetingId,
        state: RecorderState,
        elapsed_ms: Ms,
    },
    /// Input levels, ~25 Hz while recording. `rms`/`peak` are 0..=1 (linear).
    Levels { mic: Level, system: Level },
    /// A track has been silent for a while (spec: warn after 60 s).
    TrackSilent {
        meeting: MeetingId,
        track: TrackKind,
        silent_for_ms: Ms,
    },
    /// A track's device changed (e.g. AirPods connected); recording continues.
    DeviceChanged { track: TrackKind, device: String },
    /// A live (provisional) segment recognised during the call.
    LiveSegment {
        meeting: MeetingId,
        segment: Segment,
    },
    /// Post-call status/progress of a meeting.
    Status {
        meeting: MeetingId,
        status: MeetingStatus,
    },
    /// Model/runtime download progress.
    Download {
        item: String,
        done_bytes: u64,
        total_bytes: u64,
        bytes_per_sec: u64,
        state: DownloadState,
    },
    /// Something the user should see as a toast/notification.
    Notice { level: NoticeLevel, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecorderState {
    Recording,
    Paused,
    Stopped,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Level {
    pub rms: f32,
    pub peak: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "state"
)]
pub enum DownloadState {
    Queued,
    Running,
    Paused,
    Verifying,
    Done,
    /// `resumable`: a retry continues from the bytes already on disk.
    Failed {
        message: String,
        resumable: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The UI reads camelCase fields; struct variants need `rename_all_fields` for that.
    #[test]
    fn struct_variant_fields_are_camel_case() {
        let event = EngineEvent::Download {
            item: "m".into(),
            done_bytes: 1,
            total_bytes: 2,
            bytes_per_sec: 3,
            state: DownloadState::Running,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "download");
        assert_eq!(json["doneBytes"], 1);
        assert_eq!(json["totalBytes"], 2);
        assert_eq!(json["bytesPerSec"], 3);
        assert_eq!(json["state"]["state"], "running");

        let recorder = EngineEvent::Recorder {
            meeting: crate::meeting::MeetingId::new(),
            state: RecorderState::Recording,
            elapsed_ms: 5,
        };
        assert_eq!(serde_json::to_value(&recorder).unwrap()["elapsedMs"], 5);
    }
}
