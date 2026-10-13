//! Capture supervision: opens a platform backend, hands its audio to the ingest worker through a
//! lock-free ring buffer, and rebuilds the stream when the device changes or fails.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use rtrb::{Consumer, Producer, RingBuffer};
use yc_core::Result;
use yc_core::meeting::TrackKind;

use crate::config::RecorderEvent;

/// Ring capacity in native-rate mono samples (≈ 11 s at 48 kHz): the worker has to stall for that
/// long before audio is lost.
pub(crate) const RING_SAMPLES: usize = 1 << 19;

/// A stream with no callbacks for this long is considered dead and rebuilt.
const STALL_AFTER: Duration = Duration::from_secs(5);
/// How often the default device is re-resolved.
const TARGET_POLL: Duration = Duration::from_secs(1);
/// Pause between failed (re)open attempts.
const RETRY_EVERY: Duration = Duration::from_secs(1);

/// State shared between a stream's callbacks and the supervisor / worker.
#[derive(Default)]
pub(crate) struct StreamShared {
    /// Set by an error callback that needs a rebuild.
    pub failed: AtomicBool,
    /// Native samples lost because the ring was full.
    pub dropped: AtomicU64,
    /// Number of data callbacks so far (stall detection).
    pub callbacks: AtomicU64,
}

/// Producer end handed to a capture callback. Everything here is allocation- and lock-free.
pub(crate) struct Sink {
    producer: Producer<f32>,
    shared: Arc<StreamShared>,
}

impl Sink {
    /// Pushes already-mono samples.
    #[inline]
    pub(crate) fn push_mono(&mut self, mono: &[f32]) {
        self.shared.callbacks.fetch_add(1, Ordering::Relaxed);
        let n = mono.len().min(self.producer.slots());
        if n > 0
            && let Ok(chunk) = self.producer.write_chunk_uninit(n)
        {
            chunk.fill_from_iter(mono[..n].iter().copied());
        }
        if n < mono.len() {
            self.shared
                .dropped
                .fetch_add((mono.len() - n) as u64, Ordering::Relaxed);
        }
    }

    /// Downmixes interleaved `data` (any sample type, via `conv`) to mono and pushes it.
    #[inline]
    pub(crate) fn push_interleaved<T: Copy>(
        &mut self,
        data: &[T],
        channels: usize,
        conv: impl Fn(T) -> f32,
    ) {
        let channels = channels.max(1);
        if channels == 1 {
            let mut buf = [0.0f32; 1024];
            for part in data.chunks(buf.len()) {
                for (o, &s) in buf.iter_mut().zip(part) {
                    *o = conv(s);
                }
                self.push_mono(&buf[..part.len()]);
            }
            return;
        }
        let inv = 1.0 / channels as f32;
        let mut buf = [0.0f32; 1024];
        for part in data.chunks(buf.len() * channels) {
            let mut k = 0;
            for frame in part.chunks_exact(channels) {
                let mut acc = 0.0;
                for &s in frame {
                    acc += conv(s);
                }
                buf[k] = acc * inv;
                k += 1;
            }
            self.push_mono(&buf[..k]);
        }
    }

    pub(crate) fn shared_handle(&self) -> Arc<StreamShared> {
        Arc::clone(&self.shared)
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn mark_failed(&self) {
        self.shared.failed.store(true, Ordering::Relaxed);
    }
}

/// Information about a freshly opened stream.
pub(crate) struct OpenInfo {
    pub device: String,
    pub sample_rate: u32,
    /// Bluetooth headset that was avoided in favour of `device` (microphone policy).
    pub substituted_for: Option<String>,
}

/// A platform capture source.
pub(crate) trait Backend {
    /// RAII handle of the live stream; dropping it stops capture. May be `!Send`.
    type Guard;

    /// Opens the stream and starts delivering into `sink`.
    fn open(&mut self, sink: Sink) -> Result<(Self::Guard, OpenInfo)>;

