//! Deterministic fakes for every port, so the whole engine runs in tests (and in the CLI's
//! self-checks) without devices or models.
//!
//! The fake "speech" is a square wave whose amplitude is the *id* of the phrase: [`EnergyVad`]
//! finds it by energy, [`FakeAsr`] maps the amplitude to canned text and [`FakeSpeakerEmbedder`] to
//! a voice, so tests control exactly what each region means without caring about processing order.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use crossbeam_channel::{Receiver, Sender};
use tokio::sync::mpsc;
use yc_audio::writer::{TrackMeta, TrackWriter};
use yc_audio::{AudioFrame, RecorderConfig, RecorderEvent};
use yc_core::events::Level;
use yc_core::meeting::{TrackInfo, TrackKind};
use yc_core::ports::{
    EmbedKind, ModelInfo, SpeakerEmbedder, SpeechRecognizer, TextEmbedder, VoiceActivityDetector,
};
use yc_core::summary::{Decision, Summary, Task};
use yc_core::transcript::Recognized;
use yc_core::{Error, Result, SAMPLE_RATE};

use crate::ports::*;

/// `ms` of fake speech with the given level (square wave, so mean |x| == rms == `level`).
pub fn speech(level: f32, ms: u64) -> Vec<f32> {
    let n = (ms * u64::from(SAMPLE_RATE) / 1000) as usize;
    (0..n)
        .map(|i| if (i / 8) % 2 == 0 { level } else { -level })
        .collect()
}

/// `ms` of digital silence.
pub fn silence(ms: u64) -> Vec<f32> {
    vec![0.0; (ms * u64::from(SAMPLE_RATE) / 1000) as usize]
}

/// Energy-based streaming VAD over 32 ms windows.
pub struct EnergyVad {
    pub threshold: f32,
    pub min_silence_samples: usize,
    pub min_speech_samples: usize,
    pub max_region_samples: usize,
    pending: Vec<f32>,
    pos: u64,
    open: Option<u64>,
    last_speech_end: u64,
    silence_run: usize,
}

const WINDOW: usize = 512;

impl Default for EnergyVad {
    fn default() -> Self {
        Self {
            threshold: 0.02,
            min_silence_samples: 6_400, // 400 ms
            min_speech_samples: 3_200,  // 200 ms
            max_region_samples: 20 * SAMPLE_RATE as usize,
            pending: Vec::new(),
            pos: 0,
            open: None,
            last_speech_end: 0,
            silence_run: 0,
        }
    }
}

impl EnergyVad {
    fn window(&mut self, win: &[f32], out: &mut Vec<yc_core::ports::SpeechRegion>) {
        let rms = (win.iter().map(|s| s * s).sum::<f32>() / win.len().max(1) as f32).sqrt();
        let end = self.pos + win.len() as u64;
        if rms > self.threshold {
            self.open.get_or_insert(self.pos);
            self.last_speech_end = end;
            self.silence_run = 0;
        } else if self.open.is_some() {
            self.silence_run += win.len();
        }
        if let Some(start) = self.open {
            let too_long = (end - start) as usize >= self.max_region_samples;
            if self.silence_run >= self.min_silence_samples || too_long {
                self.close(out);
            }
        }
        self.pos = end;
    }

    fn close(&mut self, out: &mut Vec<yc_core::ports::SpeechRegion>) {
        if let Some(start) = self.open.take() {
            let end = self.last_speech_end;
            if (end - start) as usize >= self.min_speech_samples {
                out.push(yc_core::ports::SpeechRegion { start, end });
            }
        }
        self.silence_run = 0;
    }
}

impl VoiceActivityDetector for EnergyVad {
    fn push(&mut self, pcm: &[f32]) -> Result<Vec<yc_core::ports::SpeechRegion>> {
        self.pending.extend_from_slice(pcm);
        let mut out = Vec::new();
        let mut used = 0;
        while self.pending.len() - used >= WINDOW {
            let win: Vec<f32> = self.pending[used..used + WINDOW].to_vec();
            self.window(&win, &mut out);
            used += WINDOW;
        }
        self.pending.drain(..used);
        Ok(out)
    }

