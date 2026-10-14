//! Streaming Silero VAD v5 adapter.

use std::path::Path;

use ort::value::Tensor;
use yc_core::{
    Error, Result,
    ports::{ModelInfo, SpeechRegion, VoiceActivityDetector},
};

use crate::{
    Backend,
    backend::{AdaptiveSession, model_cache_dir},
};

const FRAME_SAMPLES: usize = 512;
const CONTEXT_SAMPLES: usize = 64;
const STATE_VALUES: usize = 2 * 128;
const SPEECH_THRESHOLD: f32 = 0.5;
const NEGATIVE_THRESHOLD: f32 = 0.35;
const MIN_SPEECH_SAMPLES: u64 = 4_000;
const MIN_SILENCE_SAMPLES: u64 = 8_000;
const SPEECH_PAD_SAMPLES: u64 = 3_200;
const MAX_REGION_SAMPLES: u64 = 320_000;
const MAX_SPLIT_LOOKBACK_SAMPLES: u64 = 80_000;

const SILERO_V5: &[u8] = include_bytes!("silero_vad/data/silero_vad.onnx");

/// Silero v5 streaming detector with YadaCall's region parameters.
pub struct SileroVad {
    session: AdaptiveSession,
    info: ModelInfo,
    frame_buffer: Vec<f32>,
    context: [f32; CONTEXT_SAMPLES],
    state: Vec<f32>,
    samples_received: u64,
    samples_processed: u64,
    active_start: Option<u64>,
    silence_start: Option<u64>,
    recent_probabilities: Vec<(u64, f32)>,
}

impl SileroVad {
    /// Loads the bundled Silero v5 ONNX model.
    pub fn load(backend: Backend, model_dir: &Path) -> Result<Self> {
        let session = AdaptiveSession::from_memory(
            SILERO_V5,
            backend,
            &model_cache_dir(model_dir, "silero-vad-v5"),
        )?;
        let info = ModelInfo {
            id: "silero-vad-v5".to_owned(),
            backend: session.backend().to_string(),
        };
        Ok(Self {
            session,
            info,
            frame_buffer: Vec::with_capacity(FRAME_SAMPLES * 2),
            context: [0.0; CONTEXT_SAMPLES],
            state: vec![0.0; STATE_VALUES],
            samples_received: 0,
            samples_processed: 0,
            active_start: None,
            silence_start: None,
            recent_probabilities: Vec::new(),
        })
    }

    fn process_frame(
        &mut self,
        frame: &[f32; FRAME_SAMPLES],
        regions: &mut Vec<SpeechRegion>,
    ) -> Result<()> {
        let start = self.samples_processed;
        let mut model_input = Vec::with_capacity(CONTEXT_SAMPLES + FRAME_SAMPLES);
        model_input.extend_from_slice(&self.context);
        model_input.extend_from_slice(frame);
        let state = self.state.clone();
        let input = model_input.clone();
        let (probability, new_state) = self.session.run_mapped(|session| {
            let audio = Tensor::from_array(([1_usize, input.len()], input.clone()))
                .map_err(|error| model_error("silero-vad-v5", error))?;
            let state = Tensor::from_array(([2_usize, 1, 128], state.clone()))
                .map_err(|error| model_error("silero-vad-v5", error))?;
            let sample_rate = Tensor::from_array(([] as [usize; 0], vec![16_000_i64]))
                .map_err(|error| model_error("silero-vad-v5", error))?;
            let outputs = session
                .run(ort::inputs!["input" => audio, "state" => state, "sr" => sample_rate])
                .map_err(|error| model_error("silero-vad-v5", error))?;
            let (_, probabilities) = outputs["output"]
                .try_extract_tensor::<f32>()
                .map_err(|error| model_error("silero-vad-v5", error))?;
            let (_, next_state) = outputs["stateN"]
                .try_extract_tensor::<f32>()
                .map_err(|error| model_error("silero-vad-v5", error))?;
            let probability = probabilities.first().copied().ok_or_else(|| Error::Model {
                model: "silero-vad-v5".to_owned(),
                message: "model returned an empty probability tensor".to_owned(),
            })?;
            Ok((probability, next_state.to_vec()))
        })?;

        self.state = new_state;
        self.context
            .copy_from_slice(&frame[FRAME_SAMPLES - CONTEXT_SAMPLES..]);
        self.samples_processed = self.samples_processed.saturating_add(FRAME_SAMPLES as u64);
        self.recent_probabilities.push((start, probability));
        let oldest = start.saturating_sub(MAX_SPLIT_LOOKBACK_SAMPLES);
        self.recent_probabilities
            .retain(|(sample, _)| *sample >= oldest);
        self.update_regions(start, probability, regions);
        self.info.backend = self.session.backend().to_string();
        Ok(())
    }

