//! A recording in progress: capture, live VAD → ASR, levels and recorder events.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use tokio::sync::broadcast;
use yc_audio::writer::{PauseSpan, meta_path, read_meta};
use yc_audio::{AudioFrame, DeviceSel, RecorderConfig, RecorderEvent};
use yc_core::events::{EngineEvent, Level, NoticeLevel, RecorderState};
use yc_core::meeting::{Bookmark, MeetingId, MeetingStatus, ParticipantHint, Stage, TrackKind};
use yc_core::ports::ModelInfo;
use yc_core::transcript::Segment;
use yc_core::{Error, Result, samples_to_ms};

use crate::asr::{AsrQueue, AsrWorker};
use crate::engine::Engine;
use crate::livelog::{LiveLine, LiveLog};
use crate::meta::{LIVE_FILE, SessionMeta, Source};
use crate::ports::{RecordingHandle, VadFactory};
use crate::regions::RegionExtractor;
use crate::timeline::file_to_track_time;

/// Which microphone to record.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MicChoice {
    /// The configured device (settings), else the system default.
    #[default]
    Configured,
    /// A specific device id or name.
    Device(String),
    /// Record only the system audio.
    Off,
}

/// Options of [`Engine::start_recording`].
#[derive(Debug, Clone)]
pub struct StartOptions {
    /// Meeting title; empty/`None` = "Встреча" with the time, replaceable by the summary's title.
    pub title: Option<String>,
    /// Expected number of remote speakers; invite-derived values are an upper bound.
    pub expected_speakers: Option<u8>,
    /// Whether the user explicitly selected `expected_speakers` in the recorder.
    pub expected_speakers_explicit: bool,
    /// Calendar invitees that may correspond to the remote speakers.
    pub participant_hints: Vec<ParticipantHint>,
    pub mic: MicChoice,
    /// Record the system audio ("them").
    pub system_audio: bool,
    /// Overrides the meeting language setting (`ru`, `en`, `auto`).
    pub language: Option<String>,
}

impl Default for StartOptions {
    fn default() -> Self {
        Self {
            title: None,
            expected_speakers: None,
            expected_speakers_explicit: false,
            participant_hints: Vec::new(),
            mic: MicChoice::Configured,
            system_audio: true,
            language: None,
        }
    }
}

/// Snapshot of the running recording.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordingInfo {
    pub meeting: MeetingId,
    pub paused: bool,
    pub elapsed_ms: u64,
}

fn idx(kind: TrackKind) -> usize {
    match kind {
        TrackKind::Mic => 0,
        TrackKind::System => 1,
    }
}

/// Facts about the live timeline that the feeders and the ASR worker share.
pub(crate) struct LiveClock {
    dir: PathBuf,
    seen: [AtomicU64; 2],
    offsets: [AtomicU64; 2],
    pauses: Mutex<[Vec<PauseSpan>; 2]>,
}

impl LiveClock {
    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            seen: [AtomicU64::new(0), AtomicU64::new(0)],
            offsets: [AtomicU64::new(u64::MAX), AtomicU64::new(u64::MAX)],
            pauses: Mutex::new([Vec::new(), Vec::new()]),
        }
    }

    /// Offset of the track's first sample from the meeting start (read from `<track>.json` once the
    /// recorder created it; 0 until then).
    fn offset(&self, kind: TrackKind) -> u64 {
        let cached = self.offsets[idx(kind)].load(Ordering::Relaxed);
        if cached != u64::MAX {
            return cached;
        }
        match read_meta(&meta_path(&self.dir, kind)) {
            Ok(m) => {
                self.offsets[idx(kind)].store(m.offset_ms, Ordering::Relaxed);
                m.offset_ms
            }
            Err(_) => 0,
        }
    }

    /// Meeting-timeline time of a file position of `kind`.
    fn to_meeting_ms(&self, kind: TrackKind, file_ms: u64) -> u64 {
        let pauses = self
            .pauses
            .lock()
            .map(|p| p[idx(kind)].clone())
            .unwrap_or_default();
        self.offset(kind) + file_to_track_time(file_ms, &pauses)
    }

    fn note_pause(&self, duration: Duration) {
        if let Ok(mut p) = self.pauses.lock() {
            for kind in TrackKind::ALL {
                p[idx(kind)].push(PauseSpan {
                    at_ms: samples_to_ms(self.seen[idx(kind)].load(Ordering::Relaxed)),
                    duration_ms: duration.as_millis() as u64,
                });
            }
        }
    }
}

