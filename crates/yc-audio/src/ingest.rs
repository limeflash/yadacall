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
/// Observation window of the measured-rate watch.
const RATE_WINDOW: Duration = Duration::from_millis(1500);
/// Windows containing a hole longer than this say nothing about the rate (stall, not slow clock).
const RATE_MAX_HOLE: Duration = Duration::from_millis(300);
/// How often the written-vs-wall-clock drift is checked.
const DRIFT_CHECK: Duration = Duration::from_secs(1);
/// Drift (pipeline seconds vs wall seconds) beyond which the stream is also rebuilt.
const DRIFT_REBUILD: Duration = Duration::from_millis(1500);
/// Smaller corrections than this are not worth a splice (callback jitter, resampler latency).
const DRIFT_IGNORE_SAMPLES: i64 = 400; // 25 ms
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
    /// Native samples received in the current rate-watch window.
    rx_total: u64,
    window_start: Instant,
    last_data: Instant,
    max_hole: Duration,
    /// Consecutive windows that disagreed with the declared rate.
    mismatch: u8,
}

/// Sample rates real devices run at; a measured rate is only trusted when it is close to one.
const STANDARD_RATES: [u32; 13] = [
    8_000, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400,
    192_000,
];

/// The standard rate within 3 % of `observed`, if any.
fn snap_rate(observed: f64) -> Option<u32> {
    STANDARD_RATES
        .iter()
        .copied()
        .map(|r| (r, (observed - f64::from(r)).abs() / f64::from(r)))
        .filter(|(_, err)| *err <= 0.03)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(r, _)| r)
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
    /// Wall-clock time of the track's first sample.
    t0: Option<Instant>,
    paused_since: Option<Instant>,
    paused_total: Duration,
    /// Output samples still to be discarded to pull a track that ran ahead back onto the clock.
    trim: usize,
    last_drift_check: Instant,
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
            t0: None,
            paused_since: None,
            paused_total: Duration::ZERO,
            trim: 0,
            last_drift_check: Instant::now(),
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
            let now = Instant::now();
            self.rate_watch(now);
            self.drift_guard(now);
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
        self.drain_active();
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
            rx_total: 0,
            window_start: Instant::now(),
            last_data: Instant::now(),
            max_hole: Duration::ZERO,
            mismatch: 0,
        });
    }

    /// Processes everything left in the current stream's ring.
    fn drain_active(&mut self) {
        while self.read_and_process() {}
        // Let the resampler release the tail it holds back so the stream covers its full duration.
        let mut tail = Vec::new();
        if let Some(a) = self.active.as_mut()
            && a.resampler.flush(&mut tail).is_ok()
            && !tail.is_empty()
        {
            self.push(&tail);
        }
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
        active.rx_total += self.in_buf.len() as u64;
        active.max_hole = active
            .max_hole
            .max(now.saturating_duration_since(active.last_data));
        active.last_data = now;
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
                Ok(p) => {
                    self.pipeline = Some(p);
                    self.t0 = Some(first_sample);
                }
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

        if self.trim > 0 {
            let n = self.trim.min(self.out_buf.len());
            self.out_buf.drain(..n);
            self.trim -= n;
        }
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
        self.pad_samples(silence_samples(gap.as_millis() as u64));
    }

    fn pad_samples(&mut self, samples: usize) {
        let mut left = samples;
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
        let now = Instant::now();
        if let Some(p) = self.pipeline.as_mut()
            && p.is_paused() != want
        {
            match p.set_paused(want, now) {
                Ok(()) if want => self.paused_since = Some(now),
                Ok(()) => {
                    if let Some(since) = self.paused_since.take() {
                        self.paused_total += now.saturating_duration_since(since);
                    }
                }
                Err(e) => self.io_error(&e.to_string()),
            }
        }
    }

    /// Measures the rate the stream really delivers (native samples per wall second) and, when it
    /// settles on a standard rate that differs from the declared one, rebuilds the resampler in
    /// place. Bluetooth headsets do this when their microphone opens (A2DP 48 kHz → HFP 16/24 kHz):
    /// the stream keeps going but at a new rate. No stream rebuild, no gap — only the audio of the
    /// last window was resampled wrongly, and [`Self::reconcile_drift`] repairs the timeline.
    fn rate_watch(&mut self, now: Instant) {
        let Some(a) = self.active.as_mut() else {
            return;
        };
        if a.idle_fill {
            return;
        }
        let elapsed = now.saturating_duration_since(a.window_start);
        if elapsed < RATE_WINDOW {
            return;
        }
        let holey = a.max_hole > RATE_MAX_HOLE;
        let observed = a.rx_total as f64 / elapsed.as_secs_f64();
        a.rx_total = 0;
        a.window_start = now;
        a.max_hole = Duration::ZERO;
        if holey {
            return;
        }
        let ratio = observed / f64::from(a.rate);
        if (0.97..=1.03).contains(&ratio) {
            a.mismatch = 0;
            return;
        }
        let Some(to) = snap_rate(observed).filter(|r| *r != a.rate) else {
            return; // a window straddling a change, or noise: wait for a clean one
        };
        a.mismatch += 1;
        let gross = !(0.75..=1.33).contains(&ratio);
        if !gross && a.mismatch < 2 {
            return;
        }
        let from = a.rate;
        match Resampler16k::new(to) {
            Ok(r) => {
                a.resampler = r;
                a.rate = to;
                a.mismatch = 0;
            }
            Err(e) => {
                self.io_error(&e.to_string());
                return;
            }
        }
        let label = format!("{} ({to} Hz)", a.device);
        tracing::warn!(track = ?self.p.kind, from, to, "capture sample rate changed mid-stream");
        let _ = self.p.events.send(RecorderEvent::RateChanged {
            track: self.p.kind,
            from_hz: from,
            to_hz: to,
        });
        if let Some(p) = self.pipeline.as_mut()
            && let Err(e) = p.note_device_change(&label)
        {
            self.io_error(&e.to_string());
        }
        self.reconcile_drift(now, DRIFT_IGNORE_SAMPLES);
    }

    /// Written samples minus what the wall clock says should exist (positive: track is behind).
    fn drift_samples(&self, now: Instant) -> Option<i64> {
        let p = self.pipeline.as_ref()?;
        if p.is_paused() {
            return None;
        }
        let elapsed = now
            .saturating_duration_since(self.t0?)
            .saturating_sub(self.paused_total);
        let expected = (elapsed.as_secs_f64() * f64::from(SAMPLE_RATE)) as i64;
        Some(expected - p.written() as i64)
    }

    /// Pads silence (track behind) or schedules a trim (track ahead) when `|drift| > threshold`.
    fn reconcile_drift(&mut self, now: Instant, threshold: i64) -> Option<i64> {
        let drift = self.drift_samples(now)?;
        if drift.abs() <= threshold {
            return None;
        }
        if drift > 0 {
            self.pad_samples(drift as usize);
        } else {
            self.trim += (-drift) as usize;
        }
        let ms = drift * 1000 / i64::from(SAMPLE_RATE);
        tracing::warn!(track = ?self.p.kind, drift_ms = ms, "track drifted from the wall clock; corrected");
        let _ = self.p.events.send(RecorderEvent::DriftCorrected {
            track: self.p.kind,
            drift_ms: ms,
        });
        Some(drift)
    }

    /// Safety net for every backend: the track must not drift from the wall clock. A large
    /// divergence also makes the supervisor rebuild the stream, which re-queries the device format.
    fn drift_guard(&mut self, now: Instant) {
        if now.saturating_duration_since(self.last_drift_check) < DRIFT_CHECK {
            return;
        }
        self.last_drift_check = now;
        if now.saturating_duration_since(self.last_arrival) > Duration::from_millis(500) {
            return; // not receiving: the stall/rebuild path owns that case
        }
        let Some(drift) = self.drift_samples(now) else {
            return;
        };
        let big = i64::try_from(DRIFT_REBUILD.as_millis()).unwrap_or(i64::MAX)
            * i64::from(SAMPLE_RATE)
            / 1000;
        if drift.abs() > big {
            self.reconcile_drift(now, 0);
            if let Some(a) = self.active.as_ref() {
                a.shared.failed.store(true, Ordering::Relaxed);
            }
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
