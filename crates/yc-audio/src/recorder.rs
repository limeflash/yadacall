//! The recorder: owns the capture supervisors and ingest workers of both tracks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use tokio::sync::watch;
use yc_core::events::Level;
use yc_core::meeting::{TrackInfo, TrackKind};
use yc_core::{Error, Result};

use crate::capture::cpal_backend::{CpalBackend, Source};
use crate::config::{AudioFrame, RecorderConfig, RecorderEvent};
use crate::disk;
use crate::ingest::{Anchor, Ingest, IngestParams};
use crate::sleep::{SleepGuard, prevent_sleep};
use crate::supervisor::{Backend, supervise};

/// How long [`Recorder::start`] waits for a track's first stream (covers the TCC prompt).
const OPEN_TIMEOUT: Duration = Duration::from_secs(120);
/// Free space required at start: one hour of the requested tracks.
const START_HEADROOM: Duration = Duration::from_secs(3600);

/// Entry point; see [`Recorder::start`].
pub struct Recorder;

struct Track {
    kind: TrackKind,
    supervisor_stop: Arc<AtomicBool>,
    worker_stop: Arc<AtomicBool>,
    supervisor: Option<JoinHandle<()>>,
    worker: Option<JoinHandle<Result<Option<TrackInfo>>>>,
    levels: watch::Receiver<Level>,
    frames: Receiver<AudioFrame>,
}

/// Live input levels of the tracks being recorded (`rms`/`peak`, linear 0..=1, ~25 Hz).
#[derive(Clone)]
pub struct Levels {
    mic: Option<watch::Receiver<Level>>,
    system: Option<watch::Receiver<Level>>,
}

impl Levels {
    /// Latest level of `kind`, or `None` if that track is not being recorded.
    pub fn get(&self, kind: TrackKind) -> Option<Level> {
        self.receiver(kind).map(|r| *r.borrow())
    }

    /// A `tokio::sync::watch` receiver for `kind` (`changed().await` or poll `borrow()`).
    pub fn receiver(&self, kind: TrackKind) -> Option<watch::Receiver<Level>> {
        match kind {
            TrackKind::Mic => self.mic.clone(),
            TrackKind::System => self.system.clone(),
        }
    }
}

/// Handle of a running recording. Dropping it stops the recording (best effort); call
/// [`RecorderHandle::stop`] to get the track summaries.
pub struct RecorderHandle {
    anchor: Anchor,
    tracks: Vec<Track>,
    events_rx: Receiver<RecorderEvent>,
    events_tx: Sender<RecorderEvent>,
    paused: Arc<AtomicBool>,
    _sleep: Option<SleepGuard>,
    stopped: bool,
}

impl Recorder {
    /// Starts recording into `cfg.dir`.
    ///
    /// Succeeds if at least one requested track opened; a track that failed (microphone unplugged,
    /// permission pending) keeps retrying in the background and reports through [`RecorderEvent::Error`].
    /// Fails with the first error when no track could be opened, and with [`Error::DiskFull`] when
    /// less than an hour of space is free.
    pub fn start(cfg: RecorderConfig) -> Result<RecorderHandle> {
        let n_tracks = usize::from(cfg.mic.is_some()) + usize::from(cfg.system);
        if n_tracks == 0 {
            return Err(Error::Invalid(
                "neither microphone nor system audio requested".into(),
            ));
        }
        std::fs::create_dir_all(&cfg.dir)?;
        if !cfg.skip_disk_check {
            disk::check_free_space(&cfg.dir, START_HEADROOM, n_tracks)?;
        }

        let anchor = Anchor::now();
        let (events_tx, events_rx) = crossbeam_channel::unbounded();
        let paused = Arc::new(AtomicBool::new(false));
        let sleep = if cfg.prevent_sleep {
            match prevent_sleep("YadaCall is recording a call") {
                Ok(g) => Some(g),
                Err(e) => {
                    tracing::warn!("{e}");
                    None
                }
            }
        } else {
            None
        };

        let mut handle = RecorderHandle {
            anchor,
            tracks: Vec::new(),
            events_rx,
            events_tx: events_tx.clone(),
            paused: Arc::clone(&paused),
            _sleep: sleep,
            stopped: false,
        };

        let mut ready: Vec<(TrackKind, Receiver<Result<String>>)> = Vec::new();
        if let Some(sel) = cfg.mic.clone() {
            let (track, rx) = spawn_track(
                TrackKind::Mic,
                move || CpalBackend::new(Source::Input(sel)),
                &cfg,
                anchor,
                &paused,
                &events_tx,
            )?;
            handle.tracks.push(track);
            ready.push((TrackKind::Mic, rx));
        }
        if cfg.system {
            let (track, rx) = spawn_system(&cfg, anchor, &paused, &events_tx)?;
            handle.tracks.push(track);
            ready.push((TrackKind::System, rx));
        }

        let mut first_error: Option<Error> = None;
        let mut opened = 0;
        for (kind, rx) in ready {
            match rx.recv_timeout(OPEN_TIMEOUT) {
                Ok(Ok(_)) => opened += 1,
                Ok(Err(e)) => {
                    tracing::warn!(?kind, "track failed to open: {e}");
                    first_error.get_or_insert(e);
                }
                Err(_) => {
                    first_error.get_or_insert(Error::Device(format!(
                        "{kind:?} capture did not start in time"
                    )));
                }
            }
        }
        if opened == 0 {
            let _ = handle.finish();
            return Err(
                first_error.unwrap_or_else(|| Error::Device("no track could be opened".into()))
            );
        }
        Ok(handle)
    }
}