type Notifier = Arc<dyn Fn(NoticeLevel, String) + Send + Sync>;

/// All state of a running recording (owned by the engine while it lasts).
pub(crate) struct Session {
    pub id: MeetingId,
    pub dir: PathBuf,
    pub language: String,
    handle: Arc<Mutex<Option<Box<dyn RecordingHandle>>>>,
    feeders: Vec<JoinHandle<()>>,
    queue: Arc<AsrQueue>,
    worker: Option<JoinHandle<()>>,
    model: Arc<Mutex<Option<ModelInfo>>>,
    monitor: Option<JoinHandle<()>>,
    monitor_stop: Arc<AtomicBool>,
    abort: Arc<AtomicBool>,
    clock: Arc<LiveClock>,
    meta: SessionMeta,
    paused_since: Option<Instant>,
}

impl Drop for Session {
    /// Dropping a session that was not stopped behaves like a crash: threads end, files stay.
    fn drop(&mut self) {
        self.abort.store(true, Ordering::Relaxed);
        self.monitor_stop.store(true, Ordering::Relaxed);
        self.queue.abort();
        if let Ok(mut h) = self.handle.lock() {
            h.take();
        }
        for f in self.feeders.drain(..) {
            let _ = f.join();
        }
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        if let Some(m) = self.monitor.take() {
            let _ = m.join();
        }
    }
}

/// Everything `finish` learned that finalisation needs.
pub(crate) struct Finished {
    pub dir: PathBuf,
    pub language: String,
}

impl Engine {
    /// Starts recording and returns the new meeting's id. Fails with `Error::Invalid` when a
    /// recording is already running, and with the recorder's error (permission, no device, disk
    /// full) when capture cannot start — in which case nothing is left on disk.
    pub async fn start_recording(self: &Arc<Self>, opts: StartOptions) -> Result<MeetingId> {
        if self.starting.swap(true, Ordering::SeqCst) {
            return Err(Error::Invalid("a recording is already starting".into()));
        }
        let this = Arc::clone(self);
        let result = tokio::task::spawn_blocking(move || this.start_blocking(opts))
            .await
            .map_err(crate::engine::join_error)
            .and_then(|r| r);
        self.starting.store(false, Ordering::SeqCst);
        result
    }

