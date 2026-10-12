//! Streaming resampler to the pipeline rate (16 kHz mono) built on `rubato`'s FFT resampler.
//!
//! The FFT (synchronous) resampler is exact for the rational ratios real devices use
//! (48000/16000 = 3, 44100/16000 = 441/160, …), has a steep anti-aliasing filter and costs a
//! fraction of a sinc resampler — well below 1 % of a core for one 48 kHz track.

use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use yc_core::{Error, Result, SAMPLE_RATE};

/// Chunk size handed to rubato (input frames); rubato rounds it to what the ratio allows.
const CHUNK_FRAMES: usize = 1024;

/// Converts mono f32 audio at an arbitrary rate to [`SAMPLE_RATE`], any block size in/out.
pub struct Resampler16k {
    inner: Option<Inner>,
}

struct Inner {
    fft: Fft<f32>,
    pending: Vec<f32>,
    scratch: Vec<f32>,
    /// Leading output frames that are filter warm-up and must be dropped to keep time alignment.
    skip: usize,
}

impl Resampler16k {
    /// Builds a resampler for `input_rate` Hz mono input. A 16 kHz input is passed through.
    pub fn new(input_rate: u32) -> Result<Self> {
        if input_rate == SAMPLE_RATE {
            return Ok(Self { inner: None });
        }
        if input_rate == 0 {
            return Err(Error::Invalid("sample rate 0".into()));
        }
        let fft = Fft::<f32>::new(
            input_rate as usize,
            SAMPLE_RATE as usize,
            CHUNK_FRAMES,
            1,
            FixedSync::Input,
        )
        .map_err(|e| Error::Device(format!("resampler {input_rate}->16000: {e}")))?;
        let scratch = vec![0.0; fft.output_frames_max()];
        let skip = fft.output_delay();
        Ok(Self {
            inner: Some(Inner {
                fft,
                pending: Vec::new(),
                scratch,
                skip,
            }),
        })
    }

    /// Resamples `input`, appending produced 16 kHz samples to `out`. Samples that do not fill a
    /// whole resampler chunk are kept internally until the next call.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<()> {
        let Some(inner) = self.inner.as_mut() else {
            out.extend_from_slice(input);
            return Ok(());
        };
        inner.pending.extend_from_slice(input);
        loop {
            let need = inner.fft.input_frames_next();
            if inner.pending.len() < need {
                return Ok(());
            }
            let in_adapter = InterleavedSlice::new(&inner.pending[..need], 1, need)
                .map_err(|e| Error::Device(format!("resampler input: {e}")))?;
            let cap = inner.scratch.len();
            let mut out_adapter = InterleavedSlice::new_mut(&mut inner.scratch, 1, cap)
                .map_err(|e| Error::Device(format!("resampler output: {e}")))?;
            let (used, produced) = inner
                .fft
                .process_into_buffer(&in_adapter, &mut out_adapter, None)
                .map_err(|e| Error::Device(format!("resampler: {e}")))?;
            inner.pending.drain(..used);
            let skip = inner.skip.min(produced);
            inner.skip -= skip;
            out.extend_from_slice(&inner.scratch[skip..produced]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, freq: f32, amp: f32, secs: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    /// Amplitude of `freq` in `x` (16 kHz) via a single-bin DFT.
    fn amplitude_at(x: &[f32], freq: f32) -> f32 {
        let w = 2.0 * std::f64::consts::PI * f64::from(freq) / f64::from(SAMPLE_RATE);
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &s) in x.iter().enumerate() {
            re += f64::from(s) * (w * i as f64).cos();
            im += f64::from(s) * (w * i as f64).sin();
        }
        (2.0 * (re * re + im * im).sqrt() / x.len() as f64) as f32
    }

    fn run(rate: u32, input: &[f32], block: usize) -> Vec<f32> {
        let mut r = Resampler16k::new(rate).unwrap();
        let mut out = Vec::new();
        for c in input.chunks(block) {
            r.process(c, &mut out).unwrap();
        }
        out
    }

    #[test]
    fn length_and_frequency_48k() {
        let out = run(48_000, &tone(48_000, 1000.0, 0.5, 2.0), 480);
        let expect = 32_000usize;
        assert!(
            out.len() <= expect && out.len() + 1100 >= expect,
            "len {}",
            out.len()
        );
        let mid = &out[4000..out.len() - 4000];
        let a = amplitude_at(mid, 1000.0);
        assert!((a - 0.5).abs() < 0.02, "1 kHz amplitude {a}");
        assert!(
            amplitude_at(mid, 1300.0) < 0.01,
            "no energy at neighbouring bins"
        );
    }

    #[test]
    fn length_and_frequency_44k1() {
        let out = run(44_100, &tone(44_100, 440.0, 0.8, 2.0), 441);
        assert!(
            out.len() <= 32_000 && out.len() + 1100 >= 32_000,
            "len {}",
            out.len()
        );
        let a = amplitude_at(&out[4000..out.len() - 4000], 440.0);
        assert!((a - 0.8).abs() < 0.03, "440 Hz amplitude {a}");
    }

    #[test]
    fn out_of_band_tone_is_filtered_not_aliased() {
        // 20 kHz is above the 8 kHz Nyquist of the target; naive decimation would alias it to 4 kHz.
        let out = run(48_000, &tone(48_000, 20_000.0, 0.8, 2.0), 512);
        let mid = &out[4000..out.len() - 4000];
        let alias = amplitude_at(mid, 4000.0);
        assert!(
            alias < 0.01,
            "aliased amplitude {alias} (≥ 38 dB rejection expected)"
        );
    }

    #[test]
    fn passthrough_at_16k_and_block_size_independence() {
        let x = tone(16_000, 300.0, 0.3, 0.5);
        assert_eq!(run(16_000, &x, 77), x);
        let y = tone(48_000, 500.0, 0.3, 1.0);
        let a = run(48_000, &y, 100);
        let b = run(48_000, &y, 4800);
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(&b).all(|(p, q)| (p - q).abs() < 1e-5));
    }
}