    fn flush(&mut self) -> Vec<yc_core::ports::SpeechRegion> {
        let mut out = Vec::new();
        if !self.pending.is_empty() {
            let win = std::mem::take(&mut self.pending);
            self.window(&win, &mut out);
        }
        self.close(&mut out);
        out
    }

    fn reset(&mut self) {
        *self = Self {
            threshold: self.threshold,
            min_silence_samples: self.min_silence_samples,
            min_speech_samples: self.min_speech_samples,
            max_region_samples: self.max_region_samples,
            ..Self::default()
        };
    }
}

/// [`VadFactory`] producing [`EnergyVad`]s.
#[derive(Default)]
pub struct FakeVadFactory;

impl VadFactory for FakeVadFactory {
    fn create(&self) -> Result<Box<dyn VoiceActivityDetector>> {
        Ok(Box::new(EnergyVad::default()))
    }
}

fn mean_abs(pcm: &[f32]) -> f32 {
    pcm.iter().map(|s| s.abs()).sum::<f32>() / pcm.len().max(1) as f32
}

/// Recogniser that returns the canned text whose level is closest to the region's mean amplitude.
pub struct FakeAsr {
    info: ModelInfo,
    table: Vec<(f32, String)>,
    calls: Arc<AtomicUsize>,
}

impl SpeechRecognizer for FakeAsr {
    fn info(&self) -> &ModelInfo {
        &self.info
    }
    fn max_chunk_ms(&self) -> u64 {
        25_000
    }
    fn recognize(&mut self, pcm: &[f32], _language: &str) -> Result<Vec<Recognized>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let level = mean_abs(pcm);
        let text = self
            .table
            .iter()
            .min_by(|a, b| (a.0 - level).abs().total_cmp(&(b.0 - level).abs()))
            .map(|(_, t)| t.clone())
            .unwrap_or_default();
        Ok(vec![Recognized {
            start_ms: 0,
            end_ms: pcm.len() as u64 * 1000 / u64::from(SAMPLE_RATE),
            text,
            words: vec![],
            confidence: Some(0.9),
        }])
    }
}

/// [`AsrFactory`] for [`FakeAsr`].
pub struct FakeAsrFactory {
    table: Vec<(f32, String)>,
    /// Number of `recognize` calls across all loaded instances.
    pub calls: Arc<AtomicUsize>,
    /// When set, `load` fails with `ModelMissing`.
    pub missing: AtomicBool,
}

impl FakeAsrFactory {
    pub fn new(table: &[(f32, &str)]) -> Self {
        Self {
            table: table.iter().map(|(l, t)| (*l, (*t).to_owned())).collect(),
            calls: Arc::default(),
            missing: AtomicBool::new(false),
        }
    }
}

impl AsrFactory for FakeAsrFactory {
    fn load(&self, _language: &str) -> Result<Box<dyn SpeechRecognizer>> {
        if self.missing.load(Ordering::Relaxed) {
            return Err(Error::ModelMissing("fake-asr".into()));
        }
        Ok(Box::new(FakeAsr {
            info: ModelInfo {
                id: "fake-asr".into(),
                backend: "cpu".into(),
            },
            table: self.table.clone(),
            calls: Arc::clone(&self.calls),
        }))
    }
}

/// Speaker embedder: a region's mean amplitude picks an angle on the unit circle in the first two
/// dimensions, so equal levels are the same voice and distant levels are different voices.
pub struct FakeSpeakerEmbedder {
    info: ModelInfo,
}

/// Embedding the fake produces for a speech level (also usable to seed known voices in tests).
pub fn fake_voice(level: f32) -> Vec<f32> {
    let angle = level * 8.0;
    vec![angle.cos(), angle.sin(), 0.0, 0.0]
}

