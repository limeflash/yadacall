//! Kaldi-compatible log-filterbank extraction for the CAMPPlus speaker model.

use rustfft::{Fft, FftPlanner, num_complex::Complex32};
use std::sync::Arc;

const SAMPLE_RATE: usize = 16_000;
const FRAME_LENGTH: usize = 400;
const FRAME_SHIFT: usize = 160;
const FFT_SIZE: usize = 512;
const MEL_BINS: usize = 80;
const PREEMPHASIS: f32 = 0.97;

/// 80-bin Kaldi FBank frontend used by the 3D-Speaker CAMPPlus model.
pub struct KaldiFbank {
    fft: Arc<dyn Fft<f32>>,
    window: [f32; FRAME_LENGTH],
    mel_weights: Vec<Vec<f32>>,
}

impl Default for KaldiFbank {
    fn default() -> Self {
        Self::new()
    }
}

impl KaldiFbank {
    /// Creates the 16 kHz, 25 ms / 10 ms Kaldi fbank configuration.
    pub fn new() -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let window = std::array::from_fn(|index| {
            let hamming = 0.54
                - 0.46
                    * (2.0 * std::f32::consts::PI * index as f32 / (FRAME_LENGTH - 1) as f32).cos();
            hamming.powf(0.85)
        });
        Self {
            fft,
            window,
            mel_weights: make_mel_weights(),
        }
    }

    /// Computes log-mel frames and subtracts the temporal mean of each mel bin.
    ///
    /// Signals shorter than one analysis frame return an empty feature matrix.
    pub fn compute(&self, pcm: &[f32]) -> Vec<[f32; MEL_BINS]> {
        if pcm.len() < FRAME_LENGTH {
            return Vec::new();
        }
        let frame_count = 1 + (pcm.len() - FRAME_LENGTH) / FRAME_SHIFT;
        let mut frames = Vec::with_capacity(frame_count);
        let mut fft_buffer = vec![Complex32::default(); FFT_SIZE];
        for frame_index in 0..frame_count {
            let start = frame_index * FRAME_SHIFT;
            let samples = &pcm[start..start + FRAME_LENGTH];
            let dc_mean = samples.iter().sum::<f32>() / FRAME_LENGTH as f32;
            fft_buffer.fill(Complex32::default());
            let mut previous = samples[0] - dc_mean;
            fft_buffer[0].re = (previous - PREEMPHASIS * previous) * self.window[0];
            for index in 1..FRAME_LENGTH {
                let current = samples[index] - dc_mean;
                let emphasized = current - PREEMPHASIS * previous;
                fft_buffer[index].re = emphasized * self.window[index];
                previous = current;
            }
            self.fft.process(&mut fft_buffer);

            let mut feature = [0.0; MEL_BINS];
            for (mel_index, weights) in self.mel_weights.iter().enumerate() {
                let energy = weights
                    .iter()
                    .zip(fft_buffer.iter())
                    .map(|(weight, value)| weight * value.norm_sqr())
                    .sum::<f32>();
                feature[mel_index] = energy.max(f32::MIN_POSITIVE).ln();
            }
            frames.push(feature);
        }

        for bin in 0..MEL_BINS {
            let mean = frames.iter().map(|frame| frame[bin]).sum::<f32>() / frame_count as f32;
            for frame in &mut frames {
                frame[bin] -= mean;
            }
        }
        frames
    }
}

fn make_mel_weights() -> Vec<Vec<f32>> {
    let low_mel = hz_to_mel(20.0);
    let high_mel = hz_to_mel((SAMPLE_RATE / 2) as f32);
    let mel_step = (high_mel - low_mel) / (MEL_BINS + 1) as f32;
    let bin_hz = SAMPLE_RATE as f32 / FFT_SIZE as f32;
    (0..MEL_BINS)
        .map(|mel_index| {
            let left = low_mel + mel_step * mel_index as f32;
            let center = left + mel_step;
            let right = center + mel_step;
            (0..=FFT_SIZE / 2)
                .map(|bin| {
                    let mel = hz_to_mel(bin as f32 * bin_hz);
                    if mel < left || mel > right {
                        0.0
                    } else if mel <= center {
                        (mel - left) / (center - left)
                    } else {
                        (right - mel) / (right - center)
                    }
                })
                .collect()
        })
        .collect()
}

fn hz_to_mel(hz: f32) -> f32 {
    1127.0 * (1.0 + hz / 700.0).ln()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_second_yields_kaldi_snipped_frame_count_and_per_bin_cms() {
        let fbank = KaldiFbank::new();
        let pcm = (0..SAMPLE_RATE)
            .map(|index| {
                (2.0 * std::f32::consts::PI * 220.0 * index as f32 / SAMPLE_RATE as f32).sin()
            })
            .collect::<Vec<_>>();
        let features = fbank.compute(&pcm);
        assert_eq!(features.len(), 98);
        for bin in 0..MEL_BINS {
            let mean = features.iter().map(|frame| frame[bin]).sum::<f32>() / features.len() as f32;
            assert!(mean.abs() < 1e-5, "bin {bin} mean was {mean}");
        }
        assert!(features.iter().flatten().all(|value| value.is_finite()));
    }

    #[test]
    fn short_waveforms_produce_no_frames() {
        assert!(
            KaldiFbank::new()
                .compute(&[0.0; FRAME_LENGTH - 1])
                .is_empty()
        );
    }
}