    fn start_blocking(self: &Arc<Self>, opts: StartOptions) -> Result<MeetingId> {
        if self
            .active
            .lock()
            .map_err(|_| Error::Storage("session lock poisoned".into()))?
            .is_some()
        {
            return Err(Error::Invalid("a recording is already running".into()));
        }
        let settings = self.settings();
        let id = MeetingId::new();
        let dir = self.recording_root().join(id.to_string());
        std::fs::create_dir_all(&dir)?;
        let language = opts
            .language
            .clone()
            .unwrap_or_else(|| settings.meeting_language.code().to_owned());
        let started_at = Utc::now();
        let (title, title_auto) = match opts.title.as_deref().map(str::trim) {
            Some(t) if !t.is_empty() => (t.to_owned(), false),
            _ => (default_title(started_at, settings.russian()), true),
        };
        let participant_hints = opts
            .participant_hints
            .into_iter()
            .filter_map(|mut hint| {
                hint.name = hint.name.trim().to_owned();
                if hint.name.is_empty() {
                    return None;
                }
                hint.email = hint
                    .email
                    .map(|email| email.trim().to_owned())
                    .filter(|email| !email.is_empty());
                Some(hint)
            })
            .collect();
        let meta = SessionMeta {
            id,
            title,
            title_auto,
            started_at,
            language: language.clone(),
            expected_speakers: opts.expected_speakers,
            expected_speakers_explicit: opts.expected_speakers_explicit,
            source: Source::Recording,
            bookmarks: Vec::new(),
            participant_hints,
        };
        meta.write(&dir)?;

        let mut config = RecorderConfig::new(&dir);
        config.mic = match &opts.mic {
            MicChoice::Off => None,
            MicChoice::Device(d) => Some(DeviceSel::Named(d.clone())),
            MicChoice::Configured => Some(resolve_mic(&settings)),
        };
        config.prefer_builtin_mic = settings.prefer_builtin_mic;
        config.system = opts.system_audio;
        config.prevent_sleep = self.cfg.prevent_sleep;
        config.skip_disk_check = self.cfg.skip_disk_check;
        let handle = match self.ports.recorder.start(config) {
            Ok(h) => h,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e);
            }
        };

        match self.spawn_session(id, dir.clone(), language, meta, handle) {
            Ok(session) => {
                self.emit(EngineEvent::Recorder {
                    meeting: id,
                    state: RecorderState::Recording,
                    elapsed_ms: 0,
                });
                self.emit(EngineEvent::Status {
                    meeting: id,
                    status: MeetingStatus::Recording,
                });
                *self
                    .active
                    .lock()
                    .map_err(|_| Error::Storage("session lock poisoned".into()))? = Some(session);
                Ok(id)
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                Err(e)
            }
        }
    }

    fn spawn_session(
        self: &Arc<Self>,
        id: MeetingId,
        dir: PathBuf,
        language: String,
        meta: SessionMeta,
        handle: Box<dyn RecordingHandle>,
    ) -> Result<Session> {
        let events = self.events_sender();
        let russian = self.settings().russian();
        let notify: Notifier = {
            let events = events.clone();
            Arc::new(move |level, message| {
                let _ = events.send(EngineEvent::Notice { level, message });
            })
        };
        let log = Arc::new(LiveLog::open(&dir.join(LIVE_FILE))?);
        let clock = Arc::new(LiveClock::new(dir.clone()));
        let queue = Arc::new(AsrQueue::new());
        let model = Arc::new(Mutex::new(None));
        let abort = Arc::new(AtomicBool::new(false));

        // One shared ASR worker (single model instance); loads while the call begins.
        let worker = {
            let clock = Arc::clone(&clock);
            let events = events.clone();
            let notify = Arc::clone(&notify);
            let reported = Arc::new(AtomicBool::new(false));
            AsrWorker {
                queue: Arc::clone(&queue),
                factory: Arc::clone(&self.ports.asr),
                language: language.clone(),
                log: Arc::clone(&log),
                on_segment: Box::new(move |mut seg: Segment| {
                    let kind = seg.track;
                    seg.start_ms = clock.to_meeting_ms(kind, seg.start_ms);
                    seg.end_ms = clock.to_meeting_ms(kind, seg.end_ms);
                    for w in &mut seg.words {
                        w.start_ms = clock.to_meeting_ms(kind, w.start_ms);
                        w.end_ms = clock.to_meeting_ms(kind, w.end_ms);
                    }
                    let _ = events.send(EngineEvent::LiveSegment {
                        meeting: id,
                        segment: seg,
                    });
                }),
                on_error: Box::new(move |e| {
                    if !reported.swap(true, Ordering::Relaxed) {
                        notify(
                            NoticeLevel::Warning,
                            if russian {
                                format!("Расшифровка во время звонка недоступна: {e}. Запись продолжается, текст появится после остановки.")
                            } else {
                                format!("Live transcription is unavailable: {e}. Recording continues; the text will appear after you stop.")
                            },
                        );
                    }
                }),
                model: Arc::clone(&model),
                stop: Arc::clone(&abort),
            }
            .spawn()
            .map_err(Error::Io)?
        };

        let mut feeders = Vec::new();
        for kind in TrackKind::ALL {
            let Some(rx) = handle.frames(kind) else {
                continue;
            };
            let feeder = Feeder {
                kind,
                rx,
                vad: Arc::clone(&self.ports.vad),
                queue: Arc::clone(&queue),
                log: Arc::clone(&log),
                clock: Arc::clone(&clock),
                abort: Arc::clone(&abort),
                notify: Arc::clone(&notify),
                russian,
            };
            feeders.push(
                std::thread::Builder::new()
                    .name(format!("yc-{}-live", kind.file_stem()))
                    .spawn(move || {
                        if let Err(e) = feeder.run() {
                            tracing::error!("live feeder failed: {e}");
                        }
                    })
                    .map_err(Error::Io)?,
            );
        }

        let handle = Arc::new(Mutex::new(Some(handle)));
        let monitor_stop = Arc::new(AtomicBool::new(false));
        let monitor = {
            let handle = Arc::clone(&handle);
            let stop = Arc::clone(&monitor_stop);
            let monitor = Monitor {
                id,
                events,
                notify,
                russian,
                warned_drop: false,
                silent_noticed: [false; 2],
            };
            std::thread::Builder::new()
                .name("yc-monitor".into())
                .spawn(move || monitor.run(&handle, &stop))
                .map_err(Error::Io)?
        };

        Ok(Session {
            id,
            dir,
            language,
            handle,
            feeders,
            queue,
            worker: Some(worker),
            model,
            monitor: Some(monitor),
            monitor_stop,
            abort,
            clock,
            meta,
            paused_since: None,
        })
    }

    /// The running recording, if any.
    pub fn recording(&self) -> Option<RecordingInfo> {
        let guard = self.active.lock().ok()?;
        let s = guard.as_ref()?;
        let elapsed = s
            .handle
            .lock()
            .ok()
            .and_then(|h| h.as_ref().map(|h| h.elapsed()))
            .unwrap_or_default();
        Some(RecordingInfo {
            meeting: s.id,
            paused: s.paused_since.is_some(),
            elapsed_ms: elapsed.as_millis() as u64,
        })
    }

    /// Pauses writing; the call continues but nothing is recorded or transcribed until resumed.
    pub fn pause_recording(&self) -> Result<()> {
        let mut guard = self.active_guard()?;
        let s = guard.as_mut().ok_or_else(not_recording)?;
        if s.paused_since.is_some() {
            return Ok(());
        }
        let elapsed = match s.handle.lock() {
            Ok(g) => match g.as_ref() {
                Some(h) => {
                    h.pause();
                    h.elapsed()
                }
                None => return Ok(()),
            },
            Err(_) => return Ok(()),
        };
        s.paused_since = Some(Instant::now());
        self.emit(EngineEvent::Recorder {
            meeting: s.id,
            state: RecorderState::Paused,
            elapsed_ms: elapsed.as_millis() as u64,
        });
        Ok(())
    }

    /// Resumes after [`Engine::pause_recording`].
    pub fn resume_recording(&self) -> Result<()> {
        let mut guard = self.active_guard()?;
        let s = guard.as_mut().ok_or_else(not_recording)?;
        let Some(since) = s.paused_since.take() else {
            return Ok(());
        };
        s.clock.note_pause(since.elapsed());
        let mut elapsed = Duration::ZERO;
        if let Ok(g) = s.handle.lock()
            && let Some(h) = g.as_ref()
        {
            h.resume();
            elapsed = h.elapsed();
        }
        self.emit(EngineEvent::Recorder {
            meeting: s.id,
            state: RecorderState::Recording,
            elapsed_ms: elapsed.as_millis() as u64,
        });
        Ok(())
    }

    /// Drops a bookmark at the current moment; returns it.
    pub fn bookmark(&self, label: Option<String>) -> Result<Bookmark> {
        let mut guard = self.active_guard()?;
        let s = guard.as_mut().ok_or_else(not_recording)?;
        let at_ms = s
            .handle
            .lock()
            .ok()
            .and_then(|h| h.as_ref().map(|h| h.elapsed().as_millis() as u64))
            .unwrap_or(0);
        let label = label.map(|l| l.trim().to_owned()).filter(|l| !l.is_empty());
        let bookmark = Bookmark { at_ms, label };
        s.meta.bookmarks.push(bookmark.clone());
        s.meta.write(&s.dir)?;
        Ok(bookmark)
    }

    /// Stops capture, deletes everything recorded and forgets the meeting.
    pub async fn cancel_recording(self: &Arc<Self>) -> Result<()> {
        let session = self.take_session()?;
        let id = session.id;
        let dir = session.dir.clone();
        self.blocking(move |_| {
            drop(session); // stops threads and capture
            let _ = std::fs::remove_dir_all(&dir);
            Ok(())
        })
        .await?;
        self.emit(EngineEvent::Recorder {
            meeting: id,
            state: RecorderState::Cancelled,
            elapsed_ms: 0,
        });
        Ok(())
    }

    /// Stops the recording, drains the transcription queue, writes the meeting and queues the
    /// follow-up jobs. Returns once the transcript is saved (seconds for a normal call); diarisation,
    /// indexing and the summary continue in the background — watch `EngineEvent::Status` or use
    /// [`Engine::wait_ready`].
    pub async fn stop_recording(self: &Arc<Self>) -> Result<MeetingId> {
        let session = self.take_session()?;
        let id = session.id;
        let finished = self.blocking(move |e| e.finish_session(session)).await;
        let finished = match finished {
            Ok(f) => f,
            Err(e) => {
                self.notice(
                    NoticeLevel::Error,
                    &format!("Не удалось корректно остановить запись: {e}. Аудио сохранено, его можно восстановить."),
                    &format!("Stopping the recording failed: {e}. The audio is saved and can be recovered."),
                );
                return Err(e);
            }
        };
        self.finalize_dir(
            &finished.dir,
            crate::finalize::FinalizeOpts {
                language: finished.language,
                fill_tail: false,
            },
        )
        .await?;
        Ok(id)
    }

    fn active_guard(&self) -> Result<std::sync::MutexGuard<'_, Option<Session>>> {
        self.active
            .lock()
            .map_err(|_| Error::Storage("session lock poisoned".into()))
    }

    fn take_session(&self) -> Result<Session> {
        self.active_guard()?.take().ok_or_else(not_recording)
    }

    /// Stops capture and drains live transcription. Blocking.
    fn finish_session(&self, mut s: Session) -> Result<Finished> {
        s.monitor_stop.store(true, Ordering::Relaxed);
        if let Some(m) = s.monitor.take() {
            let _ = m.join();
        }
        let handle = s
            .handle
            .lock()
            .map_err(|_| Error::Storage("recorder lock poisoned".into()))?
            .take();
        let elapsed = handle
            .as_ref()
            .map(|h| h.elapsed().as_millis() as u64)
            .unwrap_or(0);
        if let Some(h) = handle {
            // Joins the capture threads; the frame channels close behind it.
            let tracks = h.stop()?;
            for track in tracks {
                let pauses = read_meta(&meta_path(&s.dir, track.kind))
                    .map(|meta| meta.pauses)
                    .unwrap_or_default();
                let track_ms =
                    track.offset_ms + file_to_track_time(samples_to_ms(track.samples), &pauses);
                let drift_ms = track_ms.abs_diff(elapsed);
                if drift_ms > 2_000 {
                    tracing::warn!(
                        meeting = %s.id,
                        track = ?track.kind,
                        samples = track.samples,
                        track_ms,
                        wall_clock_ms = elapsed,
                        drift_ms,
                        "audio track sample count diverges from wall clock"
                    );
                }
            }
        }
        self.emit(EngineEvent::Recorder {
            meeting: s.id,
            state: RecorderState::Stopped,
            elapsed_ms: elapsed,
        });
        for f in s.feeders.drain(..) {
            let _ = f.join();
        }
        s.queue.close();
        if let Some(worker) = s.worker.take() {
            while !worker.is_finished() {
                let (done, total) = s.queue.progress();
                let progress = if total == 0 {
                    0.0
                } else {
                    done as f32 / total as f32
                };
                self.emit(EngineEvent::Status {
                    meeting: s.id,
                    status: MeetingStatus::Processing {
                        stage: Stage::Transcribe,
                        progress: progress.clamp(0.0, 1.0),
                    },
                });
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = worker.join();
        }
        let model = s.model.lock().ok().and_then(|m| m.clone());
        if let Some(m) = &model {
            let _ = crate::meta::write_asr_model(&s.dir, &m.id);
        }
        Ok(Finished {
            dir: s.dir.clone(),
            language: s.language.clone(),
        })
    }
}

