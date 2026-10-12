//! Per-track worker thread: ring buffer → 16 kHz mono → [`Pipeline`].
//!
//! It also keeps the track on the wall clock: gaps caused by rebuilding a stream (or by sources
//! that stop delivering during silence) are filled with digital silence so both tracks stay
//! aligned on the shared meeting timeline.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Receiver, Sender};
use rtrb::Consumer;
use tokio::sync::watch;
use yc_core::events::Level;
use yc_core::meeting::{TrackInfo, TrackKind};
use yc_core::{Result, SAMPLE_RATE};

use crate::config::{AudioFrame, RecorderEvent, SilenceConfig};
use crate::pipeline::{Pipeline, silence_samples};
use crate::resample::Resampler16k;
use crate::supervisor::{StreamShared, StreamStart};
use crate::writer::{TrackMeta, TrackWriter};

/// Shared time origin of a meeting; both tracks measure their offsets against it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Anchor {
    pub instant: Instant,
    pub wall: SystemTime,
}

impl Anchor {
    pub(crate) fn now() -> Self {
        Self {
            instant: Instant::now(),
            wall: SystemTime::now(),
        }
    }
}

/// Gaps shorter than this are normal callback jitter.
const GAP_MIN: Duration = Duration::from_millis(150);
/// Never invent more than this much silence for one gap.
const GAP_MAX: Duration = Duration::from_secs(60);
/// Idle-filling sources are topped up with silence after this long without data.
const IDLE_AFTER: Duration = Duration::from_millis(250);
const POLL: Duration = Duration::from_millis(10);
const MAX_READ: usize = 1 << 16;

struct Active {
    consumer: Consumer<f32>,
    resampler: Resampler16k,
    rate: u32,
    shared: Arc<StreamShared>,
    idle_fill: bool,
    device: String,
    fresh: bool,
}

pub(crate) struct IngestParams {
    pub kind: TrackKind,
    pub dir: std::path::PathBuf,
    pub anchor: Anchor,
    pub silence: SilenceConfig,
    pub frame_len: usize,
    pub streams: Receiver<StreamStart>,
    pub stop: Arc<AtomicBool>,
    pub paused: Arc<AtomicBool>,
    pub level_tx: watch::Sender<Level>,
    pub events: Sender<RecorderEvent>,
    pub frames_tx: Sender<AudioFrame>,
}

pub(crate) struct Ingest {
    p: IngestParams,
    active: Option<Active>,
    pipeline: Option<Pipeline>,
    in_buf: Vec<f32>,
    out_buf: Vec<f32>,
    last_arrival: Instant,
    reported_drops: u64,
    last_drop_report: Instant,
    io_failed: bool,
}

impl Ingest {
    pub(crate) fn new(p: IngestParams) -> Self {
        Self {
            p,
            active: None,
            pipeline: None,
            in_buf: Vec::with_capacity(MAX_READ),
            out_buf: Vec::with_capacity(MAX_READ),
            last_arrival: Instant::now(),
            reported_drops: 0,
            last_drop_report: Instant::now(),
            io_failed: false,
        }
    }

    /// Runs until the stop flag is set and all pending audio has been written. Returns the track
    /// summary, or `None` if the track never produced a sample.
    pub(crate) fn run(mut self) -> Result<Option<TrackInfo>> {
        loop {
            let stopping = self.p.stop.load(Ordering::Acquire);
            let mut worked = false;

            while let Ok(start) = self.p.streams.try_recv() {
                self.drain_active();
                self.switch(start);
                worked = true;
            }
            worked |= self.read_and_process();
            self.apply_pause();
            self.idle_fill_if_needed();
            self.report_drops();
            if let Some(p) = self.pipeline.as_mut()
                && let Err(e) = p.tick(Instant::now())
            {
                self.io_error(&e.to_string());
            }

            if stopping && !worked {
                break;
            }
            if !worked {
                std::thread::sleep(POLL);
            }
        }
        match self.pipeline.take() {
            Some(p) => p.finish(Instant::now()).map(Some),
            None => Ok(None),
        }
    }

    fn switch(&mut self, start: StreamStart) {
        let resampler = match Resampler16k::new(start.sample_rate) {
            Ok(r) => r,
            Err(e) => {
                let _ = self.p.events.send(RecorderEvent::Error {
                    track: self.p.kind,
                    message: e.to_string(),
                });
                return;
            }
        };
        if let Some(p) = self.pipeline.as_mut()
            && let Err(e) = p.note_device_change(&start.device)
        {
            self.io_error(&e.to_string());
        }
        self.reported_drops = 0;
        self.active = Some(Active {
            consumer: start.consumer,
            resampler,
            rate: start.sample_rate,
            shared: start.shared,
            idle_fill: start.idle_fill,
            device: start.device,
            fresh: true,
        });
    }