impl SpeakerEmbedder for FakeSpeakerEmbedder {
    fn info(&self) -> &ModelInfo {
        &self.info
    }
    fn dims(&self) -> usize {
        4
    }
    fn embed(&mut self, pcm: &[f32]) -> Result<Vec<f32>> {
        Ok(fake_voice(mean_abs(pcm)))
    }
}

/// [`SpeakerEmbedderFactory`] for [`FakeSpeakerEmbedder`].
#[derive(Default)]
pub struct FakeSpeakerFactory;

impl SpeakerEmbedderFactory for FakeSpeakerFactory {
    fn create(&self) -> Result<Option<Box<dyn SpeakerEmbedder>>> {
        Ok(Some(Box::new(FakeSpeakerEmbedder {
            info: ModelInfo {
                id: "fake-spk".into(),
                backend: "cpu".into(),
            },
        })))
    }
}

/// Text embedder hashing character bigrams into 32 dimensions (similar texts → close vectors).
pub struct FakeTextEmbedder {
    info: ModelInfo,
}

impl TextEmbedder for FakeTextEmbedder {
    fn info(&self) -> &ModelInfo {
        &self.info
    }
    fn dims(&self) -> usize {
        32
    }
    fn embed(&mut self, texts: &[&str], _kind: EmbedKind) -> Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|t| {
                let chars: Vec<char> = t.to_lowercase().chars().collect();
                let mut v = [0.0f32; 32];
                for w in chars.windows(2) {
                    let h = (w[0] as usize * 31 + w[1] as usize) % 32;
                    v[h] += 1.0;
                }
                let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
                v.iter().map(|x| x / n).collect::<Vec<f32>>()
            })
            .collect())
    }
}

/// [`TextEmbedderFactory`] for [`FakeTextEmbedder`].
#[derive(Default)]
pub struct FakeTextEmbedderFactory;

impl TextEmbedderFactory for FakeTextEmbedderFactory {
    fn create(&self) -> Result<Option<Box<dyn TextEmbedder>>> {
        Ok(Some(Box::new(FakeTextEmbedder {
            info: ModelInfo {
                id: "fake-text".into(),
                backend: "cpu".into(),
            },
        })))
    }
}

/// Summariser: lists the first turn as "about", every turn containing "надо" or "should" as a task.
/// Fails with a retryable `Error::Llm` for the first `fail_first` calls.
pub struct FakeSummarizer {
    pub fail_first: AtomicU32,
    pub calls: AtomicU32,
    pub error: Mutex<Option<fn() -> Error>>,
    pub inputs: Mutex<Vec<SummaryInput>>,
    pub title: Option<String>,
}

impl FakeSummarizer {
    pub fn new() -> Self {
        Self {
            fail_first: AtomicU32::new(0),
            calls: AtomicU32::new(0),
            error: Mutex::new(None),
            inputs: Mutex::new(Vec::new()),
            title: Some("Итоги синка".into()),
        }
    }
}

impl Default for FakeSummarizer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Summarizer for FakeSummarizer {
    async fn summarize(
        &self,
        input: SummaryInput,
        progress: &(dyn Fn(f32) + Send + Sync),
    ) -> Result<Summary> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_first.load(Ordering::SeqCst) > 0 {
            self.fail_first.fetch_sub(1, Ordering::SeqCst);
            let make = *self
                .error
                .lock()
                .map_err(|_| Error::Llm("poisoned".into()))?;
            return Err(make.map_or_else(|| Error::Llm("provider unavailable".into()), |f| f()));
        }
        progress(0.5);
        let tasks = input
            .turns
            .iter()
            .filter(|t| t.text.contains("надо") || t.text.contains("should"))
            .map(|t| Task {
                text: t.text.clone(),
                owner: Some(t.speaker.clone()),
                due: None,
                at_ms: Some(t.start_ms),
                done: false,
            })
            .collect();
        let summary = Summary {
            title: self.title.clone(),
            about: vec![
                input
                    .turns
                    .first()
                    .map(|t| t.text.clone())
                    .unwrap_or_default(),
            ],
            decisions: vec![Decision {
                text: "decided".into(),
                at_ms: None,
            }],
            tasks,
            open_questions: vec![],
            language: input.language.clone(),
            model: "fake:summarizer".into(),
        };
        self.inputs
            .lock()
            .map_err(|_| Error::Llm("poisoned".into()))?
            .push(input);
        progress(1.0);
        Ok(summary)
    }
}