    /// Identity of the device a fresh `open` would pick right now (the current default for
    /// "follow the default" sources). A change triggers a rebuild.
    fn target(&self) -> Option<String>;

    /// Whether a silent callback stream means the stream is dead.
    fn stall_detect(&self) -> bool {
        true
    }

    /// Whether the source stops delivering during silence (WASAPI loopback), so the worker must
    /// synthesise silence from the wall clock.
    fn idle_fill(&self) -> bool {
        false
    }
}

/// A new stream announcing itself to the ingest worker.
pub(crate) struct StreamStart {
    pub consumer: Consumer<f32>,
    pub sample_rate: u32,
    pub device: String,
    pub shared: Arc<StreamShared>,
    pub idle_fill: bool,
}

fn sleep_checking(stop: &AtomicBool, total: Duration) {
    let end = Instant::now() + total;
    while !stop.load(Ordering::Relaxed) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Supervisor loop; runs on its own thread until `stop`.
pub(crate) fn supervise<B: Backend>(
    kind: TrackKind,
    mut backend: B,
    streams: Sender<StreamStart>,
    events: Sender<RecorderEvent>,
    stop: Arc<AtomicBool>,
    ready: Sender<Result<String>>,
) {
    let mut ready = Some(ready);
    let mut last_error: Option<String> = None;
    let mut opened_before = false;

    while !stop.load(Ordering::Relaxed) {
        let (producer, consumer) = RingBuffer::<f32>::new(RING_SAMPLES);
        let shared = Arc::new(StreamShared::default());
        let sink = Sink {
            producer,
            shared: Arc::clone(&shared),
        };

        match backend.open(sink) {
            Ok((guard, info)) => {
                last_error = None;
                let opened_target = backend.target();
                let device = info.device.clone();
                let substituted_for = info.substituted_for.clone();
                let _ = streams.send(StreamStart {
                    consumer,
                    sample_rate: info.sample_rate,
                    device: info.device,
                    shared: Arc::clone(&shared),
                    idle_fill: backend.idle_fill(),
                });
                let ev = if opened_before {
                    RecorderEvent::DeviceChanged {
                        track: kind,
                        device: device.clone(),
                    }
                } else {
                    RecorderEvent::TrackStarted {
                        track: kind,
                        device: device.clone(),
                    }
                };
                opened_before = true;
                let _ = events.send(ev);
                if let Some(headset) = substituted_for {
                    let _ = events.send(RecorderEvent::MicSubstituted {
                        bluetooth: headset,
                        using: device.clone(),
                    });
                }
                if let Some(r) = ready.take() {
                    let _ = r.send(Ok(device));
                }

                let mut last_cb = 0;
                let mut last_progress = Instant::now();
                let mut last_poll = Instant::now();
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(50));
                    if shared.failed.load(Ordering::Relaxed) {
                        tracing::warn!(?kind, "capture stream failed; rebuilding");
                        break;
                    }
                    let cb = shared.callbacks.load(Ordering::Relaxed);
                    if cb != last_cb {
                        last_cb = cb;
                        last_progress = Instant::now();
                    } else if backend.stall_detect() && last_progress.elapsed() > STALL_AFTER {
                        tracing::warn!(?kind, "capture stream stalled; rebuilding");
                        break;
                    }
                    if last_poll.elapsed() >= TARGET_POLL {
                        last_poll = Instant::now();
                        let now = backend.target();
                        if now.is_some() && now != opened_target {
                            tracing::info!(?kind, ?now, "capture device changed; rebuilding");
                            break;
                        }
                    }
                }
                drop(guard);
            }
            Err(e) => {
                let msg = e.to_string();
                if let Some(r) = ready.take() {
                    let _ = r.send(Err(e));
                }
                if last_error.as_deref() != Some(msg.as_str()) {
                    let _ = events.send(RecorderEvent::Error {
                        track: kind,
                        message: msg.clone(),
                    });
                    last_error = Some(msg);
                }
                sleep_checking(&stop, RETRY_EVERY);
            }
        }
    }
}
