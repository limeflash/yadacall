//! `live.jsonl`: the append-only, crash-safe record of what live transcription produced.
//!
//! One JSON object per line. A torn last line (crash mid-write) is ignored on read. Segment times
//! are **file positions** of the track (pauses not yet re-inserted), see [`crate::timeline`].

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use yc_core::meeting::TrackKind;
use yc_core::transcript::Segment;
use yc_core::{Error, Result};

const SYNC_EVERY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum LiveLine {
    /// A recognised stretch; `segment.track` says which file, times are file positions.
    Segment { segment: Segment },
    /// Samples of `track` that live processing missed (input channel overflow); the finalize pass
    /// transcribes them offline.
    Gap {
        track: TrackKind,
        start_sample: u64,
        end_sample: u64,
    },
    /// The offline pass processed a [`LiveLine::Gap`] with exactly these bounds.
    Filled {
        track: TrackKind,
        start_sample: u64,
        end_sample: u64,
    },
}

/// Gaps that were logged and not yet filled, per track, merged and sorted.
pub fn open_gaps(lines: &[LiveLine]) -> Vec<(TrackKind, u64, u64)> {
    let mut gaps: Vec<(TrackKind, u64, u64)> = Vec::new();
    for l in lines {
        if let LiveLine::Gap { track, start_sample, end_sample } = l
            && !lines.iter().any(|f| matches!(f, LiveLine::Filled { track: t, start_sample: s, end_sample: e } if t == track && s == start_sample && e == end_sample))
        {
            gaps.push((*track, *start_sample, *end_sample));
        }
    }
    gaps.sort_by_key(|g| (g.0 as u8, g.1));
    let mut merged: Vec<(TrackKind, u64, u64)> = Vec::new();
    for g in gaps {
        match merged.last_mut() {
            Some(last) if last.0 == g.0 && g.1 <= last.2 => last.2 = last.2.max(g.2),
            _ => merged.push(g),
        }
    }
    merged
}

/// Appender shared by the ASR worker and the feeders.
pub struct LiveLog {
    inner: Mutex<Inner>,
}

struct Inner {
    file: File,
    last_sync: Instant,
    dirty: bool,
}

impl LiveLog {
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(path)?;
        // A crash can leave a torn last line without a newline; start the next record on a new line.
        let len = file.metadata()?.len();
        if len > 0 {
            use std::io::{Read, Seek, SeekFrom};
            file.seek(SeekFrom::Start(len - 1))?;
            let mut last = [0u8; 1];
            file.read_exact(&mut last)?;
            if last[0] != b'\n' {
                file.write_all(b"\n")?;
            }
        }
        Ok(Self {
            inner: Mutex::new(Inner {
                file,
                last_sync: Instant::now(),
                dirty: false,
            }),
        })
    }

    pub fn append(&self, line: &LiveLine) -> Result<()> {
        let mut json =
            serde_json::to_vec(line).map_err(|e| Error::Storage(format!("live line: {e}")))?;
        json.push(b'\n');
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| Error::Storage("live log poisoned".into()))?;
        inner.file.write_all(&json)?;
        inner.dirty = true;
        if inner.last_sync.elapsed() >= SYNC_EVERY {
            inner.file.sync_data()?;
            inner.last_sync = Instant::now();
            inner.dirty = false;
        }
        Ok(())
    }

    /// Forces the data to disk.
    pub fn sync(&self) -> Result<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| Error::Storage("live log poisoned".into()))?;
        if inner.dirty {
            inner.file.sync_data()?;
            inner.dirty = false;
            inner.last_sync = Instant::now();
        }
        Ok(())
    }
}

/// Reads every intact line; missing file = empty.
pub fn read_live(path: &Path) -> Result<Vec<LiveLine>> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<LiveLine>(&line) {
            Ok(l) => out.push(l),
            Err(e) => tracing::warn!("ignoring unreadable live.jsonl line: {e}"),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: u64, text: &str) -> LiveLine {
        LiveLine::Segment {
            segment: Segment {
                start_ms: start,
                end_ms: start + 500,
                track: TrackKind::Mic,
                speaker: "me".into(),
                text: text.into(),
                words: vec![],
                confidence: None,
            },
        }
    }

    #[test]
    fn append_read_and_tolerate_torn_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.jsonl");
        let log = LiveLog::open(&path).unwrap();
        log.append(&seg(0, "один")).unwrap();
        log.append(&LiveLine::Gap {
            track: TrackKind::System,
            start_sample: 10,
            end_sample: 20,
        })
        .unwrap();
        log.sync().unwrap();
        drop(log);
        // Torn write at the end.
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(br#"{"kind":"segment","segment":{"startM"#)
            .unwrap();
        let lines = read_live(&path).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], seg(0, "один"));
        assert!(read_live(&dir.path().join("none")).unwrap().is_empty());
    }
}
