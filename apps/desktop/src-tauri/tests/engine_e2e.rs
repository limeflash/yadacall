//! End-to-end test of the real backend: real adapters on a temp library, a generated Russian clip,
//! import -> transcript -> search -> summary (local Gemma) -> ask.
//!
//! Ignored unless `YC_MODELS_DIR` is set (the developer model cache, usually
//! `~/.cache/yadacall-dev/models`). It needs macOS TTS (`say`, `afconvert`). Missing small models
//! (E5 embeddings, CAM++) are fetched into the cache through the same download path the app uses.
//!
//! Run: `YC_MODELS_DIR=~/.cache/yadacall-dev/models cargo test -p yadacall-desktop --test engine_e2e -- --ignored --nocapture`

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use yadacall_desktop_lib::api::*;
use yadacall_desktop_lib::engine_backend::{BackendConfig, EngineBackend};
use yc_core::events::{DownloadState, EngineEvent};
use yc_core::meeting::MeetingStatus;

const SPEECH: &str = "Здравствуйте, коллеги. Сегодня мы обсудили запуск нового проекта. \
Мы решили выпустить первую версию в пятницу. \
Иван подготовит релизные заметки до среды. \
Мария проверит автоматические тесты завтра утром. \
Остался открытым вопрос про бюджет на рекламу.";

fn models_dir() -> Option<PathBuf> {
    std::env::var_os("YC_MODELS_DIR").map(PathBuf::from)
}

fn run(cmd: &str, args: &[&str]) {
    let status = Command::new(cmd).args(args).status().expect("spawn");
    assert!(status.success(), "{cmd} failed");
}

fn link(from: &Path, to: &Path) {
    if let Some(p) = to.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    if !to.exists() {
        std::os::unix::fs::symlink(from, to).unwrap();
    }
}