    /// Processes everything left in the current stream's ring.
    fn drain_active(&mut self) {
        while self.read_and_process() {}
    }

    fn read_and_process(&mut self) -> bool {
        let Some(active) = self.active.as_mut() else {
            return false;
        };
        let avail = active.consumer.slots().min(MAX_READ);
        if avail == 0 {
            return false;
        }
        self.in_buf.clear();
        if let Ok(chunk) = active.consumer.read_chunk(avail) {
            let (a, b) = chunk.as_slices();
            self.in_buf.extend_from_slice(a);
            self.in_buf.extend_from_slice(b);
            chunk.commit_all();
        }
        let now = Instant::now();
        let chunk_dur = Duration::from_secs_f64(self.in_buf.len() as f64 / f64::from(active.rate));
        let (fresh, idle_fill) = (active.fresh, active.idle_fill);
        let device = active.device.clone();
        active.fresh = false;

        self.out_buf.clear();
        if let Err(e) = active.resampler.process(&self.in_buf, &mut self.out_buf) {
            let _ = self.p.events.send(RecorderEvent::Error {
                track: self.p.kind,
                message: e.to_string(),
            });
            return true;
        }

        if self.pipeline.is_none() {
            let first_sample = now.checked_sub(chunk_dur).unwrap_or(now);
            match self.create_pipeline(&device, first_sample) {
                Ok(p) => self.pipeline = Some(p),
                Err(e) => {
                    self.io_error(&format!("cannot create track files: {e}"));
                    return true;
                }
            }
        } else if fresh && !idle_fill {
            // Wall-clock gap between the previous stream's last data and this stream's first sample.
            let gap = now
                .saturating_duration_since(self.last_arrival)
                .saturating_sub(chunk_dur);
            if gap >= GAP_MIN && !self.pipeline.as_ref().is_some_and(Pipeline::is_paused) {
                self.pad(gap.min(GAP_MAX));
            }
        }
        self.last_arrival = now;

        if !self.out_buf.is_empty() {
            let out = std::mem::take(&mut self.out_buf);
            self.push(&out);
            self.out_buf = out;
        }
        true
    }

    fn create_pipeline(&self, device: &str, first_sample: Instant) -> Result<Pipeline> {
        let offset = first_sample.saturating_duration_since(self.p.anchor.instant);
        let wall = self.p.anchor.wall + offset;
        let unix_ms = wall
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let meta = TrackMeta::new(self.p.kind, device, unix_ms, offset.as_millis() as u64);
        let writer = TrackWriter::create(&self.p.dir, meta, Instant::now())?;
        Ok(Pipeline::new(
            self.p.kind,
            writer,
            self.p.silence,
            self.p.frame_len,
            self.p.level_tx.clone(),
            self.p.events.clone(),
            self.p.frames_tx.clone(),
        ))
    }

    fn push(&mut self, block: &[f32]) {
        if let Some(p) = self.pipeline.as_mut()
            && let Err(e) = p.push(block)
        {
            self.io_error(&e.to_string());
        }
    }

    fn pad(&mut self, gap: Duration) {
        let mut left = silence_samples(gap.as_millis() as u64);
        let zeros = vec![0.0f32; SAMPLE_RATE as usize / 2];
        while left > 0 {
            let n = left.min(zeros.len());
            self.push(&zeros[..n]);
            left -= n;
        }
    }

    fn idle_fill_if_needed(&mut self) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        if !active.idle_fill || self.pipeline.is_none() {
            return;
        }
        let now = Instant::now();
        let idle = now.saturating_duration_since(self.last_arrival);
        if idle >= IDLE_AFTER {
            self.pad(idle);
            self.last_arrival = now;
        }
    }

    fn apply_pause(&mut self) {
        let want = self.p.paused.load(Ordering::Relaxed);
        if let Some(p) = self.pipeline.as_mut()
            && p.is_paused() != want
            && let Err(e) = p.set_paused(want, Instant::now())
        {
            self.io_error(&e.to_string());
        }
    }

    fn report_drops(&mut self) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        if self.last_drop_report.elapsed() < Duration::from_secs(1) {
            return;
        }
        let total = active.shared.dropped.load(Ordering::Relaxed);
        if total > self.reported_drops {
            let _ = self.p.events.send(RecorderEvent::Overrun {
                track: self.p.kind,
                dropped_samples: total - self.reported_drops,
            });
            self.reported_drops = total;
        }
        self.last_drop_report = Instant::now();
    }

    fn io_error(&mut self, message: &str) {
        tracing::error!(track = ?self.p.kind, "{message}");
        if !self.io_failed {
            self.io_failed = true;
            let _ = self.p.events.send(RecorderEvent::Error {
                track: self.p.kind,
                message: message.to_owned(),
            });
        }
    }
}
