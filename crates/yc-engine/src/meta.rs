//! `session.json`: what the engine knows about an in-progress recording that the audio files do not
//! say. Lives in `<app data>/recording/<id>/` until the meeting is finalised.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use yc_core::meeting::{Bookmark, MeetingId, ParticipantHint};
use yc_core::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// Live capture.
    Recording,
    /// Imported audio/video file.
    Import,
    /// Re-transcription of a stored meeting.
    Retranscribe,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub id: MeetingId,
    pub title: String,
    /// The title was generated, so a summary may replace it.
    pub title_auto: bool,
    pub started_at: DateTime<Utc>,
    pub language: String,
    pub expected_speakers: Option<u8>,
    pub source: Source,
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participant_hints: Vec<ParticipantHint>,
}

pub const SESSION_FILE: &str = "session.json";
pub const LIVE_FILE: &str = "live.jsonl";
pub const ASR_FILE: &str = "asr.json";

impl SessionMeta {
    pub fn read(dir: &Path) -> Result<Self> {
        let bytes = std::fs::read(dir.join(SESSION_FILE))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| Error::Storage(format!("{}: {e}", dir.join(SESSION_FILE).display())))
    }

    /// Atomically (temp file + rename) writes the file.
    pub fn write(&self, dir: &Path) -> Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| Error::Storage(e.to_string()))?;
        write_atomic(&dir.join(SESSION_FILE), &json)
    }
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp: PathBuf = path.with_extension("tmp");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_data()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Records which ASR model produced the live segments (`asr.json`).
pub fn write_asr_model(dir: &Path, model: &str) -> Result<()> {
    write_atomic(
        &dir.join(ASR_FILE),
        serde_json::json!({ "model": model }).to_string().as_bytes(),
    )
}

pub fn read_asr_model(dir: &Path) -> Option<String> {
    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join(ASR_FILE)).ok()?).ok()?;
    v.get("model")?.as_str().map(str::to_owned)
}
