//! Free-disk-space pre-check (≈ 250 MB per hour for both tracks).

use std::path::Path;
use std::time::Duration;

use yc_core::{Error, Result};

/// Raw bytes per second of one track: 16 kHz × 2 bytes (s16le mono).
pub const BYTES_PER_SECOND_PER_TRACK: u64 = 32_000;

/// Bytes `tracks` tracks need for `duration`, with 10 % headroom for metadata and FLAC encoding.
/// Both tracks for one hour come out at ≈ 253 MB.
pub fn required_bytes(duration: Duration, tracks: usize) -> u64 {
    let raw = duration.as_secs() * BYTES_PER_SECOND_PER_TRACK * tracks as u64;
    raw + raw / 10
}

/// Free bytes on the volume holding `dir` (which must exist).
pub fn available_bytes(dir: &Path) -> Result<u64> {
    fs4::available_space(dir).map_err(Error::Io)
}

/// Fails with [`Error::DiskFull`] unless `dir`'s volume can hold `duration` of `tracks` tracks.
pub fn check_free_space(dir: &Path, duration: Duration, tracks: usize) -> Result<()> {
    let needed = required_bytes(duration, tracks);
    let available = available_bytes(dir)?;
    if available < needed {
        return Err(Error::DiskFull { needed, available });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_hour_both_tracks_is_about_250_mb() {
        let b = required_bytes(Duration::from_secs(3600), 2);
        assert!((240_000_000..260_000_000).contains(&b), "{b}");
    }

    #[test]
    fn check_reports_disk_full_for_absurd_request() {
        let dir = tempfile::tempdir().unwrap();
        let err = check_free_space(dir.path(), Duration::from_secs(u32::MAX as u64 * 100), 2)
            .unwrap_err();
        assert!(matches!(err, Error::DiskFull { .. }));
        check_free_space(dir.path(), Duration::from_secs(1), 1).unwrap();
    }
}
