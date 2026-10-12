//! Crash-safe track writer.
//!
//! A track is two files in the recording directory:
//!
//! * `<mic|them>.pcm` — raw s16le mono 16 kHz, append-only. `sync_data` runs at most every 2 s, so a
//!   power loss costs ≤ 2 s; a process crash loses nothing that was already `write`n.
//! * `<mic|them>.json` — [`TrackMeta`]: device, first-sample wall clock, offset to the meeting start,
//!   pause intervals, device changes. Rewritten atomically (temp file + rename) on every state
//!   change, so it is always valid JSON. The sample count in it is only authoritative when
//!   `cleanStop` is true; [`crate::recover`] recomputes it from the `.pcm` length.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use yc_core::meeting::{TrackInfo, TrackKind};
use yc_core::{Error, Result, SAMPLE_RATE, samples_to_ms};

/// How often the `.pcm` file is forced to disk.
pub const SYNC_INTERVAL: Duration = Duration::from_secs(2);

/// A pause: the file contains nothing for it, so the wall-clock time of a sample after the pause is
/// `offset + position + Σ durations of pauses at earlier positions`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PauseSpan {
    /// Position in the track (ms of audio written so far) where the pause happened.
    pub at_ms: u64,
    /// Wall-clock length of the pause.
    pub duration_ms: u64,
}

/// A mid-call device switch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceChange {
    /// Position in the track (ms of audio written so far).
    pub at_ms: u64,
    pub device: String,
}

/// Contents of `<track>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackMeta {
    pub version: u32,
    pub kind: TrackKind,
    pub device: String,
    pub sample_rate: u32,
    pub format: String,
    /// Wall-clock time of the first sample, ms since the Unix epoch.
    pub started_at_unix_ms: u64,
    /// Offset of the first sample from the meeting start (shared anchor for both tracks).
    pub offset_ms: u64,
    /// Samples in the `.pcm` (authoritative only when `clean_stop`).
    pub samples: u64,
    #[serde(default)]
    pub pauses: Vec<PauseSpan>,
    #[serde(default)]
    pub device_changes: Vec<DeviceChange>,
    /// True once the recorder finished the file; false after a crash (see [`crate::recover`]).
    pub clean_stop: bool,
    /// True when [`crate::recover`] reconstructed this metadata from the `.pcm` length.
    #[serde(default)]
    pub recovered: bool,
}

impl TrackMeta {
    /// Fresh metadata for a track that starts now.
    pub fn new(kind: TrackKind, device: &str, started_at_unix_ms: u64, offset_ms: u64) -> Self {
        Self {
            version: 1,
            kind,
            device: device.to_owned(),
            sample_rate: SAMPLE_RATE,
            format: "s16le".to_owned(),
            started_at_unix_ms,
            offset_ms,
            samples: 0,
            pauses: Vec::new(),
            device_changes: Vec::new(),
            clean_stop: false,
            recovered: false,
        }
    }

    /// Summary in the shape the rest of the app uses.
    pub fn track_info(&self) -> TrackInfo {
        TrackInfo {
            kind: self.kind,
            device: self.device.clone(),
            offset_ms: self.offset_ms,
            samples: self.samples,
        }
    }
}

/// `<dir>/<mic|them>.pcm`.
pub fn pcm_path(dir: &Path, kind: TrackKind) -> PathBuf {
    dir.join(format!("{}.pcm", kind.file_stem()))
}

/// `<dir>/<mic|them>.json`.
pub fn meta_path(dir: &Path, kind: TrackKind) -> PathBuf {
    dir.join(format!("{}.json", kind.file_stem()))
}

/// Atomically (re)writes the metadata file.
pub fn write_meta(path: &Path, meta: &TrackMeta) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(meta).map_err(|e| Error::Storage(e.to_string()))?;
    {
        let mut f = File::create(&tmp)?;
        f.write_all(&json)?;
        f.sync_data()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Reads `<track>.json`.
pub fn read_meta(path: &Path) -> Result<TrackMeta> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(|e| Error::Storage(format!("{}: {e}", path.display())))
}

/// Appends 16 kHz mono samples to `<track>.pcm` as s16le.
pub struct TrackWriter {
    pcm: File,
    meta_path: PathBuf,
    meta: TrackMeta,
    samples: u64,
    bytes: Vec<u8>,
    last_sync: Instant,
    unsynced: bool,
    pause_started: Option<Instant>,
}