fn not_recording() -> Error {
    Error::Invalid("no recording in progress".into())
}

fn default_title(at: DateTime<Utc>, russian: bool) -> String {
    let local = at.with_timezone(&chrono::Local);
    if russian {
        format!("Встреча {}", local.format("%H:%M"))
    } else {
        format!("Meeting {}", local.format("%H:%M"))
    }
}

/// Picks the microphone: the configured one, else the system default. Swapping a Bluetooth
/// headset's microphone for the built-in one (a headset mic drops the headphones to call-quality
/// audio) is done by the recorder itself, driven by `RecorderConfig::prefer_builtin_mic`.
fn resolve_mic(settings: &crate::settings::Settings) -> DeviceSel {
    match &settings.mic_device {
        Some(d) => DeviceSel::Named(d.clone()),
        None => DeviceSel::Default,
    }
}

/// Per-track thread: frames → VAD → closed regions → ASR queue.
struct Feeder {
    kind: TrackKind,
    rx: Receiver<AudioFrame>,
    vad: Arc<dyn VadFactory>,
    queue: Arc<AsrQueue>,
    log: Arc<LiveLog>,
    clock: Arc<LiveClock>,
    abort: Arc<AtomicBool>,
    notify: Notifier,
    russian: bool,
}

/// Longest gap filled with silence to keep the VAD timeline aligned; longer gaps restart the VAD.
const MAX_FILL: u64 = 120 * 16_000;
/// Backlog (ms of speech) beyond which the user is told transcription lags.
const BACKLOG_WARN_MS: u64 = 30_000;

