//! Domain types, port traits and engine events shared by every YadaCall crate.
//!
//! This crate is deliberately dependency-light and free of I/O: it defines *what* the app talks
//! about (meetings, tracks, segments, summaries) and the traits adapters implement (speech
//! recognition, VAD, embeddings). Wiring lives in `yc-engine`.

pub mod error;
pub mod events;
pub mod meeting;
pub mod ports;
pub mod summary;
pub mod transcript;

pub use error::{Error, Result};

/// All audio inside the pipeline is mono f32 PCM at this rate (what VAD and ASR models expect).
pub const SAMPLE_RATE: u32 = 16_000;

/// Milliseconds since the start of a meeting (the common timeline of both tracks).
pub type Ms = u64;

/// Converts a sample index at [`SAMPLE_RATE`] to milliseconds.
pub const fn samples_to_ms(samples: u64) -> Ms {
    samples * 1000 / SAMPLE_RATE as u64
}

/// Converts milliseconds to a sample index at [`SAMPLE_RATE`].
pub const fn ms_to_samples(ms: Ms) -> u64 {
    ms * SAMPLE_RATE as u64 / 1000
}