    fn update_regions(
        &mut self,
        frame_start: u64,
        probability: f32,
        regions: &mut Vec<SpeechRegion>,
    ) {
        let frame_end = frame_start + FRAME_SAMPLES as u64;
        if self.active_start.is_none() {
            if probability >= SPEECH_THRESHOLD {
                self.active_start = Some(frame_start);
            }
            return;
        }

        if probability < NEGATIVE_THRESHOLD {
            self.silence_start.get_or_insert(frame_start);
        } else if probability >= SPEECH_THRESHOLD {
            self.silence_start = None;
        }

        if let (Some(start), Some(silence_start)) = (self.active_start, self.silence_start)
            && frame_end.saturating_sub(silence_start) >= MIN_SILENCE_SAMPLES
        {
            self.push_region(start, silence_start, regions);
            self.active_start = None;
            self.silence_start = None;
            self.recent_probabilities.clear();
            return;
        }

        if let Some(start) = self.active_start
            && frame_end.saturating_sub(start) >= MAX_REGION_SAMPLES
        {
            let lookback_start = frame_end
                .saturating_sub(MAX_SPLIT_LOOKBACK_SAMPLES)
                .max(start);
            if let Some((split_frame, _)) = self
                .recent_probabilities
                .iter()
                .filter(|(sample, _)| *sample >= lookback_start)
                .min_by(|left, right| left.1.total_cmp(&right.1))
            {
                let split = split_frame.saturating_add((FRAME_SAMPLES / 2) as u64);
                if split > start {
                    self.push_region(start, split, regions);
                    self.active_start = Some(split);
                    self.silence_start = None;
                    self.recent_probabilities
                        .retain(|(sample, _)| *sample >= split);
                }
            }
        }
    }

    fn push_region(&self, start: u64, end: u64, regions: &mut Vec<SpeechRegion>) {
        if end.saturating_sub(start) < MIN_SPEECH_SAMPLES {
            return;
        }
        let padded_start = start.saturating_sub(SPEECH_PAD_SAMPLES);
        let padded_end = end
            .saturating_add(SPEECH_PAD_SAMPLES)
            .min(self.samples_received);
        if padded_end > padded_start {
            regions.push(SpeechRegion {
                start: padded_start,
                end: padded_end,
            });
        }
    }

    fn clear_state(&mut self) {
        self.frame_buffer.clear();
        self.context = [0.0; CONTEXT_SAMPLES];
        self.state.fill(0.0);
        self.samples_received = 0;
        self.samples_processed = 0;
        self.active_start = None;
        self.silence_start = None;
        self.recent_probabilities.clear();
    }
}

impl VoiceActivityDetector for SileroVad {
    fn push(&mut self, pcm: &[f32]) -> Result<Vec<SpeechRegion>> {
        self.samples_received = self.samples_received.saturating_add(pcm.len() as u64);
        self.frame_buffer.extend_from_slice(pcm);
        let mut regions = Vec::new();
        let mut consumed = 0;
        while self.frame_buffer.len() - consumed >= FRAME_SAMPLES {
            let mut frame = [0.0; FRAME_SAMPLES];
            frame.copy_from_slice(&self.frame_buffer[consumed..consumed + FRAME_SAMPLES]);
            consumed += FRAME_SAMPLES;
            if let Err(error) = self.process_frame(&frame, &mut regions) {
                self.compact_frame_buffer(consumed);
                return Err(error);
            }
        }
        self.compact_frame_buffer(consumed);
        Ok(regions)
    }

    fn flush(&mut self) -> Vec<SpeechRegion> {
        let mut regions = Vec::new();
        if let Some(start) = self.active_start.take() {
            self.push_region(start, self.samples_received, &mut regions);
        }
        self.clear_state();
        regions
    }

    fn reset(&mut self) {
        self.clear_state();
    }
}

impl SileroVad {
    /// Drop all complete frames consumed by the last push in one linear move.
    fn compact_frame_buffer(&mut self, consumed: usize) {
        if consumed == 0 {
            return;
        }
        self.frame_buffer.copy_within(consumed.., 0);
        self.frame_buffer
            .truncate(self.frame_buffer.len() - consumed);
    }
}

fn model_error(model: &str, error: impl std::fmt::Display) -> Error {
    Error::Model {
        model: model.to_owned(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_push_matches_one_second_pushes() {
        let cache = std::env::temp_dir().join(format!(
            "yc-vad-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&cache).expect("create temporary model cache");
        let audio = (0..16_000 * 20)
            .map(|sample| {
                let second = sample / 16_000;
                if (2..6).contains(&second) || (10..15).contains(&second) {
                    (2.0 * std::f32::consts::PI * 220.0 * sample as f32 / 16_000.0).sin() * 0.2
                } else {
                    0.0
                }
            })
            .collect::<Vec<_>>();
        let mut single = SileroVad::load(Backend::Cpu, &cache).expect("VAD model");
        let mut chunked = SileroVad::load(Backend::Cpu, &cache).expect("VAD model");

        let mut single_regions = single.push(&audio).expect("single large push");
        single_regions.extend(single.flush());
        let mut chunked_regions = Vec::new();
        for chunk in audio.chunks(16_000) {
            chunked_regions.extend(chunked.push(chunk).expect("one-second push"));
        }
        chunked_regions.extend(chunked.flush());

        assert_eq!(single_regions, chunked_regions);
        std::fs::remove_dir_all(cache).expect("remove temporary model cache");
    }
}
