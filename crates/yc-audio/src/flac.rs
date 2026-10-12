//! Pure-Rust FLAC encoding of a finished track (for retention: FLAC is ~½ the size of raw PCM).

use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::Path;

use flacenc::bitsink::ByteSink;
use flacenc::component::BitRepr;
use flacenc::config::Encoder;
use flacenc::error::{SourceError, Verify};
use flacenc::source::{Fill, Source};
use yc_core::{Error, Result, SAMPLE_RATE};

/// Streams an s16le mono `.pcm` file into the encoder without expanding it to i32 in memory.
struct PcmFileSource {
    reader: BufReader<File>,
    bytes: Vec<u8>,
    total: usize,
}

impl Source for PcmFileSource {
    fn channels(&self) -> usize {
        1
    }
    fn bits_per_sample(&self) -> usize {
        16
    }
    fn sample_rate(&self) -> usize {
        SAMPLE_RATE as usize
    }
    fn read_samples<F: Fill>(
        &mut self,
        block_size: usize,
        dest: &mut F,
    ) -> std::result::Result<usize, SourceError> {
        self.bytes.resize(block_size * 2, 0);
        let mut got = 0;
        while got < self.bytes.len() {
            let n = self
                .reader
                .read(&mut self.bytes[got..])
                .map_err(SourceError::from_io_error)?;
            if n == 0 {
                break;
            }
            got += n;
        }
        let whole = got & !1;
        dest.fill_le_bytes(&self.bytes[..whole], 2)?;
        Ok(whole / 2)
    }
    fn len_hint(&self) -> Option<usize> {
        Some(self.total)
    }
}

/// Encodes `<pcm>` (s16le mono 16 kHz) to a FLAC file at `out`. Writes to a temp file first and
/// renames, so `out` is never half-written. Returns the FLAC size in bytes.
pub fn encode_flac(pcm: &Path, out: &Path) -> Result<u64> {
    let len = fs::metadata(pcm)?.len();
    let source = PcmFileSource {
        reader: BufReader::new(File::open(pcm)?),
        bytes: Vec::new(),
        total: (len / 2) as usize,
    };
    let config = Encoder::default()
        .into_verified()
        .map_err(|e| Error::Invalid(format!("flac config: {e:?}")))?;
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| Error::Storage(format!("flac encode: {e:?}")))?;
    let mut sink = ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| Error::Storage(format!("flac write: {e:?}")))?;
    let tmp = out.with_extension("flac.tmp");
    fs::write(&tmp, sink.as_slice())?;
    fs::rename(&tmp, out)?;
    Ok(sink.as_slice().len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flac_round_trip_is_lossless_and_smaller() {
        let dir = tempfile::tempdir().unwrap();
        // 3 s of two tones plus a deterministic noise burst, as s16.
        let mut seed = 12345u32;
        let samples: Vec<i16> = (0..48_000usize)
            .map(|i| {
                let t = i as f32 / 16_000.0;
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = ((seed >> 16) as f32 / 65536.0 - 0.5) * 0.02;
                let v = 0.4 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                    + 0.2 * (2.0 * std::f32::consts::PI * 2500.0 * t).sin()
                    + noise;
                (v * 32767.0) as i16
            })
            .collect();
        let pcm: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let pcm_path = dir.path().join("t.pcm");
        let flac_path = dir.path().join("t.flac");
        fs::write(&pcm_path, &pcm).unwrap();

        let size = encode_flac(&pcm_path, &flac_path).unwrap();
        assert!(size < pcm.len() as u64 * 3 / 4, "{size} vs {}", pcm.len());

        let mut reader = claxon::FlacReader::open(&flac_path).unwrap();
        let info = reader.streaminfo();
        assert_eq!(
            (info.sample_rate, info.channels, info.bits_per_sample),
            (16_000, 1, 16)
        );
        let decoded: Vec<i16> = reader.samples().map(|s| s.unwrap() as i16).collect();
        assert_eq!(decoded, samples, "bit-exact round trip");
    }

    #[test]
    fn handles_non_block_multiple_and_empty() {
        let dir = tempfile::tempdir().unwrap();
        let pcm_path = dir.path().join("t.pcm");
        let samples: Vec<i16> = (0..5000).map(|i| ((i * 7) % 2000 - 1000) as i16).collect();
        fs::write(
            &pcm_path,
            samples
                .iter()
                .flat_map(|s| s.to_le_bytes())
                .collect::<Vec<u8>>(),
        )
        .unwrap();
        let out = dir.path().join("t.flac");
        encode_flac(&pcm_path, &out).unwrap();
        let decoded: Vec<i16> = claxon::FlacReader::open(&out)
            .unwrap()
            .samples()
            .map(|s| s.unwrap() as i16)
            .collect();
        assert_eq!(decoded, samples);
    }
}
