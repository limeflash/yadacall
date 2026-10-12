//! Decoding of arbitrary audio/video files (mp3, m4a/aac, ogg, flac, wav, mkv, mp4, …) into the
//! pipeline format (16 kHz mono f32), streamed in blocks so memory stays bounded for long files.

use std::fs::File;
use std::path::Path;

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use yc_core::{Error, Result, SAMPLE_RATE, samples_to_ms};

use crate::resample::Resampler16k;

/// What [`decode_to_16k_mono`] produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeInfo {
    /// Samples delivered to the sink at 16 kHz.
    pub samples: u64,
    /// `samples` expressed in milliseconds.
    pub duration_ms: u64,
}

/// Decodes the first audio track of `path` and hands 16 kHz mono f32 blocks to `sink` in order.
///
/// Multi-channel audio is averaged to mono. A truncated file yields what could be decoded; a file
/// with no decodable audio is an [`Error::Invalid`].
pub fn decode_to_16k_mono(
    path: &Path,
    sink: &mut dyn FnMut(&[f32]) -> Result<()>,
) -> Result<DecodeInfo> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| Error::Invalid(format!("{}: unsupported media: {e}", path.display())))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| Error::Invalid(format!("{}: no audio track", path.display())))?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| Error::Invalid(format!("{}: no audio codec parameters", path.display())))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| Error::Invalid(format!("{}: unsupported codec: {e}", path.display())))?;

    let mut interleaved: Vec<f32> = Vec::new();
    let mut mono: Vec<f32> = Vec::new();
    let mut out: Vec<f32> = Vec::new();
    let mut resampler: Option<(Resampler16k, u32)> = None;
    let mut input_frames: u64 = 0;
    let mut delivered: u64 = 0;

    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => {
                if delivered == 0 && input_frames == 0 {
                    return Err(Error::Invalid(format!("{}: {e}", path.display())));
                }
                tracing::warn!("{}: stopped decoding: {e}", path.display());
                break;
            }
        };
        if packet.track_id != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(b) => b,
            Err(SymError::DecodeError(_)) => continue,
            Err(SymError::IoError(_)) => continue,
            Err(e) => {
                tracing::warn!("{}: decoder error: {e}", path.display());
                break;
            }
        };
        let channels = buf.spec().channels().count().max(1);
        let rate = buf.spec().rate();
        interleaved.resize(buf.samples_interleaved(), 0.0);
        buf.copy_to_slice_interleaved(&mut interleaved[..]);
        mono.clear();
        if channels == 1 {
            mono.extend_from_slice(&interleaved);
        } else {
            let inv = 1.0 / channels as f32;
            mono.extend(
                interleaved
                    .chunks_exact(channels)
                    .map(|frame| frame.iter().sum::<f32>() * inv),
            );
        }
        input_frames += mono.len() as u64;
        if resampler.as_ref().is_none_or(|(_, r)| *r != rate) {
            resampler = Some((Resampler16k::new(rate)?, rate));
        }
        let (rs, _) = resampler
            .as_mut()
            .ok_or_else(|| Error::Storage("resampler".into()))?;
        out.clear();
        rs.process(&mono, &mut out)?;
        if !out.is_empty() {
            delivered += out.len() as u64;
            sink(&out)?;
        }
    }

    if let Some((rs, rate)) = resampler.as_mut() {
        // Flush the resampler's tail with silence, then trim to the exact expected length.
        out.clear();
        rs.process(&vec![0.0; 4096], &mut out)?;
        let expected = input_frames * u64::from(SAMPLE_RATE) / u64::from(*rate);
        let missing = expected.saturating_sub(delivered) as usize;
        if missing > 0 && !out.is_empty() {
            let take = missing.min(out.len());
            delivered += take as u64;
            sink(&out[..take])?;
        }
    }
    if delivered == 0 {
        return Err(Error::Invalid(format!(
            "{}: no decodable audio",
            path.display()
        )));
    }
    Ok(DecodeInfo {
        samples: delivered,
        duration_ms: samples_to_ms(delivered),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal 16-bit PCM WAV writer for fixtures.
    pub(crate) fn wav_bytes(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data_len).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
        v.extend_from_slice(&(channels * 2).to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            v.extend_from_slice(&s.to_le_bytes());
        }
        v
    }

    fn decode_all(path: &Path) -> (Vec<f32>, DecodeInfo) {
        let mut all = Vec::new();
        let info = decode_to_16k_mono(path, &mut |b| {
            all.extend_from_slice(b);
            Ok(())
        })
        .unwrap();
        (all, info)
    }

    #[test]
    fn decodes_48k_stereo_wav_to_16k_mono() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        // 2 s of a 440 Hz tone, left = right, at 48 kHz stereo.
        let mut s = Vec::new();
        for i in 0..96_000 {
            let v = (0.5
                * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin()
                * 32767.0) as i16;
            s.push(v);
            s.push(v);
        }
        std::fs::write(&path, wav_bytes(48_000, 2, &s)).unwrap();
        let (pcm, info) = decode_all(&path);
        assert_eq!(info.samples as usize, pcm.len());
        assert!((31_000..=32_100).contains(&pcm.len()), "len {}", pcm.len());
        let peak = pcm.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((0.4..0.6).contains(&peak), "peak {peak}");
    }

    #[test]
    fn passes_through_16k_mono_and_rejects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.wav");
        std::fs::write(&path, wav_bytes(16_000, 1, &vec![1000i16; 16_000])).unwrap();
        let (pcm, info) = decode_all(&path);
        assert_eq!(pcm.len(), 16_000);
        assert_eq!(info.duration_ms, 1000);

        let junk = dir.path().join("c.mp3");
        std::fs::write(&junk, b"not audio at all, just text").unwrap();
        let err = decode_to_16k_mono(&junk, &mut |_| Ok(())).unwrap_err();
        assert!(matches!(err, Error::Invalid(_)));
    }
}
