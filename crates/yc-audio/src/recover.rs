//! Crash recovery: rebuild track summaries from whatever reached the disk.

use std::fs::{self, OpenOptions};
use std::path::Path;

use yc_core::meeting::{TrackInfo, TrackKind};
use yc_core::{Error, Result};

use crate::writer::{TrackMeta, meta_path, pcm_path, read_meta, write_meta};

/// Inspects a recording directory after a crash (or a clean stop) and returns one [`TrackInfo`] per
/// track that has audio on disk.
///
/// The `.pcm` length is the truth: a trailing half sample from a torn write is truncated, the sample
/// count is recomputed, and `<track>.json` is rewritten with `recovered: true`. A missing or corrupt
/// json falls back to `device = "unknown"`, `offset = 0`.
pub fn recover(dir: &Path) -> Result<Vec<TrackInfo>> {
    let mut out = Vec::new();
    for kind in TrackKind::ALL {
        let pcm = pcm_path(dir, kind);
        let len = match fs::metadata(&pcm) {
            Ok(m) => m.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(Error::Io(e)),
        };
        let whole = len & !1;
        if whole != len {
            OpenOptions::new().write(true).open(&pcm)?.set_len(whole)?;
        }
        let mut meta = read_meta(&meta_path(dir, kind))
            .unwrap_or_else(|_| TrackMeta::new(kind, "unknown", 0, 0));
        let samples = whole / 2;
        if !meta.clean_stop || meta.samples != samples {
            meta.samples = samples;
            meta.recovered = !meta.clean_stop;
            meta.clean_stop = true;
            write_meta(&meta_path(dir, kind), &meta)?;
        }
        out.push(meta.track_info());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::time::Instant;

    use super::*;
    use crate::writer::TrackWriter;

    #[test]
    fn recovers_after_simulated_crash_with_torn_sample() {
        let dir = tempfile::tempdir().unwrap();
        let meta = TrackMeta::new(TrackKind::System, "Speakers", 1_700_000_000_000, 40);
        let mut w = TrackWriter::create(dir.path(), meta, Instant::now()).unwrap();
        w.write(&vec![0.25; 32_000]).unwrap();
        drop(w); // crash: no finish(), json still says cleanStop=false, samples=0

        // Torn write: one stray byte of a half-written sample.
        let pcm = pcm_path(dir.path(), TrackKind::System);
        OpenOptions::new()
            .append(true)
            .open(&pcm)
            .unwrap()
            .write_all(&[0x7f])
            .unwrap();
        assert_eq!(fs::metadata(&pcm).unwrap().len(), 64_001);

        let tracks = recover(dir.path()).unwrap();
        assert_eq!(tracks.len(), 1, "mic.pcm does not exist");
        let t = &tracks[0];
        assert_eq!(
            (t.kind, t.samples, t.offset_ms),
            (TrackKind::System, 32_000, 40)
        );
        assert_eq!(t.device, "Speakers");
        assert_eq!(
            fs::metadata(&pcm).unwrap().len(),
            64_000,
            "odd byte truncated"
        );
        let meta = read_meta(&meta_path(dir.path(), TrackKind::System)).unwrap();
        assert!(meta.clean_stop && meta.recovered);

        // Idempotent.
        assert_eq!(recover(dir.path()).unwrap(), tracks);
    }

    #[test]
    fn recovers_without_json() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(pcm_path(dir.path(), TrackKind::Mic), vec![0u8; 3200]).unwrap();
        let tracks = recover(dir.path()).unwrap();
        assert_eq!(tracks[0].samples, 1600);
        assert_eq!(tracks[0].device, "unknown");
    }
}
