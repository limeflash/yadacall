//! Public configuration, event and frame types of the recorder.

use std::path::PathBuf;
use std::time::Duration;

use yc_core::error::PermissionKind;
use yc_core::meeting::TrackKind;

/// Which input device the microphone track records from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DeviceSel {
    /// The system default input; the recorder follows it when the user switches (AirPods, USB mic).
    #[default]
    Default,
    /// A specific device, matched by its stable id (see `yadacall devices`) or, failing that, by
    /// its exact / case-insensitive-substring name.
    Named(String),
}

/// Tuning of the silence watchdog (spec: warn when a track is silent for more than 60 s).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SilenceConfig {
    /// RMS level (dBFS) below which a 100 ms window counts as silent.
    pub threshold_dbfs: f32,
    /// How long a track must stay silent before `TrackSilent` is raised (and re-raised).
    pub warn_after: Duration,
    /// How long a track must be *exactly* zero since the very start of recording before
    /// `PermissionSuspected` is raised (macOS delivers digital silence when TCC access is denied).
    pub no_signal_after: Duration,
}

impl Default for SilenceConfig {
    fn default() -> Self {
        Self {
            threshold_dbfs: -50.0,
            warn_after: Duration::from_secs(60),
            no_signal_after: Duration::from_secs(10),
        }
    }
}

/// Everything [`crate::Recorder::start`] needs.
#[derive(Debug, Clone)]
pub struct RecorderConfig {
    /// Directory receiving `mic.pcm`/`mic.json` and `them.pcm`/`them.json`. Created if missing.
    pub dir: PathBuf,
    /// Record the microphone ("me"); `None` disables the track.
    pub mic: Option<DeviceSel>,
    /// Record system audio ("them"): Core Audio tap / WASAPI loopback / PulseAudio monitor.
    pub system: bool,
    /// Silence watchdog tuning.
    pub silence: SilenceConfig,
    /// Samples per [`AudioFrame`] delivered to live consumers (512 = one Silero VAD window).
    pub frame_len: usize,
    /// Capacity of each per-track frame channel; when full, frames are dropped and counted.
    pub frame_queue: usize,
    /// Hold a "no idle sleep" assertion while recording.
    pub prevent_sleep: bool,
    /// Skip the free-disk-space pre-check (tests).
    pub skip_disk_check: bool,
}

impl RecorderConfig {
    /// Both tracks on their default devices, sensible defaults for everything else.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            mic: Some(DeviceSel::Default),
            system: true,
            silence: SilenceConfig::default(),
            frame_len: 512,
            frame_queue: 512,
            prevent_sleep: true,
            skip_disk_check: false,
        }
    }
}

/// Things the UI / engine should know about while recording.
#[derive(Debug, Clone, PartialEq)]
pub enum RecorderEvent {
    /// A track's capture stream opened (first time).
    TrackStarted { track: TrackKind, device: String },
    /// A track's stream was rebuilt on another device (or after an error); recording continues.
    DeviceChanged { track: TrackKind, device: String },
    /// The track has been below the silence threshold for `silent_for_ms` (re-raised each period).
    TrackSilent {
        track: TrackKind,
        silent_for_ms: u64,
    },
    /// The track has left silence after a `TrackSilent` warning.
    TrackActive { track: TrackKind },
    /// The track has delivered *exact zeros* since the start of the recording. On macOS this is how a
    /// missing system-audio / microphone permission looks, but it also happens when nothing plays.
    PermissionSuspected {
        track: TrackKind,
        permission: PermissionKind,
        silent_for_ms: u64,
    },
    /// Capture failed (will keep retrying) or the writer hit an I/O error.
    Error { track: TrackKind, message: String },
    /// The capture callback outran the worker and `dropped_samples` native samples were lost.
    Overrun {
        track: TrackKind,
        dropped_samples: u64,
    },
    /// The live-frame consumer lags; `total` frames have been dropped so far on this track.
    FramesDropped { track: TrackKind, total: u64 },
    /// Recording paused (nothing is written until resumed).
    Paused,
    /// Recording resumed.
    Resumed,
}

/// A block of 16 kHz mono f32 audio for live VAD/ASR.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioFrame {
    pub track: TrackKind,
    /// Index of `samples[0]` in the written track (`<track>.pcm`), i.e. pauses excluded.
    pub start_sample: u64,
    pub samples: Vec<f32>,
}
