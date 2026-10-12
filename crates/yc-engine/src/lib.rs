//! YadaCall orchestration: recording sessions, live transcription, the post-call job pipeline and
//! the queries the UI needs.
//!
//! ```text
//!  RecorderBackend ─ frames ─▶ Feeder (VAD) ─ regions ─▶ AsrQueue ─▶ AsrWorker ─▶ live.jsonl
//!        │ (mic.pcm, them.pcm)                                             │
//!        ▼                                                                 ▼
//!   stop ─▶ drain ─▶ catch_up (gaps, recovery tail) ─▶ finalize ─▶ meeting files + jobs
//!                                    jobs: diarize → summarize → archive → index
//! ```
//!
//! [`Engine`] is the only entry point; the adapters it needs are the traits in [`ports`], and
//! [`testing`] has deterministic fakes of all of them.

mod asr;
mod audio;
mod engine;
mod finalize;
mod jobs;
mod livelog;
mod meta;
mod offline;
mod participants;
mod queries;
mod recorder;
mod recover;
mod regions;
mod retention;
mod session;
mod stages;
mod voices;

pub mod diarize;
pub mod ports;
pub mod settings;
pub mod testing;
pub mod timeline;

pub use engine::{Engine, EngineConfig};
pub use jobs::{JOB_KINDS, JobConfig, JobPayload};
pub use meta::Source;
pub use queries::{ExportFormat, MeetingDetail};
pub use recorder::YadaRecorder;
pub use recover::{ImportOptions, PendingRecovery};
pub use session::{MicChoice, RecordingInfo, StartOptions};
pub use settings::Settings;
pub use yc_core::meeting::ParticipantHint;
