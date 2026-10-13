//! The per-track sink for 16 kHz mono audio: level meter, silence watchdog, crash-safe writer and
//! live frame delivery. Pure logic driven by sample counts, so it is testable without devices.

use std::time::Instant;

use crossbeam_channel::{Sender, TrySendError};
use tokio::sync::watch;
use yc_core::error::PermissionKind;
use yc_core::events::Level;
use yc_core::meeting::{TrackInfo, TrackKind};
use yc_core::{Result, SAMPLE_RATE};

use crate::config::{AudioFrame, RecorderEvent, SilenceConfig};
use crate::meter::LevelMeter;
use crate::watchdog::{SilenceWatchdog, WatchdogEvent};
use crate::writer::{SYNC_INTERVAL, TrackWriter};

/// Levels are published this many times per second.
pub const LEVEL_HZ: u32 = 25;

pub(crate) struct Pipeline {
    kind: TrackKind,
    writer: TrackWriter,
    meter: LevelMeter,
    watchdog: SilenceWatchdog,
    level_tx: watch::Sender<Level>,
    events: Sender<RecorderEvent>,
    frames_tx: Sender<AudioFrame>,
    frame_len: usize,
    frame_acc: Vec<f32>,
    frame_start: u64,
    dropped_frames: u64,
    paused: bool,
    wd_events: Vec<WatchdogEvent>,
}

impl Pipeline {
    pub(crate) fn new(
        kind: TrackKind,
        writer: TrackWriter,
        silence: SilenceConfig,
        frame_len: usize,
        level_tx: watch::Sender<Level>,
        events: Sender<RecorderEvent>,
        frames_tx: Sender<AudioFrame>,
    ) -> Self {
        let frame_len = frame_len.max(1);
        Self {
            kind,
            writer,
            meter: LevelMeter::new(LEVEL_HZ),
            watchdog: SilenceWatchdog::new(silence),
            level_tx,
            events,
            frames_tx,
            frame_len,
            frame_acc: Vec::with_capacity(frame_len),
            frame_start: 0,
            dropped_frames: 0,
            paused: false,
            wd_events: Vec::new(),
        }
    }

    /// Samples written to the track so far.
    pub(crate) fn written(&self) -> u64 {
        self.writer.samples()
    }

    pub(crate) fn is_paused(&self) -> bool {
        self.paused
    }

    /// Handles one block of 16 kHz mono samples.
    pub(crate) fn push(&mut self, block: &[f32]) -> Result<()> {
        let level_tx = &self.level_tx;
        self.meter.push(block, |l| {
            let _ = level_tx.send(l);
        });

        self.wd_events.clear();
        self.watchdog.push(block, &mut self.wd_events);
        for ev in self.wd_events.drain(..) {
            let ev = match ev {
                WatchdogEvent::Silent { ms } => RecorderEvent::TrackSilent {
                    track: self.kind,
                    silent_for_ms: ms,
                },
                WatchdogEvent::Recovered => RecorderEvent::TrackActive { track: self.kind },
                WatchdogEvent::NoSignal { ms } => RecorderEvent::PermissionSuspected {
                    track: self.kind,
                    permission: match self.kind {
                        TrackKind::Mic => PermissionKind::Microphone,
                        TrackKind::System => PermissionKind::SystemAudio,
                    },
                    silent_for_ms: ms,
                },
            };
            let _ = self.events.send(ev);
        }

        if self.paused {
            return Ok(());
        }
        let result = self.writer.write(block);
        self.deliver_frames(block);
        result
    }

    fn deliver_frames(&mut self, block: &[f32]) {
        let mut rest = block;
        while !rest.is_empty() {
            let take = (self.frame_len - self.frame_acc.len()).min(rest.len());
            self.frame_acc.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.frame_acc.len() == self.frame_len {
                let samples =
                    std::mem::replace(&mut self.frame_acc, Vec::with_capacity(self.frame_len));
                let frame = AudioFrame {
                    track: self.kind,
                    start_sample: self.frame_start,
                    samples,
                };
                self.frame_start += self.frame_len as u64;
                match self.frames_tx.try_send(frame) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        self.dropped_frames += 1;
                        if self.dropped_frames % 64 == 1 {
                            let _ = self.events.send(RecorderEvent::FramesDropped {
                                track: self.kind,
                                total: self.dropped_frames,
                            });
                        }
                    }
                    Err(TrySendError::Disconnected(_)) => {}
                }
            }
        }
    }

    /// Pauses/resumes writing (levels and the watchdog keep running).
    pub(crate) fn set_paused(&mut self, paused: bool, now: Instant) -> Result<()> {
        if paused == self.paused {
            return Ok(());
        }
        self.paused = paused;
        if paused {
            self.writer.begin_pause(now);
            // A partial frame must not straddle the pause.
            self.flush_partial_frame();
            Ok(())
        } else {
            self.writer.end_pause(now)
        }
    }

    /// Partial live frames are padded with silence so the consumer sees every sample.
    fn flush_partial_frame(&mut self) {
        if !self.frame_acc.is_empty() {
            let pad = vec![0.0; self.frame_len - self.frame_acc.len()];
            self.deliver_frames(&pad);
        }
    }

    pub(crate) fn note_device_change(&mut self, device: &str) -> Result<()> {
        self.writer.note_device_change(device)
    }

    /// Periodic housekeeping: forces data to disk at most every 2 s.
    pub(crate) fn tick(&mut self, now: Instant) -> Result<()> {
        self.writer.sync_if_due(now, SYNC_INTERVAL)
    }

    pub(crate) fn finish(mut self, now: Instant) -> Result<TrackInfo> {
        if !self.paused {
            self.flush_partial_frame();
        }
        self.writer.finish(now)
    }
}