/// Lays out the temp app data dir like the app would see it, pointing at the shared model cache.
fn prepare_data_dir(data: &Path, cache: &Path) {
    link(cache, &data.join("models"));
    // The llama.cpp runtime from the cache stands in for the downloaded runtime pack.
    let runtimes = cache.join("ggml-org__llama.cpp");
    if let Some(dir) = std::fs::read_dir(&runtimes)
        .ok()
        .and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.is_dir()))
        .and_then(|d| std::fs::read_dir(d).ok())
        .and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.is_dir()))
    {
        link(&dir, &data.join("runtimes/llama-b11569-macos-arm64-metal"));
    }
    // CAM++ is stored under its catalog id.
    let camp = "3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx";
    let src = cache.join("k2-fsa__sherpa-onnx__speaker-recongition-models").join(camp);
    let dst = cache.join("speaker-campplus-zh-en-16k").join(camp);
    if src.is_file() && !dst.is_file() {
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::copy(&src, &dst).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs YC_MODELS_DIR and macOS TTS"]
async fn import_transcribe_search_summarise_ask() {
    let Some(cache) = models_dir() else {
        eprintln!("YC_MODELS_DIR is not set; skipping");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let library = tmp.path().join("library");
    prepare_data_dir(&data, &cache);

    // Speech fixture.
    let aiff = tmp.path().join("clip.aiff");
    let wav = tmp.path().join("совещание.wav");
    run("say", &["-v", "Milena", SPEECH, "-o", aiff.to_str().unwrap()]);
    run(
        "afconvert",
        &["-f", "WAVE", "-d", "LEI16@16000", "-c", "1", aiff.to_str().unwrap(), wav.to_str().unwrap()],
    );

    let events: Arc<Mutex<Vec<EngineEvent>>> = Arc::default();
    let timeline: Arc<Mutex<Vec<(Duration, String)>>> = Arc::default();
    let clock = Instant::now();
    let sink: EventSink = {
        let events = Arc::clone(&events);
        let timeline = Arc::clone(&timeline);
        Arc::new(move |e| {
            if let EngineEvent::Status { status, .. } = &e {
                let label = match status {
                    MeetingStatus::Processing { stage, .. } => format!("{stage:?}"),
                    other => format!("{other:?}"),
                };
                let mut t = timeline.lock().unwrap();
                if t.last().is_none_or(|(_, l)| *l != label) {
                    t.push((clock.elapsed(), label));
                }
            }
            events.lock().unwrap().push(e);
        })
    };
    let mut cfg = BackendConfig::new(&data, sink);
    cfg.library_dir = Some(library.clone());
    cfg.prevent_sleep = false;
    cfg.skip_disk_check = true;
    let t0 = Instant::now();
    let backend = EngineBackend::open(cfg).await.expect("open backend");
    eprintln!("open: {:?}", t0.elapsed());

    // First-run settings.
    let s = backend.get_settings().await.unwrap();
    assert!(!s.onboarded);
    assert_eq!(s.summaries.provider, SummaryProvider::Local);
    backend
        .update_settings(SettingsPatch {
            meeting_language: Some("ru".into()),
            ui_language: Some(UiLanguage::Ru),
            ..Default::default()
        })
        .await
        .unwrap();

    // Models: the cache provides GigaAM/Parakeet/Gemma; E5 comes through the app's download path.
    let overview = backend.models_overview().await.unwrap();
    let find = |id: &str| overview.models.iter().find(|m| m.id == id).unwrap_or_else(|| panic!("{id}"));
    assert!(find("gigaam-v3-e2e-rnnt-int8").installed && find("gigaam-v3-e2e-rnnt-int8").active);
    assert!(find("gemma-4-e2b-q4-0").installed);
    eprintln!(
        "hardware: {} | {} models listed | gigaam: {:?} \"{}\"",
        overview.hardware.label,
        overview.models.len(),
        find("gigaam-v3-e2e-rnnt-int8").verdict,
        find("gigaam-v3-e2e-rnnt-int8").estimate
    );
    let e5 = "multilingual-e5-small-int8";
    if !find(e5).installed {
        let t = Instant::now();
        let seen: Arc<Mutex<Vec<EngineEvent>>> = Arc::default();
        let s2 = Arc::clone(&seen);
        backend
            .model_download(e5, Arc::new(move |e| s2.lock().unwrap().push(e)))
            .await
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(600);
        loop {
            let done = seen.lock().unwrap().iter().any(|e| {
                matches!(e, EngineEvent::Download { state: DownloadState::Done, .. })
            });
            if done {
                break;
            }
            assert!(Instant::now() < deadline, "E5 download timed out");
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let progress = seen.lock().unwrap().len();
        eprintln!("E5 download: {:?} ({progress} progress events)", t.elapsed());
    }
    assert!(backend.models_overview().await.unwrap().models.iter().any(|m| m.id == e5 && m.installed));
    backend.model_set_active(e5).await.unwrap();

    // Import -> ready.
    let import_started = clock.elapsed();
    let t = Instant::now();
    let jobs = backend
        .import_audio(vec![wav.to_string_lossy().into_owned()])
        .await
        .unwrap();
    let meeting_id = jobs[0].meeting_id.clone().expect("meeting id");
    let id = meeting_id.parse().unwrap();
    let status = backend
        .engine()
        .wait_ready(id, Duration::from_secs(900))
        .await
        .unwrap();
    let ready_in = t.elapsed();
    eprintln!("import -> ready: {ready_in:?}, status {status:?}");
    assert_eq!(status, MeetingStatus::Ready, "meeting failed");

    for (at, label) in timeline.lock().unwrap().iter() {
        eprintln!("  +{:>6.1}s  {label}", (*at - import_started.min(*at)).as_secs_f64());
    }

    // Transcript text.
    let detail = backend.get_meeting(&meeting_id).await.unwrap();
    let text = detail
        .turns
        .iter()
        .map(|t| t.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    eprintln!("transcript: {text}");
    assert!(text.contains("релиз"), "transcript lacks 'релиз': {text}");
    assert!(text.contains("пятниц"), "transcript lacks 'пятниц': {text}");

    // Search (hybrid: FTS + semantic).
    let hits = backend.search("релизные заметки").await.unwrap();
    assert!(!hits.is_empty(), "no search hits");
    assert_eq!(hits[0].meeting_id, meeting_id);

    // Summary from local Gemma.
    let summary = detail.summary.expect("summary was not produced");
    eprintln!("summary: {summary:#?}");
    assert!(!summary.tasks.is_empty(), "summary has no tasks");
    let tasks = backend.list_tasks(true).await.unwrap();
    assert!(!tasks.is_empty());

    // Listing, tasks, export, copy-for-Claude.
    let list = backend.list_meetings().await.unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].snippet.is_some());
    let path = backend.export_meeting(&meeting_id, ExportFormat::Md).await.unwrap();
    assert!(Path::new(&path).is_file());
    assert!(!backend.copy_for_claude(&meeting_id).await.unwrap().is_empty());

    // Ask over the library with the local model.
    let tokens = Arc::new(Mutex::new(String::new()));
    let finished = Arc::new(Mutex::new(None::<SentNote>));
    let (tk, fin) = (Arc::clone(&tokens), Arc::clone(&finished));
    let t = Instant::now();
    backend
        .ask(
            "Кто подготовит релизные заметки?",
            Arc::new(move |e| match e {
                AskEvent::Token { text } => tk.lock().unwrap().push_str(&text),
                AskEvent::Done { sent } => *fin.lock().unwrap() = Some(sent),
                AskEvent::Error { message } => panic!("ask failed: {message}"),
                AskEvent::Citations { .. } => {}
            }),
        )
        .await
        .unwrap();
    eprintln!("ask: {:?} -> {}", t.elapsed(), tokens.lock().unwrap());
    let sent = finished.lock().unwrap().clone().expect("done event");
    assert_eq!(sent.provider, "local");

    // Real benchmarks feed the predictor's calibration.
    for id in ["gigaam-v3-e2e-rnnt-int8", "gemma-4-e2b-q4-0"] {
        let t = Instant::now();
        let r = backend.model_benchmark(id).await.unwrap();
        eprintln!("benchmark {id}: {:.1} in {:?}", r.realtime_factor, t.elapsed());
        assert!(r.realtime_factor > 0.0);
    }
    let calibration = std::fs::read_to_string(data.join("calibration.json")).unwrap();
    eprintln!("calibration.json: {calibration}");
    assert!(calibration.contains("gigaam-v3-e2e-rnnt-int8") && calibration.contains("gemma-4-e2b-q4-0"));
    let after = backend.models_overview().await.unwrap();
    let giga = after.models.iter().find(|m| m.id == "gigaam-v3-e2e-rnnt-int8").unwrap();
    eprintln!("gigaam after calibration: {:?} \"{}\"", giga.verdict, giga.estimate);

    // Status events reached the sink.
    assert!(events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, EngineEvent::Status { status: MeetingStatus::Ready, .. })));

    backend.shutdown().await;
}
