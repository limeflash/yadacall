//! Events the engine publishes. The desktop shell forwards them to the UI; the CLI prints them.

use serde::{Deserialize, Serialize};

use crate::meeting::{MeetingId, MeetingStatus, TrackKind};
use crate::transcript::Segment;
use crate::Ms;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum EngineEvent {
    /// Recording started, paused, resumed or stopped.
    Recorder { meeting: MeetingId, state: RecorderState, elapsed_ms: Ms },
    /// Input levels, ~25 Hz while recording. `rms`/`peak` are 0..=1 (linear).
    Levels { mic: Level, system: Level },
    /// A track has been silent for a while (spec: warn after 60 s).
    TrackSilent { meeting: MeetingId, track: TrackKind, silent_for_ms: Ms },
    /// A track's device changed (e.g. AirPods connected); recording continues.
    DeviceChanged { track: TrackKind, device: String },
    /// A live (provisional) segment recognised during the call.
    LiveSegment { meeting: MeetingId, segment: Segment },
    /// Post-call status/progress of a meeting.
    Status { meeting: MeetingId, status: MeetingStatus },
    /// Model/runtime download progress.
    Download { item: String, done_bytes: u64, total_bytes: u64, bytes_per_sec: u64, state: DownloadState },
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
#[serde(rename_all = "camelCase", tag = "state")]
pub enum DownloadState {
    Queued,
    Running,
    Paused,
    Verifying,
    Done,
    /// `resumable`: a retry continues from the bytes already on disk.
    Failed { message: String, resumable: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}