fn spawn_track<B, F>(
    kind: TrackKind,
    make: F,
    cfg: &RecorderConfig,
    anchor: Anchor,
    paused: &Arc<AtomicBool>,
    events: &Sender<RecorderEvent>,
) -> Result<(Track, Receiver<Result<String>>)>
where
    B: Backend,
    F: FnOnce() -> B + Send + 'static,
{
    let (streams_tx, streams_rx) = crossbeam_channel::unbounded();
    let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
    let (level_tx, levels) = watch::channel(Level::default());
    let (frames_tx, frames) = crossbeam_channel::bounded(cfg.frame_queue.max(1));
    let supervisor_stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::new(AtomicBool::new(false));

    let ingest = Ingest::new(IngestParams {
        kind,
        dir: cfg.dir.clone(),
        anchor,
        silence: cfg.silence,
        frame_len: cfg.frame_len,
        streams: streams_rx,
        stop: Arc::clone(&worker_stop),
        paused: Arc::clone(paused),
        level_tx,
        events: events.clone(),
        frames_tx,
    });
    let worker = std::thread::Builder::new()
        .name(format!("yc-{}-ingest", kind.file_stem()))
        .spawn(move || ingest.run())
        .map_err(Error::Io)?;

    let sup_stop = Arc::clone(&supervisor_stop);
    let sup_events = events.clone();
    let supervisor = std::thread::Builder::new()
        .name(format!("yc-{}-capture", kind.file_stem()))
        .spawn(move || supervise(kind, make(), streams_tx, sup_events, sup_stop, ready_tx))
        .map_err(Error::Io)?;

    Ok((
        Track {
            kind,
            supervisor_stop,
            worker_stop,
            supervisor: Some(supervisor),
            worker: Some(worker),
            levels,
            frames,
        },
        ready_rx,
    ))
}

#[cfg(target_os = "macos")]
fn spawn_system(
    cfg: &RecorderConfig,
    anchor: Anchor,
    paused: &Arc<AtomicBool>,
    events: &Sender<RecorderEvent>,
) -> Result<(Track, Receiver<Result<String>>)> {
    spawn_track(
        TrackKind::System,
        || crate::capture::macos_tap::TapBackend,
        cfg,
        anchor,
        paused,
        events,
    )
}

#[cfg(target_os = "windows")]
fn spawn_system(
    cfg: &RecorderConfig,
    anchor: Anchor,
    paused: &Arc<AtomicBool>,
    events: &Sender<RecorderEvent>,
) -> Result<(Track, Receiver<Result<String>>)> {
    spawn_track(
        TrackKind::System,
        || CpalBackend::new(Source::Loopback),
        cfg,
        anchor,
        paused,
        events,
    )
}

