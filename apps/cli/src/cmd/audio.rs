//! Common-file decoding, channel mixing and 16 kHz resampling for CLI audio.

use std::{fs::File, io::Cursor, path::Path};

use anyhow::{Context, anyhow, bail};
use rubato::{
    Fft, FixedSync, Resampler, audioadapter::Adapter,
    audioadapter_buffers::direct::SequentialSliceOfVecs,
};
use symphonia::core::{
    codecs::audio::AudioDecoderOptions,
    errors::Error as SymphoniaError,
    formats::{FormatOptions, TrackType, probe::Hint},
    io::MediaSourceStream,
    meta::MetadataOptions,
};

const OUTPUT_SAMPLE_RATE: usize = 16_000;

/// Fully decoded, mono f32 audio at 16 kHz.
pub struct AudioData {
    pub pcm: Vec<f32>,
}

impl AudioData {
    pub fn duration_seconds(&self) -> f64 {
        self.pcm.len() as f64 / OUTPUT_SAMPLE_RATE as f64
    }
}

/// Decodes a file supported by Symphonia and converts it to mono 16 kHz PCM.
pub fn decode_file(path: &Path) -> anyhow::Result<AudioData> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    decode_source(Box::new(file), extension)
}

/// Decodes an in-memory audio fixture with the given file extension.
pub fn decode_bytes(bytes: &[u8], extension: &str) -> anyhow::Result<AudioData> {
    decode_source(Box::new(Cursor::new(bytes.to_vec())), extension)
}

fn decode_source(
    source: Box<dyn symphonia::core::io::MediaSource>,
    extension: &str,
) -> anyhow::Result<AudioData> {
    let stream = MediaSourceStream::new(source, Default::default());
    let mut hint = Hint::new();
    if !extension.is_empty() {
        hint.with_extension(extension);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .context("unsupported or invalid audio format")?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| anyhow!("audio file contains no audio track"))?;
    let track_id = track.id;
    let num_frames = track.num_frames;
    let track_duration = track.duration;
    let track_time_base = track.time_base;
    let codec_parameters = track
        .codec_params
        .as_ref()
        .context("missing audio codec parameters")?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(
            codec_parameters
                .audio()
                .context("missing audio codec parameters")?,
            &AudioDecoderOptions::default(),
        )
        .context("unsupported audio codec")?;

    let mut mono = Vec::new();
    let mut input_sample_rate = None;
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymphoniaError::ResetRequired) => bail!("chained audio streams are not supported"),
            Err(error) => return Err(anyhow!(error).context("failed to read audio packet")),
        };
        if packet.track_id != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(error) => return Err(anyhow!(error).context("failed to decode audio packet")),
        };
        let sample_rate = decoded.spec().rate();
        if let Some(previous) = input_sample_rate
            && previous != sample_rate
        {
            bail!("audio sample rate changed mid-stream from {previous} to {sample_rate}");
        }
        input_sample_rate = Some(sample_rate);
        let channels = decoded.spec().channels().count();
        if channels == 0 {
            bail!("audio decoder returned zero channels");
        }
        let mut interleaved = vec![0.0_f32; decoded.samples_interleaved()];
        decoded.copy_to_slice_interleaved(&mut interleaved);
        for frame in interleaved.chunks_exact(channels) {
            let mono_sample = frame.iter().copied().sum::<f32>() / channels as f32;
            mono.push(mono_sample);
        }
    }
    let input_sample_rate =
        input_sample_rate.ok_or_else(|| anyhow!("audio file decoded no samples"))?;
    let playable_frames = num_frames
        .and_then(|frames| usize::try_from(frames).ok())
        .or_else(|| {
            let duration = track_duration?;
            let time_base = track_time_base?;
            let frames = (duration.get() as u128)
                .checked_mul(time_base.numer.get() as u128)?
                .checked_mul(input_sample_rate as u128)?
                / time_base.denom.get() as u128;
            usize::try_from(frames).ok()
        });
    if let Some(playable_frames) = playable_frames
        && mono.len() > playable_frames
    {
        mono.truncate(playable_frames);
    }
    if mono.is_empty() {
        bail!("audio file decoded no samples");
    }
    let pcm = if input_sample_rate as usize == OUTPUT_SAMPLE_RATE {
        mono
    } else {
        resample(&mono, input_sample_rate as usize)?
    };
    Ok(AudioData { pcm })
}

fn resample(mono: &[f32], input_sample_rate: usize) -> anyhow::Result<Vec<f32>> {
    let mut resampler = Fft::<f32>::new(
        input_sample_rate,
        OUTPUT_SAMPLE_RATE,
        1024,
        1,
        FixedSync::Both,
    )
    .context("failed to configure audio resampler")?;
    let input = vec![mono.to_vec()];
    let input_adapter = SequentialSliceOfVecs::new(&input, 1, mono.len())
        .context("failed to prepare audio resampler input")?;
    let output = resampler
        .process_all(&input_adapter, mono.len(), None)
        .context("audio resampling failed")?;
    let target_frames =
        ((mono.len() as u128 * OUTPUT_SAMPLE_RATE as u128) / input_sample_rate as u128) as usize;
    if output.frames() < target_frames {
        bail!(
            "resampler returned {} frames, expected {target_frames}",
            output.frames()
        );
    }
    (0..target_frames)
        .map(|frame| {
            output
                .read_sample(0, frame)
                .ok_or_else(|| anyhow!("resampler returned incomplete output"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_compressed_m4a_benchmark_audio_and_resamples_to_16khz() {
        let audio = decode_bytes(include_bytes!("../../assets/bench_sample.m4a"), "m4a")
            .expect("decode benchmark clip");
        assert!((audio.pcm.len() as i64 - 480_000).abs() <= 1_000);
        assert!(audio.pcm.iter().all(|sample| sample.is_finite()));
    }
}