impl TrackWriter {
    /// Creates `<dir>/<track>.pcm` and `.json`. Refuses to touch an existing recording.
    pub fn create(dir: &Path, meta: TrackMeta, now: Instant) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let pcm_path = pcm_path(dir, meta.kind);
        let meta_path = meta_path(dir, meta.kind);
        let pcm = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pcm_path)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    Error::Invalid(format!(
                        "{} already exists; recording directories are never overwritten",
                        pcm_path.display()
                    ))
                } else {
                    Error::Io(e)
                }
            })?;
        write_meta(&meta_path, &meta)?;
        Ok(Self {
            pcm,
            meta_path,
            meta,
            samples: 0,
            bytes: Vec::with_capacity(8192),
            last_sync: now,
            unsynced: false,
            pause_started: None,
        })
    }

    /// Samples written so far.
    pub fn samples(&self) -> u64 {
        self.samples
    }

    /// Converts and appends `block`. Ignored while paused.
    pub fn write(&mut self, block: &[f32]) -> Result<()> {
        if self.pause_started.is_some() || block.is_empty() {
            return Ok(());
        }
        self.bytes.clear();
        for &s in block {
            let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            self.bytes.extend_from_slice(&v.to_le_bytes());
        }
        self.pcm.write_all(&self.bytes)?;
        self.samples += block.len() as u64;
        self.unsynced = true;
        Ok(())
    }

    /// Forces the data to disk if the last sync is older than `every`.
    pub fn sync_if_due(&mut self, now: Instant, every: Duration) -> Result<()> {
        if self.unsynced && now.duration_since(self.last_sync) >= every {
            self.pcm.sync_data()?;
            self.last_sync = now;
            self.unsynced = false;
        }
        Ok(())
    }

    /// Starts a pause: nothing is written until [`Self::end_pause`].
    pub fn begin_pause(&mut self, now: Instant) {
        if self.pause_started.is_none() {
            self.pause_started = Some(now);
        }
    }

    /// Ends a pause and records it in the metadata.
    pub fn end_pause(&mut self, now: Instant) -> Result<()> {
        if let Some(start) = self.pause_started.take() {
            self.meta.pauses.push(PauseSpan {
                at_ms: samples_to_ms(self.samples),
                duration_ms: now.duration_since(start).as_millis() as u64,
            });
            write_meta(&self.meta_path, &self.meta)?;
        }
        Ok(())
    }

    /// Records a device switch (the track continues in the same file).
    pub fn note_device_change(&mut self, device: &str) -> Result<()> {
        self.meta.device_changes.push(DeviceChange {
            at_ms: samples_to_ms(self.samples),
            device: device.to_owned(),
        });
        write_meta(&self.meta_path, &self.meta)
    }

    /// Syncs the data, writes the final metadata and returns the track summary.
    pub fn finish(mut self, now: Instant) -> Result<TrackInfo> {
        self.end_pause(now)?;
        self.pcm.sync_all()?;
        self.meta.samples = self.samples;
        self.meta.clean_stop = true;
        write_meta(&self.meta_path, &self.meta)?;
        Ok(self.meta.track_info())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn writer(dir: &Path) -> TrackWriter {
        let meta = TrackMeta::new(TrackKind::Mic, "Test Mic", 1_700_000_000_000, 120);
        TrackWriter::create(dir, meta, Instant::now()).unwrap()
    }

    #[test]
    fn writes_s16le_and_final_meta() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = writer(dir.path());
        w.write(&[0.0, 0.5, -0.5, 1.0, -1.0, 2.0]).unwrap();
        let info = w.finish(Instant::now()).unwrap();
        assert_eq!(info.samples, 6);
        assert_eq!(info.offset_ms, 120);
        let raw = fs::read(pcm_path(dir.path(), TrackKind::Mic)).unwrap();
        let s: Vec<i16> = raw
            .chunks(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(s, vec![0, 16384, -16384, 32767, -32767, 32767]);
        let meta = read_meta(&meta_path(dir.path(), TrackKind::Mic)).unwrap();
        assert!(meta.clean_stop);
        assert_eq!(meta.samples, 6);
        assert_eq!(meta.device, "Test Mic");
    }

    #[test]
    fn pause_writes_nothing_and_is_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = writer(dir.path());
        let t0 = Instant::now();
        w.write(&vec![0.1; 16_000]).unwrap();
        w.begin_pause(t0);
        w.write(&vec![0.1; 16_000]).unwrap();
        w.end_pause(t0 + Duration::from_millis(1500)).unwrap();
        w.write(&vec![0.1; 8_000]).unwrap();
        let info = w.finish(Instant::now()).unwrap();
        assert_eq!(info.samples, 24_000);
        let meta = read_meta(&meta_path(dir.path(), TrackKind::Mic)).unwrap();
        assert_eq!(
            meta.pauses,
            vec![PauseSpan {
                at_ms: 1000,
                duration_ms: 1500
            }]
        );
    }

    #[test]
    fn sync_is_rate_limited_and_refuses_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = writer(dir.path());
        let t0 = Instant::now();
        w.write(&[0.1; 100]).unwrap();
        w.sync_if_due(t0 + Duration::from_millis(500), SYNC_INTERVAL)
            .unwrap();
        assert!(w.unsynced, "too early to sync");
        w.sync_if_due(t0 + Duration::from_millis(2100), SYNC_INTERVAL)
            .unwrap();
        assert!(!w.unsynced);
        let meta = TrackMeta::new(TrackKind::Mic, "x", 0, 0);
        assert!(matches!(
            TrackWriter::create(dir.path(), meta, t0),
            Err(Error::Invalid(_))
        ));
    }
}