impl Feeder {
    fn run(self) -> Result<()> {
        let mut extractor = RegionExtractor::new(self.kind, self.vad.create()?, 0);
        let mut expected = 0u64;
        let mut warned = false;
        loop {
            if self.abort.load(Ordering::Relaxed) {
                return Ok(());
            }
            let frame = match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(f) => f,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let end = frame.start_sample + frame.samples.len() as u64;
            if end <= expected {
                continue;
            }
            let mut regions = Vec::new();
            if frame.start_sample > expected {
                let gap = frame.start_sample - expected;
                self.log.append(&LiveLine::Gap {
                    track: self.kind,
                    start_sample: expected,
                    end_sample: frame.start_sample,
                })?;
                if gap > MAX_FILL {
                    regions.extend(extractor.flush());
                    extractor =
                        RegionExtractor::new(self.kind, self.vad.create()?, frame.start_sample);
                } else {
                    regions.extend(extractor.feed_silence(gap as usize)?);
                }
            }
            let skip = expected.saturating_sub(frame.start_sample) as usize;
            regions.extend(extractor.feed(&frame.samples[skip..])?);
            expected = end;
            self.clock.seen[idx(self.kind)].store(end, Ordering::Relaxed);
            for region in regions {
                self.queue.push(region);
            }
            if !warned && self.queue.backlog_ms() > BACKLOG_WARN_MS {
                warned = true;
                (self.notify)(
                    NoticeLevel::Info,
                    if self.russian {
                        "Расшифровка отстаёт от звонка; ничего не потеряно, догоним после остановки.".to_owned()
                    } else {
                        "Transcription is falling behind the call; nothing is lost, it will catch up after you stop.".to_owned()
                    },
                );
            }
        }
        for region in extractor.flush() {
            self.queue.push(region);
        }
        Ok(())
    }
}

