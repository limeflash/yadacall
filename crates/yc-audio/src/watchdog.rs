//! Silence watchdog: warns when a track is quiet for too long and when it is *digitally* dead.

use yc_core::SAMPLE_RATE;

use crate::config::SilenceConfig;

/// 100 ms analysis windows.
const WINDOW: usize = (SAMPLE_RATE / 10) as usize;

/// What the watchdog observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogEvent {
    /// Below the threshold for `ms` (raised at `warn_after`, then again every `warn_after`).
    Silent { ms: u64 },
    /// Signal is back after a `Silent` warning.
    Recovered,
    /// Exactly zero samples since the very start for `ms`.
    NoSignal { ms: u64 },
}

/// Feed 16 kHz mono blocks with [`SilenceWatchdog::push`].
#[derive(Debug)]
pub struct SilenceWatchdog {
    threshold_lin: f32,
    warn_after: u64,
    no_signal_after: u64,
    filled: usize,
    sum_sq: f64,
    all_zero: bool,
    silent_samples: u64,
    next_warn: u64,
    warned: bool,
    ever_signal: bool,
    zero_samples: u64,
    no_signal_reported: bool,
}

impl SilenceWatchdog {
    pub fn new(cfg: SilenceConfig) -> Self {
        let warn_after = (cfg.warn_after.as_secs_f64() * f64::from(SAMPLE_RATE)) as u64;
        Self {
            threshold_lin: 10f32.powf(cfg.threshold_dbfs / 20.0),
            warn_after,
            no_signal_after: (cfg.no_signal_after.as_secs_f64() * f64::from(SAMPLE_RATE)) as u64,
            filled: 0,
            sum_sq: 0.0,
            all_zero: true,
            silent_samples: 0,
            next_warn: warn_after,
            warned: false,
            ever_signal: false,
            zero_samples: 0,
            no_signal_reported: false,
        }
    }

    /// Analyses `block`, appending events to `out`.
    pub fn push(&mut self, block: &[f32], out: &mut Vec<WatchdogEvent>) {
        for &s in block {
            self.sum_sq += f64::from(s) * f64::from(s);
            self.all_zero &= s == 0.0;
            self.filled += 1;
            if self.filled == WINDOW {
                self.window_done(out);
            }
        }
    }

    fn window_done(&mut self, out: &mut Vec<WatchdogEvent>) {
        let rms = (self.sum_sq / WINDOW as f64).sqrt() as f32;
        let all_zero = self.all_zero;
        self.filled = 0;
        self.sum_sq = 0.0;
        self.all_zero = true;

        if rms < self.threshold_lin {
            self.silent_samples += WINDOW as u64;
            if self.silent_samples >= self.next_warn {
                out.push(WatchdogEvent::Silent {
                    ms: self.silent_samples * 1000 / u64::from(SAMPLE_RATE),
                });
                self.warned = true;
                self.next_warn += self.warn_after;
            }
        } else {
            if self.warned {
                out.push(WatchdogEvent::Recovered);
            }
            self.silent_samples = 0;
            self.next_warn = self.warn_after;
            self.warned = false;
        }

        if !self.ever_signal {
            if all_zero {
                self.zero_samples += WINDOW as u64;
                if !self.no_signal_reported && self.zero_samples >= self.no_signal_after {
                    self.no_signal_reported = true;
                    out.push(WatchdogEvent::NoSignal {
                        ms: self.zero_samples * 1000 / u64::from(SAMPLE_RATE),
                    });
                }
            } else {
                self.ever_signal = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn cfg() -> SilenceConfig {
        SilenceConfig {
            threshold_dbfs: -50.0,
            warn_after: Duration::from_secs(60),
            no_signal_after: Duration::from_secs(10),
        }
    }

    fn feed(w: &mut SilenceWatchdog, secs: usize, level: f32) -> Vec<WatchdogEvent> {
        let mut out = Vec::new();
        let block = vec![level; SAMPLE_RATE as usize];
        for _ in 0..secs {
            w.push(&block, &mut out);
        }
        out
    }

    #[test]
    fn warns_after_60s_of_quiet_then_recovers() {
        let mut w = SilenceWatchdog::new(cfg());
        // -60 dBFS hiss: quiet but not digital zero.
        assert!(feed(&mut w, 59, 0.001).is_empty());
        let ev = feed(&mut w, 2, 0.001);
        assert_eq!(ev, vec![WatchdogEvent::Silent { ms: 60_000 }]);
        // Re-raised one period later, not every second.
        let ev = feed(&mut w, 60, 0.001);
        assert_eq!(ev, vec![WatchdogEvent::Silent { ms: 120_000 }]);
        let ev = feed(&mut w, 1, 0.2);
        assert_eq!(ev, vec![WatchdogEvent::Recovered]);
        assert!(feed(&mut w, 30, 0.001).is_empty(), "counter restarted");
    }

    #[test]
    fn loud_audio_never_warns() {
        let mut w = SilenceWatchdog::new(cfg());
        assert!(feed(&mut w, 120, 0.05).is_empty());
    }

    #[test]
    fn exact_zeros_from_start_flag_no_signal_once() {
        let mut w = SilenceWatchdog::new(cfg());
        let ev = feed(&mut w, 12, 0.0);
        assert_eq!(ev, vec![WatchdogEvent::NoSignal { ms: 10_000 }]);
        assert!(feed(&mut w, 5, 0.0).is_empty());
    }

    #[test]
    fn zeros_after_real_audio_are_not_a_permission_problem() {
        let mut w = SilenceWatchdog::new(cfg());
        feed(&mut w, 1, 0.3);
        assert!(feed(&mut w, 30, 0.0).is_empty());
    }
}