#[cfg(target_os = "linux")]
fn spawn_system(
    cfg: &RecorderConfig,
    anchor: Anchor,
    paused: &Arc<AtomicBool>,
    events: &Sender<RecorderEvent>,
) -> Result<(Track, Receiver<Result<String>>)> {
    spawn_track(
        TrackKind::System,
        || crate::capture::linux_monitor::MonitorBackend,
        cfg,
        anchor,
        paused,
        events,
    )
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn spawn_system(
    _cfg: &RecorderConfig,
    _anchor: Anchor,
    _paused: &Arc<AtomicBool>,
    _events: &Sender<RecorderEvent>,
) -> Result<(Track, Receiver<Result<String>>)> {
    Err(Error::Device(
        "system audio capture is not supported on this platform".into(),
    ))
}

impl RecorderHandle {
    /// Stops writing; levels, the watchdog and capture keep running so the UI stays live.
    /// Pauses are recorded in the track json, the audio file stays gapless.
    pub fn pause(&self) {
        if !self.paused.swap(true, Ordering::Relaxed) {
            let _ = self.events_tx.send(RecorderEvent::Paused);
        }
    }

    /// Resumes writing after [`Self::pause`].
    pub fn resume(&self) {
        if self.paused.swap(false, Ordering::Relaxed) {
            let _ = self.events_tx.send(RecorderEvent::Resumed);
        }
    }

    /// Whether the recording is paused.
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Live levels of both tracks.
    pub fn levels(&self) -> Levels {
        let get = |k| {
            self.tracks
                .iter()
                .find(|t| t.kind == k)
                .map(|t| t.levels.clone())
        };
        Levels {
            mic: get(TrackKind::Mic),
            system: get(TrackKind::System),
        }
    }

    /// Channel of 16 kHz mono f32 frames of `kind` for live VAD/ASR. Bounded: when the consumer
    /// lags, frames are dropped (and counted in [`RecorderEvent::FramesDropped`]) — capture and the
    /// file on disk are never affected.
    pub fn frames(&self, kind: TrackKind) -> Option<Receiver<AudioFrame>> {
        self.tracks
            .iter()
            .find(|t| t.kind == kind)
            .map(|t| t.frames.clone())
    }

    /// Stream of [`RecorderEvent`]s (device changes, silence warnings, errors, …).
    pub fn events(&self) -> Receiver<RecorderEvent> {
        self.events_rx.clone()
    }

    /// Time since the meeting anchor (the shared origin of both tracks' offsets).
    pub fn elapsed(&self) -> Duration {
        self.anchor.instant.elapsed()
    }

    /// Wall-clock time of the meeting anchor.
    pub fn started_at(&self) -> std::time::SystemTime {
        self.anchor.wall
    }

    /// Stops capture, flushes everything to disk and returns one [`TrackInfo`] per track that
    /// produced audio.
    pub fn stop(mut self) -> Result<Vec<TrackInfo>> {
        self.finish()
    }

    fn finish(&mut self) -> Result<Vec<TrackInfo>> {
        if self.stopped {
            return Ok(Vec::new());
        }
        self.stopped = true;
        for t in &self.tracks {
            t.supervisor_stop.store(true, Ordering::Relaxed);
        }
        for t in &mut self.tracks {
            if let Some(h) = t.supervisor.take() {
                let _ = h.join();
            }
        }
        // Producers are gone: whatever is left in the rings is final.
        for t in &self.tracks {
            t.worker_stop.store(true, Ordering::Release);
        }
        let mut infos = Vec::new();
        let mut first_error = None;
        for t in &mut self.tracks {
            let Some(h) = t.worker.take() else { continue };
            match h.join() {
                Ok(Ok(Some(info))) => infos.push(info),
                Ok(Ok(None)) => {}
                Ok(Err(e)) => {
                    first_error.get_or_insert(e);
                }
                Err(_) => {
                    first_error.get_or_insert(Error::Storage("ingest worker panicked".into()));
                }
            }
        }
        match first_error {
            Some(e) if infos.is_empty() => Err(e),
            Some(e) => {
                tracing::error!("track finalisation error: {e}");
                Ok(infos)
            }
            None => Ok(infos),
        }
    }
}

impl Drop for RecorderHandle {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU32;
    use std::time::Instant;

    use super::*;
    use crate::supervisor::{OpenInfo, Sink};
    use crate::writer::{meta_path, pcm_path, read_meta};

    /// A fake device: a real-time sine generator whose "default device" can be switched.
    struct FakeBackend {
        rate: u32,
        current: Arc<AtomicU32>,
        open_delay: Duration,
    }

    struct FakeStream {
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl Drop for FakeStream {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    impl Backend for FakeBackend {
        type Guard = FakeStream;

        fn open(&mut self, mut sink: Sink) -> Result<(FakeStream, OpenInfo)> {
            std::thread::sleep(self.open_delay);
            let stop = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&stop);
            let rate = self.rate;
            let thread = std::thread::spawn(move || {
                let block = (rate / 100) as usize;
                let mut n = 0u64;
                let start = Instant::now();
                while !flag.load(Ordering::Relaxed) {
                    let samples: Vec<f32> = (0..block)
                        .map(|i| {
                            0.5 * (2.0 * std::f32::consts::PI * 440.0 * (n + i as u64) as f32
                                / rate as f32)
                                .sin()
                        })
                        .collect();
                    n += block as u64;
                    sink.push_mono(&samples);
                    let due = start + Duration::from_secs_f64(n as f64 / f64::from(rate));
                    std::thread::sleep(due.saturating_duration_since(Instant::now()));
                }
            });
            let id = self.current.load(Ordering::Relaxed);
            Ok((
                FakeStream {
                    stop,
                    thread: Some(thread),
                },
                OpenInfo {
                    device: format!("Fake {id}"),
                    sample_rate: rate,
                },
            ))
        }

        fn target(&self) -> Option<String> {
            Some(format!("dev{}", self.current.load(Ordering::Relaxed)))
        }
    }

    #[test]
    fn device_switch_mid_call_keeps_one_gapless_track() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = RecorderConfig::new(dir.path());
        cfg.prevent_sleep = false;
        cfg.skip_disk_check = true;
        let anchor = Anchor::now();
        let (events_tx, events_rx) = crossbeam_channel::unbounded();
        let paused = Arc::new(AtomicBool::new(false));
        let current = Arc::new(AtomicU32::new(1));
        let backend_state = Arc::clone(&current);
        let (track, ready) = spawn_track(
            TrackKind::Mic,
            move || FakeBackend {
                rate: 44_100,
                current: backend_state,
                open_delay: Duration::from_millis(400),
            },
            &cfg,
            anchor,
            &paused,
            &events_tx,
        )
        .unwrap();
        let mut handle = RecorderHandle {
            anchor,
            tracks: vec![track],
            events_rx: events_rx.clone(),
            events_tx,
            paused,
            _sleep: None,
            stopped: false,
        };
        assert_eq!(
            ready.recv_timeout(Duration::from_secs(5)).unwrap().unwrap(),
            "Fake 1"
        );

        std::thread::sleep(Duration::from_millis(1200));
        let lvl = handle.levels().get(TrackKind::Mic).unwrap();
        assert!((lvl.peak - 0.5).abs() < 0.05, "peak {lvl:?}");
        let frames = handle.frames(TrackKind::Mic).unwrap();
        assert!(frames.len() > 20, "live frames flow: {}", frames.len());

        // The user plugs in other headphones: the default input changes, the stream is rebuilt
        // (taking 400 ms here) and the file keeps going.
        current.store(2, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(2200));
        handle.pause();
        std::thread::sleep(Duration::from_millis(600));
        handle.resume();
        std::thread::sleep(Duration::from_millis(500));

        let total = handle.anchor.instant.elapsed();
        let infos = handle.finish().unwrap();
        assert_eq!(infos.len(), 1);

        let events: Vec<_> = events_rx.try_iter().collect();
        assert!(
            events.contains(&RecorderEvent::DeviceChanged {
                track: TrackKind::Mic,
                device: "Fake 2".into()
            }),
            "{events:?}"
        );
        assert!(
            events.contains(&RecorderEvent::Paused) && events.contains(&RecorderEvent::Resumed)
        );

        // Wall time minus the 0.6 s pause: the device-switch gap is padded with silence, so the
        // track stays on the wall clock.
        let expect = (total.as_secs_f64() - 0.6) * 16_000.0 - infos[0].offset_ms as f64 * 16.0;
        let got = infos[0].samples as f64;
        assert!(
            (got - expect).abs() < 16_000.0 * 0.35,
            "got {got} samples, expected ≈ {expect}"
        );
        let len = std::fs::metadata(pcm_path(dir.path(), TrackKind::Mic))
            .unwrap()
            .len();
        assert_eq!(len, infos[0].samples * 2);

        let meta = read_meta(&meta_path(dir.path(), TrackKind::Mic)).unwrap();
        assert!(meta.clean_stop);
        assert_eq!(meta.device, "Fake 1");
        assert_eq!(meta.device_changes.len(), 1);
        assert_eq!(meta.device_changes[0].device, "Fake 2");
        assert_eq!(meta.pauses.len(), 1);
        assert!(
            (500..800).contains(&meta.pauses[0].duration_ms),
            "{:?}",
            meta.pauses
        );
    }
}
