//! Shared harness: an engine on temp dirs wired to the synthetic recorder and the fakes.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;
use tokio::sync::broadcast::Receiver;
use yc_core::events::EngineEvent;
use yc_core::meeting::{MeetingId, MeetingStatus};
use yc_engine::ports::Ports;
use yc_engine::testing::*;
use yc_engine::{Engine, EngineConfig, JobConfig};

pub const A: f32 = 0.10;
pub const B: f32 = 0.20;
pub const C: f32 = 0.30;

pub const TEXT_A: &str = "добрый день начнём синк";
pub const TEXT_B: &str = "да договорились по срокам";
pub const TEXT_C: &str = "спасибо надо ещё созвониться";

pub struct Env {
    pub dir: TempDir,
    pub asr: Arc<FakeAsrFactory>,
    pub summarizer: Arc<FakeSummarizer>,
    pub recorder: Arc<SyntheticRecorder>,
}

impl Env {
    pub fn new(scripts: Vec<TrackScript>) -> Self {
        Self::with_recorder(SyntheticRecorder::new(scripts))
    }

    pub fn with_recorder(recorder: SyntheticRecorder) -> Self {
        Self {
            dir: tempfile::tempdir().expect("tempdir"),
            asr: Arc::new(FakeAsrFactory::new(&[
                (A, TEXT_A),
                (B, TEXT_B),
                (C, TEXT_C),
            ])),
            summarizer: Arc::new(FakeSummarizer::new()),
            recorder: Arc::new(recorder),
        }
    }

    pub fn app_data(&self) -> PathBuf {
        self.dir.path().join("app")
    }

    pub fn recording_dir(&self, id: MeetingId) -> PathBuf {
        self.app_data().join("recording").join(id.to_string())
    }

    pub fn library(&self) -> PathBuf {
        self.dir.path().join("library")
    }

    pub fn ports(&self) -> Ports {
        Ports {
            asr: self.asr.clone(),
            vad: Arc::new(FakeVadFactory),
            speaker: Arc::new(FakeSpeakerFactory),
            text_embedder: Arc::new(FakeTextEmbedderFactory),
            summarizer: Some(self.summarizer.clone()),
            answerer: Some(Arc::new(FakeAnswerer)),
            recorder: self.recorder.clone(),
        }
    }

    pub async fn open(&self) -> Arc<Engine> {
        self.open_with(self.ports()).await
    }

    pub async fn open_with(&self, ports: Ports) -> Arc<Engine> {
        let mut cfg = EngineConfig::new(self.app_data(), self.library(), ports);
        cfg.prevent_sleep = false;
        cfg.skip_disk_check = true;
        cfg.jobs = JobConfig {
            poll: Duration::from_millis(40),
            max_attempts: 3,
            backoff_base: Duration::from_millis(10),
        };
        let engine = Engine::open(cfg).await.expect("open engine");
        engine
            .update_settings(|s| s.prefer_builtin_mic = false)
            .await
            .expect("settings");
        engine
    }
}

/// A two-sided call: the other side says A then C (3.5 s each); the mic hears an echo of A and says B.
pub fn call_scripts() -> Vec<TrackScript> {
    let mut sys = silence(500);
    sys.extend(speech(A, 3500));
    sys.extend(silence(4500));
    sys.extend(speech(C, 3500));
    sys.extend(silence(800));
    let mut mic = silence(600);
    mic.extend(speech(A, 3500)); // acoustic echo of the other side
    mic.extend(silence(1500));
    mic.extend(speech(B, 2000));
    mic.extend(silence(5200));
    vec![
        TrackScript {
            kind: yc_core::meeting::TrackKind::System,
            offset_ms: 0,
            audio: sys,
        },
        TrackScript {
            kind: yc_core::meeting::TrackKind::Mic,
            offset_ms: 0,
            audio: mic,
        },
    ]
}

/// Waits for an event matching `pred`.
pub async fn wait_event(
    rx: &mut Receiver<EngineEvent>,
    secs: u64,
    mut pred: impl FnMut(&EngineEvent) -> bool,
) -> EngineEvent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Ok(ev)) if pred(&ev) => return ev,
            Ok(Ok(_)) | Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
            Ok(Err(e)) => panic!("event channel closed: {e}"),
            Err(_) => panic!("timed out waiting for an event"),
        }
    }
}

/// Polls `f` until it returns true.
pub async fn eventually(secs: u64, what: &str, mut f: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while !f() {
        assert!(std::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub async fn ready(engine: &Arc<Engine>, id: MeetingId) -> MeetingStatus {
    engine
        .wait_ready(id, Duration::from_secs(20))
        .await
        .expect("meeting settles")
}

/// Records the standard call to completion and returns its id.
pub async fn record_call(env: &Env, engine: &Arc<Engine>, title: &str) -> MeetingId {
    start_call(env, engine, title).await;
    engine.stop_recording().await.expect("stop")
}

pub async fn record_call_with_hints(
    env: &Env,
    engine: &Arc<Engine>,
    title: &str,
    participant_hints: Vec<yc_core::meeting::ParticipantHint>,
) -> MeetingId {
    start_call_with_hints(env, engine, title, participant_hints).await;
    engine.stop_recording().await.expect("stop")
}

/// Starts the standard call and waits until all scripted audio reached the disk.
pub async fn start_call(env: &Env, engine: &Arc<Engine>, title: &str) -> MeetingId {
    start_call_with_hints(env, engine, title, Vec::new()).await
}

pub async fn start_call_with_hints(
    env: &Env,
    engine: &Arc<Engine>,
    title: &str,
    participant_hints: Vec<yc_core::meeting::ParticipantHint>,
) -> MeetingId {
    let id = engine
        .start_recording(yc_engine::StartOptions {
            title: Some(title.to_owned()),
            participant_hints,
            ..Default::default()
        })
        .await
        .expect("start");
    let dir = env.recording_dir(id);
    // 12.8 s of 16 kHz s16 audio per track.
    eventually(10, "audio written", || {
        std::fs::metadata(dir.join("them.pcm")).is_ok_and(|m| m.len() >= 12_800 * 32)
            && std::fs::metadata(dir.join("mic.pcm")).is_ok_and(|m| m.len() >= 12_800 * 32)
    })
    .await;
    id
}
