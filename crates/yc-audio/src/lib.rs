//! Two-track call capture: microphone ("me") and system audio ("them").
//!
//! ```text
//! capture callback ─ rtrb ring ─▶ worker: mono → 16 kHz ─┬─ level meter (25 Hz, watch channel)
//!  (no alloc, no locks)                                    ├─ silence watchdog (−50 dBFS / 60 s)
//!                                                          ├─ <track>.pcm  (s16le, sync ≤ every 2 s)
//!                                                          └─ live frames  (bounded, drops when full)
//! ```
//!
//! Entry points: [`Recorder::start`] → [`RecorderHandle`], [`recover`] after a crash,
//! [`encode_flac`] for retention, [`prevent_sleep`] and [`disk`] helpers, [`devices`] for the UI.

pub mod config;
pub mod devices;
pub mod disk;
pub mod flac;
pub mod meter;
pub mod recorder;
pub mod recover;
pub mod resample;
pub mod sleep;
pub mod watchdog;
pub mod writer;

mod capture;
mod ingest;
mod pipeline;
mod supervisor;

pub use config::{AudioFrame, DeviceSel, RecorderConfig, RecorderEvent, SilenceConfig};
pub use devices::{
    DeviceInfo, input_is_bluetooth, list_input_devices, list_output_devices,
    output_is_builtin_speakers, system_audio_support,
};
pub use flac::encode_flac;
pub use pipeline::LEVEL_HZ;
pub use recorder::{Levels, Recorder, RecorderHandle};
pub use recover::recover;
pub use sleep::{SleepGuard, prevent_sleep};