/// Answerer that echoes the question with the number of excerpts, citing all of them.
#[derive(Default)]
pub struct FakeAnswerer;

#[async_trait]
impl Answerer for FakeAnswerer {
    async fn answer(&self, request: AnswerRequest, out: mpsc::Sender<AnswerEvent>) -> Result<()> {
        let n = request.excerpts.len();
        let _ = out
            .send(AnswerEvent::Egress(format!("{n} excerpts sent, no audio")))
            .await;
        for word in ["Answer", "to", &request.question] {
            if out.send(AnswerEvent::Token(word.to_owned())).await.is_err() {
                return Ok(());
            }
        }
        let _ = out.send(AnswerEvent::Citations((0..n).collect())).await;
        let _ = out.send(AnswerEvent::Done).await;
        Ok(())
    }
}

/// What a synthetic track plays.
#[derive(Clone)]
pub struct TrackScript {
    pub kind: TrackKind,
    /// Offset of the track's first sample from the meeting start.
    pub offset_ms: u64,
    pub audio: Vec<f32>,
}

/// A [`RecorderBackend`] that "records" scripted audio: writes the track files exactly like the
/// real recorder (`yc_audio::writer`) and streams frames to the engine as fast as it consumes them.
pub struct SyntheticRecorder {
    scripts: Vec<TrackScript>,
    events: Arc<Mutex<Option<Sender<RecorderEvent>>>>,
    /// Sleep between 512-sample frames (default none).
    pub pace: Option<Duration>,
    /// Frames to skip silently after this many per track (simulates an overflowing live channel).
    pub drop_frames_after: Option<u64>,
}

impl SyntheticRecorder {
    pub fn new(scripts: Vec<TrackScript>) -> Self {
        Self {
            scripts,
            events: Arc::default(),
            pace: None,
            drop_frames_after: None,
        }
    }

    /// Injects a recorder event into the running session.
    pub fn emit(&self, event: RecorderEvent) {
        if let Ok(guard) = self.events.lock()
            && let Some(tx) = guard.as_ref()
        {
            let _ = tx.send(event);
        }
    }
}

struct SyntheticTrack {
    frames: Receiver<AudioFrame>,
    level: Arc<AtomicU32>,
    thread: Option<std::thread::JoinHandle<Option<TrackInfo>>>,
}

struct SyntheticHandle {
    tracks: Vec<(TrackKind, SyntheticTrack)>,
    stop: Arc<AtomicBool>,
    abort: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    events_rx: Receiver<RecorderEvent>,
    started: Instant,
    wall: SystemTime,
}