/// Levels at ~25 Hz, a once-a-second clock tick, and recorder events → engine events.
struct Monitor {
    id: MeetingId,
    events: broadcast::Sender<EngineEvent>,
    notify: Notifier,
    russian: bool,
    warned_drop: bool,
    /// Tracks the user was already told are silent (the recorder re-raises the warning).
    silent_noticed: [bool; 2],
}

impl Monitor {
    fn run(mut self, handle: &Mutex<Option<Box<dyn RecordingHandle>>>, stop: &AtomicBool) {
        let mut last_tick = Instant::now();
        let mut recorder_events: Option<Receiver<RecorderEvent>> = None;
        while !stop.load(Ordering::Relaxed) {
            {
                let Ok(guard) = handle.lock() else { return };
                let Some(h) = guard.as_ref() else { return };
                let rx = recorder_events.get_or_insert_with(|| h.events());
                let level = |k| h.level(k).unwrap_or(Level::default());
                let _ = self.events.send(EngineEvent::Levels {
                    mic: level(TrackKind::Mic),
                    system: level(TrackKind::System),
                });
                if last_tick.elapsed() >= Duration::from_secs(1) {
                    last_tick = Instant::now();
                    let _ = self.events.send(EngineEvent::Recorder {
                        meeting: self.id,
                        state: RecorderState::Recording,
                        elapsed_ms: h.elapsed().as_millis() as u64,
                    });
                }
                while let Ok(ev) = rx.try_recv() {
                    self.forward(ev);
                }
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    fn forward(&mut self, ev: RecorderEvent) {
        let ru = self.russian;
        let who = |t: TrackKind| match (t, ru) {
            (TrackKind::Mic, true) => "микрофон",
            (TrackKind::Mic, false) => "microphone",
            (TrackKind::System, true) => "звук собеседника",
            (TrackKind::System, false) => "system audio",
        };
        match ev {
            RecorderEvent::DeviceChanged { track, device } => {
                (self.notify)(
                    NoticeLevel::Info,
                    if ru {
                        format!(
                            "Устройство ({}) сменилось: {device}. Запись продолжается.",
                            who(track)
                        )
                    } else {
                        format!(
                            "Input changed ({}): {device}. Recording continues.",
                            who(track)
                        )
                    },
                );
                let _ = self
                    .events
                    .send(EngineEvent::DeviceChanged { track, device });
            }
            RecorderEvent::TrackSilent {
                track,
                silent_for_ms,
            } => {
                let _ = self.events.send(EngineEvent::TrackSilent {
                    meeting: self.id,
                    track,
                    silent_for_ms,
                });
                if !std::mem::replace(&mut self.silent_noticed[idx(track)], true) {
                    (self.notify)(
                        NoticeLevel::Warning,
                        if ru {
                            format!("Больше минуты тишины: {}. Всё в порядке?", who(track))
                        } else {
                            format!(
                                "More than a minute of silence from {}. Is everything alright?",
                                who(track)
                            )
                        },
                    );
                }
            }
            RecorderEvent::TrackActive { track } => {
                self.silent_noticed[idx(track)] = false;
            }
            RecorderEvent::PermissionSuspected { track, .. } => {
                (self.notify)(
                    NoticeLevel::Warning,
                    if ru {
                        format!(
                            "Нет сигнала: {}. Проверьте разрешения YadaCall в системных настройках.",
                            who(track)
                        )
                    } else {
                        format!(
                            "No signal from {}. Check YadaCall's permissions in system settings.",
                            who(track)
                        )
                    },
                );
            }
            RecorderEvent::Error { track, message } => {
                (self.notify)(
                    NoticeLevel::Error,
                    if ru {
                        format!("Ошибка записи ({}): {message}", who(track))
                    } else {
                        format!("Recording error ({}): {message}", who(track))
                    },
                );
            }
            RecorderEvent::MicSubstituted { bluetooth, using } => {
                (self.notify)(
                    NoticeLevel::Info,
                    if ru {
                        format!(
                            "{bluetooth} остаются в хорошем качестве: пишем встроенный микрофон ({using})."
                        )
                    } else {
                        format!(
                            "{bluetooth} stays in high quality: recording the built-in microphone ({using})."
                        )
                    },
                );
            }
            RecorderEvent::RateChanged {
                track,
                from_hz,
                to_hz,
            } => {
                tracing::warn!(?track, from_hz, to_hz, "capture sample rate changed");
            }
            RecorderEvent::DriftCorrected { track, drift_ms } => {
                tracing::warn!(?track, drift_ms, "track drift corrected");
            }
            RecorderEvent::Overrun { .. } | RecorderEvent::FramesDropped { .. } => {
                if !self.warned_drop {
                    self.warned_drop = true;
                    (self.notify)(
                        NoticeLevel::Warning,
                        if ru {
                            "Часть звука не успевает обрабатываться на лету; запись на диск не затронута, текст дозаполнится после остановки.".to_owned()
                        } else {
                            "Some audio could not be processed live; the recording on disk is intact and the text will be completed after you stop.".to_owned()
                        },
                    );
                }
            }
            RecorderEvent::TrackStarted { .. } | RecorderEvent::Paused | RecorderEvent::Resumed => {
            }
        }
    }
}