/// Silence of `ms` milliseconds as 16 kHz samples (used to keep tracks on the wall clock).
pub(crate) fn silence_samples(ms: u64) -> usize {
    (ms * u64::from(SAMPLE_RATE) / 1000) as usize
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::writer::{TrackMeta, meta_path, pcm_path, read_meta};

    struct Rig {
        pipe: Pipeline,
        levels: watch::Receiver<Level>,
        events: crossbeam_channel::Receiver<RecorderEvent>,
        frames: crossbeam_channel::Receiver<AudioFrame>,
        dir: tempfile::TempDir,
    }

    fn rig(frame_queue: usize) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let meta = TrackMeta::new(TrackKind::System, "dev", 0, 0);
        let writer = TrackWriter::create(dir.path(), meta, Instant::now()).unwrap();
        let (level_tx, levels) = watch::channel(Level::default());
        let (etx, events) = crossbeam_channel::unbounded();
        let (ftx, frames) = crossbeam_channel::bounded(frame_queue);
        let pipe = Pipeline::new(
            TrackKind::System,
            writer,
            SilenceConfig::default(),
            512,
            level_tx,
            etx,
            ftx,
        );
        Rig {
            pipe,
            levels,
            events,
            frames,
            dir,
        }
    }

    #[test]
    fn levels_frames_and_file_agree() {
        let mut r = rig(1024);
        let tone: Vec<f32> = (0..16_000)
            .map(|i| 0.4 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin())
            .collect();
        for c in tone.chunks(333) {
            r.pipe.push(c).unwrap();
        }
        let lvl = *r.levels.borrow();
        assert!(
            (lvl.peak - 0.4).abs() < 0.02 && (lvl.rms - 0.283).abs() < 0.02,
            "{lvl:?}"
        );

        let frames: Vec<AudioFrame> = r.frames.try_iter().collect();
        assert_eq!(frames.len(), 16_000 / 512);
        assert_eq!(frames[3].start_sample, 3 * 512);
        assert_eq!(&frames[0].samples[..4], &tone[..4]);

        let info = r.pipe.finish(Instant::now()).unwrap();
        assert_eq!(info.samples, 16_000);
        let len = std::fs::metadata(pcm_path(r.dir.path(), TrackKind::System))
            .unwrap()
            .len();
        assert_eq!(len, 32_000);
    }

    #[test]
    fn lagging_consumer_drops_frames_without_blocking() {
        let mut r = rig(4);
        r.pipe.push(&vec![0.1; 512 * 20]).unwrap();
        assert_eq!(r.frames.len(), 4);
        let ev: Vec<_> = r.events.try_iter().collect();
        assert!(
            ev.iter()
                .any(|e| matches!(e, RecorderEvent::FramesDropped { total: 1, .. }))
        );
        // Recording itself was unaffected.
        assert_eq!(r.pipe.finish(Instant::now()).unwrap().samples, 512 * 20);
    }

    #[test]
    fn pause_keeps_file_gapless_and_continues_metering() {
        let mut r = rig(1024);
        let t0 = Instant::now();
        r.pipe.push(&vec![0.2; 16_000]).unwrap();
        r.pipe.set_paused(true, t0).unwrap();
        r.pipe.push(&vec![0.9; 16_000]).unwrap(); // not written
        assert!(
            (r.levels.borrow().peak - 0.9).abs() < 0.01,
            "meter still live while paused"
        );
        r.pipe
            .set_paused(false, t0 + Duration::from_secs(1))
            .unwrap();
        r.pipe.push(&vec![0.2; 8_000]).unwrap();
        let info = r.pipe.finish(Instant::now()).unwrap();
        assert_eq!(info.samples, 24_000);
        let meta = read_meta(&meta_path(r.dir.path(), TrackKind::System)).unwrap();
        assert_eq!(meta.pauses.len(), 1);
        assert_eq!(
            (meta.pauses[0].at_ms, meta.pauses[0].duration_ms),
            (1000, 1000)
        );
        let frames: Vec<AudioFrame> = r.frames.try_iter().collect();
        assert!(
            frames.iter().all(|f| f.samples.iter().all(|&s| s < 0.5)),
            "paused audio never reaches ASR"
        );
    }

    #[test]
    fn silent_track_raises_warning_and_permission_suspicion() {
        let mut r = rig(4096);
        // 70 s of exact zeros: NoSignal at 10 s, Silent at 60 s.
        for _ in 0..70 {
            r.pipe.push(&vec![0.0; 16_000]).unwrap();
        }
        let ev: Vec<_> = r
            .events
            .try_iter()
            .filter(|e| !matches!(e, RecorderEvent::FramesDropped { .. }))
            .collect();
        assert!(matches!(
            ev[0],
            RecorderEvent::PermissionSuspected {
                permission: PermissionKind::SystemAudio,
                silent_for_ms: 10_000,
                ..
            }
        ));
        assert!(
            matches!(
                ev[1],
                RecorderEvent::TrackSilent {
                    silent_for_ms: 60_000,
                    ..
                }
            ),
            "{ev:?}"
        );
    }
}