impl RecorderBackend for SyntheticRecorder {
    fn start(&self, config: RecorderConfig) -> Result<Box<dyn RecordingHandle>> {
        std::fs::create_dir_all(&config.dir)?;
        let (events_tx, events_rx) = crossbeam_channel::unbounded();
        if let Ok(mut g) = self.events.lock() {
            *g = Some(events_tx);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let abort = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let started = Instant::now();
        let wall = SystemTime::now();
        let mut tracks = Vec::new();
        for script in &self.scripts {
            if (script.kind == TrackKind::Mic && config.mic.is_none())
                || (script.kind == TrackKind::System && !config.system)
            {
                continue;
            }
            let (tx, frames) = crossbeam_channel::bounded(config.frame_queue.max(1));
            let level = Arc::new(AtomicU32::new(0));
            let meta = TrackMeta::new(
                script.kind,
                "synthetic",
                wall.duration_since(SystemTime::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64),
                script.offset_ms,
            );
            let mut writer = TrackWriter::create(&config.dir, meta, started)?;
            let script = script.clone();
            let stop = Arc::clone(&stop);
            let abort = Arc::clone(&abort);
            let paused = Arc::clone(&paused);
            let level_c = Arc::clone(&level);
            let pace = self.pace;
            let drop_after = self.drop_frames_after;
            let thread = std::thread::Builder::new()
                .name(format!("synthetic-{}", script.kind.file_stem()))
                .spawn(move || {
                    let mut was_paused = false;
                    for (n, block) in script.audio.chunks(config.frame_len.max(1)).enumerate() {
                        let sent = n as u64 + 1;
                        if abort.load(Ordering::Relaxed) {
                            return None;
                        }
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        let p = paused.load(Ordering::Relaxed);
                        if p != was_paused {
                            if p {
                                writer.begin_pause(Instant::now());
                            } else {
                                let _ = writer.end_pause(Instant::now());
                            }
                            was_paused = p;
                        }
                        let start_sample = writer.samples();
                        if writer.write(block).is_err() {
                            return None;
                        }
                        let rms = (block.iter().map(|s| s * s).sum::<f32>()
                            / block.len().max(1) as f32)
                            .sqrt();
                        level_c.store(rms.to_bits(), Ordering::Relaxed);
                        let skip = drop_after.is_some_and(|n| sent > n && sent <= n + 40);
                        if !p && !skip {
                            let frame = AudioFrame {
                                track: script.kind,
                                start_sample,
                                samples: block.to_vec(),
                            };
                            if tx.send(frame).is_err() {
                                return None;
                            }
                        }
                        if let Some(d) = pace {
                            std::thread::sleep(d);
                        }
                    }
                    // Audio exhausted: keep the "device" open until stopped, like a quiet room.
                    while !stop.load(Ordering::Relaxed) {
                        if abort.load(Ordering::Relaxed) {
                            return None;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    if abort.load(Ordering::Relaxed) {
                        return None;
                    }
                    writer.finish(Instant::now()).ok()
                })
                .map_err(Error::Io)?;
            tracks.push((
                script.kind,
                SyntheticTrack {
                    frames,
                    level,
                    thread: Some(thread),
                },
            ));
        }
        Ok(Box::new(SyntheticHandle {
            tracks,
            stop,
            abort,
            paused,
            events_rx,
            started,
            wall,
        }))
    }
}

impl RecordingHandle for SyntheticHandle {
    fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
    }
    fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }
    fn level(&self, kind: TrackKind) -> Option<Level> {
        self.tracks.iter().find(|(k, _)| *k == kind).map(|(_, t)| {
            let rms = f32::from_bits(t.level.load(Ordering::Relaxed));
            Level { rms, peak: rms }
        })
    }
    fn frames(&self, kind: TrackKind) -> Option<Receiver<AudioFrame>> {
        self.tracks
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, t)| t.frames.clone())
    }
    fn events(&self) -> Receiver<RecorderEvent> {
        self.events_rx.clone()
    }
    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
    fn started_at(&self) -> SystemTime {
        self.wall
    }
    fn stop(mut self: Box<Self>) -> Result<Vec<TrackInfo>> {
        self.stop.store(true, Ordering::Relaxed);
        let mut infos = Vec::new();
        for (_, t) in &mut self.tracks {
            if let Some(h) = t.thread.take()
                && let Ok(Some(info)) = h.join()
            {
                infos.push(info);
            }
        }
        Ok(infos)
    }
}

impl Drop for SyntheticHandle {
    fn drop(&mut self) {
        // Dropped without `stop`: behave like a crash — the writers are dropped without `finish`.
        self.abort.store(true, Ordering::Relaxed);
        for (_, t) in &mut self.tracks {
            if let Some(h) = t.thread.take() {
                let _ = h.join();
            }
        }
    }
}
