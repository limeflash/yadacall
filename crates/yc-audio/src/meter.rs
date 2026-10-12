//! RMS + peak level meter over fixed windows (25 Hz at 16 kHz).

use yc_core::SAMPLE_RATE;
use yc_core::events::Level;

/// Windowed level meter. Feed it any block size; it calls `emit` once per completed window.
#[derive(Debug)]
pub struct LevelMeter {
    window: usize,
    filled: usize,
    sum_sq: f64,
    peak: f32,
}

impl LevelMeter {
    /// Meter publishing `hz` levels per second for 16 kHz audio.
    pub fn new(hz: u32) -> Self {
        Self {
            window: (SAMPLE_RATE / hz.max(1)) as usize,
            filled: 0,
            sum_sq: 0.0,
            peak: 0.0,
        }
    }

    /// Consumes `block`, calling `emit` for every completed window.
    pub fn push(&mut self, block: &[f32], mut emit: impl FnMut(Level)) {
        for &s in block {
            self.sum_sq += f64::from(s) * f64::from(s);
            let a = s.abs();
            if a > self.peak {
                self.peak = a;
            }
            self.filled += 1;
            if self.filled == self.window {
                let rms = (self.sum_sq / self.window as f64).sqrt() as f32;
                emit(Level {
                    rms: rms.min(1.0),
                    peak: self.peak.min(1.0),
                });
                self.filled = 0;
                self.sum_sq = 0.0;
                self.peak = 0.0;
            }
        }
    }
}

/// Linear amplitude to dBFS (floor −120).
pub fn to_dbfs(linear: f32) -> f32 {
    if linear <= 1e-6 {
        -120.0
    } else {
        20.0 * linear.log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SAMPLE_RATE as f32).sin()
            })
            .collect()
    }

    #[test]
    fn sine_rms_and_peak() {
        let mut m = LevelMeter::new(25);
        let mut out = Vec::new();
        m.push(&sine(1000.0, 0.5, 16_000), |l| out.push(l));
        assert_eq!(out.len(), 25, "25 Hz over one second");
        let l = out[10];
        assert!((l.rms - 0.5 / 2f32.sqrt()).abs() < 0.01, "rms {}", l.rms);
        assert!((l.peak - 0.5).abs() < 0.01, "peak {}", l.peak);
    }

    #[test]
    fn windows_span_blocks_and_silence_reads_zero() {
        let mut m = LevelMeter::new(25);
        let mut out = Vec::new();
        for _ in 0..16 {
            m.push(&[0.0; 100], |l| out.push(l)); // 1600 samples = 2.5 windows
        }
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|l| l.rms == 0.0 && l.peak == 0.0));
        assert_eq!(to_dbfs(0.0), -120.0);
        assert!((to_dbfs(0.1) + 20.0).abs() < 0.01);
    }
}
