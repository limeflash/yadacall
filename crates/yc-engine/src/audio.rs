//! Random access to a track's samples, whether it still is a raw `.pcm` (recording folder or a
//! freshly finalised meeting) or has been archived as FLAC.

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use yc_core::meeting::TrackKind;
use yc_core::{Error, Result};

/// Reads s16le blocks of a track as f32.
pub struct TrackAudio {
    file: File,
    samples: u64,
    /// Temp file decoded from FLAC, removed on drop.
    temp: Option<PathBuf>,
}

impl TrackAudio {
    /// Opens `<dir>/<track>.pcm`, or decodes `<dir>/<track>.flac` into a temp file under `tmp_dir`.
    /// `Ok(None)` when the track has no audio there.
    pub fn open(dir: &Path, kind: TrackKind, tmp_dir: &Path) -> Result<Option<Self>> {
        let pcm = yc_audio::writer::pcm_path(dir, kind);
        if pcm.is_file() {
            return Self::from_pcm(pcm, None).map(Some);
        }
        let flac = dir.join(format!("{}.flac", kind.file_stem()));
        if !flac.is_file() {
            return Ok(None);
        }
        std::fs::create_dir_all(tmp_dir)?;
        let tmp = tmp_dir.join(format!("decode-{}-{}.pcm", std::process::id(), unique()));
        let mut out = BufWriter::new(File::create(&tmp)?);
        let result = yc_audio::decode_to_16k_mono(&flac, &mut |block| {
            write_s16(&mut out, block)?;
            Ok(())
        });
        let finish = result.and_then(|_| out.flush().map_err(Error::Io));
        if let Err(e) = finish {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        drop(out);
        Self::from_pcm(tmp.clone(), Some(tmp)).map(Some)
    }

    fn from_pcm(path: PathBuf, temp: Option<PathBuf>) -> Result<Self> {
        let file = File::open(&path)?;
        let samples = file.metadata()?.len() / 2;
        Ok(Self {
            file,
            samples,
            temp,
        })
    }

    /// Total samples (16 kHz).
    pub fn samples(&self) -> u64 {
        self.samples
    }

    /// Reads up to `len` samples starting at `start` (shorter at the end of the track).
    pub fn read(&mut self, start: u64, len: usize) -> Result<Vec<f32>> {
        read_pcm_block(&mut self.file, start, len)
    }
}

impl Drop for TrackAudio {
    fn drop(&mut self) {
        if let Some(t) = self.temp.take() {
            let _ = std::fs::remove_file(t);
        }
    }
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

/// Reads and converts `len` samples at sample index `start` of an s16le file.
pub fn read_pcm_block(file: &mut File, start: u64, len: usize) -> Result<Vec<f32>> {
    file.seek(SeekFrom::Start(start * 2))?;
    let mut bytes = Vec::with_capacity(len * 2);
    file.take(len as u64 * 2).read_to_end(&mut bytes)?;
    Ok(bytes
        .chunks_exact(2)
        .map(|c| f32::from(i16::from_le_bytes([c[0], c[1]])) / 32768.0)
        .collect())
}

/// Appends f32 samples as s16le.
pub fn write_s16(out: &mut impl Write, block: &[f32]) -> Result<()> {
    let mut bytes = Vec::with_capacity(block.len() * 2);
    for &s in block {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    out.write_all(&bytes)?;
    Ok(())
}
