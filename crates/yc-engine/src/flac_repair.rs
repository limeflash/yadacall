//! One-time migration: FLAC files written by the first `encode_flac` carry a STREAMINFO that Core
//! Audio and symphonia reject. `yc_audio::repair_flac` fixes them in place; this walks the library
//! once (guarded by the store setting [`MIGRATION_KEY`], a key of its own so it can never race with
//! the user's settings object) and does that for every `audio/*.flac`.

use std::path::Path;
use std::sync::Arc;

use yc_audio::Repair;
use yc_core::Result;

use crate::engine::Engine;

/// Store setting recording that the migration ran.
const MIGRATION_KEY: &str = "migration.flacStreaminfoRepaired";

/// Outcome of one pass over the library.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RepairReport {
    pub inspected: usize,
    pub repaired: usize,
    pub failed: usize,
}

/// Repairs every `<library>/<meeting>/audio/*.flac`. Failures are logged and counted, never fatal.
pub fn repair_library(library: &Path) -> RepairReport {
    let mut report = RepairReport::default();
    let Ok(meetings) = std::fs::read_dir(library) else {
        return report;
    };
    for meeting in meetings.flatten() {
        let Ok(files) = std::fs::read_dir(meeting.path().join("audio")) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("flac") {
                continue;
            }
            report.inspected += 1;
            match yc_audio::repair_flac(&path) {
                Ok(Repair::Repaired) => {
                    report.repaired += 1;
                    tracing::info!("repaired FLAC header of {}", path.display());
                }
                Ok(Repair::AlreadyValid) => {}
                Err(e) => {
                    report.failed += 1;
                    tracing::warn!("cannot repair {}: {e}", path.display());
                }
            }
        }
    }
    report
}

impl Engine {
    /// Runs [`repair_library`] once per installation and remembers that in the store settings.
    pub(crate) async fn migrate_flac_headers(self: &Arc<Self>) -> Result<RepairReport> {
        let library = self.library_root();
        self.blocking(move |engine| {
            if engine.store.get_setting(MIGRATION_KEY)?.is_some() {
                return Ok(RepairReport::default());
            }
            let report = repair_library(&library);
            if report.repaired > 0 {
                tracing::info!("repaired {} FLAC files", report.repaired);
            }
            engine
                .store
                .set_setting(MIGRATION_KEY, &serde_json::json!(true))?;
            Ok(report)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_missing_library_and_non_flac() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("2026-01-01 10-00 x").join("audio");
        std::fs::create_dir_all(&audio).unwrap();
        std::fs::write(audio.join("mic.pcm"), [0u8; 10]).unwrap();
        std::fs::write(audio.join("bad.flac"), [0u8; 10]).unwrap();
        let r = repair_library(dir.path());
        assert_eq!(
            r,
            RepairReport {
                inspected: 1,
                repaired: 0,
                failed: 1
            }
        );
        assert_eq!(
            repair_library(&dir.path().join("nope")),
            RepairReport::default()
        );
    }

    #[test]
    fn repairs_a_legacy_header_in_the_library() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("2026-01-01 10-00 x").join("audio");
        std::fs::create_dir_all(&audio).unwrap();
        let pcm: Vec<u8> = (0..4096 * 6 + 1973)
            .flat_map(|i: i32| (((i * 37) % 4000 - 2000) as i16).to_le_bytes())
            .collect();
        std::fs::write(audio.join("mic.pcm"), pcm).unwrap();
        let flac = audio.join("mic.flac");
        yc_audio::encode_flac(&audio.join("mic.pcm"), &flac).unwrap();
        // Rewrite the header the way the first encoder left it: min_blocksize = the short tail.
        let mut bytes = std::fs::read(&flac).unwrap();
        bytes[8..10].copy_from_slice(&1973u16.to_be_bytes());
        std::fs::write(&flac, bytes).unwrap();

        let r = repair_library(dir.path());
        assert_eq!(
            r,
            RepairReport {
                inspected: 1,
                repaired: 1,
                failed: 0
            }
        );
        assert_eq!(repair_library(dir.path()).repaired, 0);
    }
}
