//! End to end: synthetic two-track call → live transcription → stop → files, jobs, summary.

mod common;

use std::time::Duration;

use common::*;
use yc_core::events::EngineEvent;
use yc_core::meeting::{MeetingStatus, ParticipantHint, Stage, TrackKind};
use yc_engine::ExportFormat;
use yc_engine::testing::{FakeAsrFactory, SyntheticRecorder, TrackScript, silence, speech};
use yc_store::SearchFilters;

fn folder_of(env: &Env, folder: &str) -> std::path::PathBuf {
    env.library().join(folder)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn call_to_transcript_summary_and_tasks() {
    let env = Env::new(call_scripts());
    let engine = env.open().await;
    let mut rx = engine.events();

    let id = start_call(&env, &engine, "Синк по онбордингу").await;

    // Live transcription: segments are announced while the call is still running.
    let live = wait_event(&mut rx, 10, |e| {
        matches!(e, EngineEvent::LiveSegment { .. })
    })
    .await;
    let EngineEvent::LiveSegment { meeting, segment } = live else {
        unreachable!()
    };
    assert_eq!(meeting, id);
    assert!(!segment.text.is_empty());
    assert!(engine.recording().is_some());

    engine.bookmark(Some("важное".into())).expect("bookmark");
    let t = std::time::Instant::now();
    let stopped = engine.stop_recording().await.expect("stop");
    assert!(
        t.elapsed() < Duration::from_secs(3),
        "stop took {:?}",
        t.elapsed()
    );
    assert_eq!(stopped, id);
    assert!(engine.recording().is_none());

    // The transcript is on disk as soon as stop returns.
    let detail = engine.get_meeting(id).await.expect("meeting");
    let folder = folder_of(&env, &detail.folder);
    assert!(folder.join("meeting.json").is_file());
    assert!(folder.join("transcript.json").is_file());
    let md = std::fs::read_to_string(folder.join("transcript.md")).expect("md");
    assert!(md.contains(TEXT_A) && md.contains(TEXT_C));
    // Echo of the other side on the mic is dropped; the user's own line stays.
    let texts: Vec<_> = detail
        .segments
        .iter()
        .map(|s| (s.speaker.as_str(), s.text.as_str()))
        .collect();
    assert_eq!(
        texts,
        vec![("them", TEXT_A), ("me", TEXT_B), ("them", TEXT_C)],
        "segments: {:?}",
        detail.segments
    );
    assert_eq!(detail.segments[0].track, TrackKind::System);
    assert!(detail.segments[1].start_ms > detail.segments[0].end_ms);
    assert_eq!(detail.meeting.bookmarks.len(), 1);
    assert_eq!(detail.meeting.asr_model.as_deref(), Some("fake-asr"));
    assert!(
        detail.meeting.duration_ms >= 12_700,
        "{}",
        detail.meeting.duration_ms
    );
    assert!(
        !env.recording_dir(id).join("mic.pcm").exists(),
        "audio moved into the library"
    );

    // Follow-up jobs: diarise, summarise, archive, index.
    assert_eq!(ready(&engine, id).await, MeetingStatus::Ready);
    let detail = engine.get_meeting(id).await.expect("meeting");
    let speakers: Vec<_> = detail.segments.iter().map(|s| s.speaker.as_str()).collect();
    assert_eq!(speakers, vec!["s1", "me", "s2"]);
    assert_eq!(detail.meeting.participants.len(), 2);
    assert_eq!(detail.meeting.participants[0].name, "Спикер 1");
    let summary = detail.summary.expect("summary");
    assert_eq!(summary.tasks.len(), 1);
    assert!(summary.tasks[0].text.contains("созвониться"));
    assert!(
        folder_of(&env, &detail.folder)
            .join("summary.json")
            .is_file()
    );
    // The user's own title survives; the summary's suggestion is only for auto titles.
    assert_eq!(detail.meeting.title, "Синк по онбордингу");
    // Retention default (30 days): audio is archived as FLAC, raw pcm gone.
    let audio = folder_of(&env, &detail.folder).join("audio");
    assert!(audio.join("them.flac").is_file() && audio.join("mic.flac").is_file());
    assert!(!audio.join("them.pcm").exists());
    assert_eq!(detail.audio.len(), 2);
    assert!(!env.recording_dir(id).exists());

    // Tasks are mirrored and can be ticked off.
    let tasks = engine.list_tasks(true).await.expect("tasks");
    assert_eq!(tasks.len(), 1);
    engine.set_task_done(id, 0, true).await.expect("done");
    assert!(engine.list_tasks(true).await.expect("tasks").is_empty());

    // Search sees the text (lexical + semantic after the re-index job).
    engine
        .wait_ready(id, Duration::from_secs(10))
        .await
        .expect("settled");
    let hits = engine
        .search("созвониться".into(), SearchFilters::default(), 5)
        .await
        .expect("search");
    assert_eq!(hits.first().map(|h| h.meeting_id), Some(id));

    // Exports.
    let md = engine.export(id, ExportFormat::Markdown).await.expect("md");
    assert!(md.contains(TEXT_B));
    let json = engine.export(id, ExportFormat::Json).await.expect("json");
    assert!(json.contains("\"summary\"") && json.contains("созвониться"));
    assert!(
        engine
            .copy_for_claude(id)
            .await
            .expect("claude")
            .contains(TEXT_A)
    );

    // Status events reached Ready.
    let mut saw_ready = false;
    while let Ok(ev) = rx.try_recv() {
        saw_ready |= matches!(
            ev,
            EngineEvent::Status {
                status: MeetingStatus::Ready,
                ..
            }
        );
    }
    assert!(saw_ready);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn naming_a_speaker_teaches_the_voice_for_the_next_meeting() {
    let env = Env::new(call_scripts());
    let engine = env.open().await;
    let first = record_call_with_hints(
        &env,
        &engine,
        "Первый",
        vec![ParticipantHint {
            name: "Анна".into(),
            email: Some("anna@example.com".into()),
        }],
    )
    .await;
    ready(&engine, first).await;

    engine
        .rename_speaker(first, "s1".into(), "Анна".into())
        .await
        .expect("rename");
    let detail = engine.get_meeting(first).await.expect("meeting");
    assert_eq!(detail.meeting.participants[0].name, "Анна");
    let voice_id = detail.meeting.participants[0]
        .voice_id
        .as_deref()
        .expect("voice profile");
    assert_eq!(
        detail.meeting.participant_hints[0].email.as_deref(),
        Some("anna@example.com")
    );
    let contacts: serde_json::Value = serde_json::from_slice(
        &std::fs::read(env.library().join("voice-emails.json")).expect("voice emails saved"),
    )
    .expect("voice emails are JSON");
    assert_eq!(contacts[voice_id][0], "anna@example.com");
    assert!(
        engine
            .copy_for_claude(first)
            .await
            .unwrap()
            .contains("Анна")
    );
    let voices = engine.list_voices().await.expect("voices");
    assert_eq!(voices.len(), 1);
    assert_eq!(voices[0].1, "Анна");

    // Same voices in a second call: the first speaker is recognised by name.
    engine
        .wait_ready(first, Duration::from_secs(10))
        .await
        .expect("settled");
    let second = record_call(&env, &engine, "Второй").await;
    ready(&engine, second).await;
    let detail = engine.get_meeting(second).await.expect("meeting");
    let anna = detail
        .meeting
        .participants
        .iter()
        .find(|p| p.name == "Анна")
        .expect("recognised");
    assert!(anna.voice_id.is_some());
    // The summary got the recognised name.
    let inputs = env.summarizer.inputs.lock().unwrap();
    assert!(
        inputs
            .last()
            .unwrap()
            .participants
            .iter()
            .any(|p| p.name == "Анна")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keep_audio_none_deletes_audio_after_the_transcript_is_written() {
    let env = Env::new(call_scripts());
    let engine = env.open().await;
    engine
        .update_settings(|s| s.keep_audio = yc_engine::settings::KeepAudio::None)
        .await
        .unwrap();
    let id = record_call(&env, &engine, "Без аудио").await;
    ready(&engine, id).await;
    let detail = engine.get_meeting(id).await.unwrap();
    let folder = folder_of(&env, &detail.folder);
    assert!(folder.join("transcript.json").is_file());
    assert!(!folder.join("audio").exists(), "audio removed");
    assert!(detail.audio.is_empty());
    assert!(!detail.segments.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retention_sweep_removes_old_audio_only() {
    let env = Env::new(call_scripts());
    let engine = env.open().await;
    engine
        .update_settings(|s| s.keep_audio = yc_engine::settings::KeepAudio::Days7)
        .await
        .unwrap();
    let id = record_call(&env, &engine, "Старый").await;
    ready(&engine, id).await;
    let folder = folder_of(&env, &engine.get_meeting(id).await.unwrap().folder);
    assert!(folder.join("audio/them.flac").is_file());
    assert_eq!(
        engine.sweep_retention().await.unwrap(),
        0,
        "fresh meeting stays"
    );
    let later = chrono::Utc::now() + chrono::Duration::days(8);
    assert_eq!(engine.sweep_retention_at(later).await.unwrap(), 1);
    assert!(!folder.join("audio").exists());
    assert!(folder.join("transcript.md").is_file(), "text is forever");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_deletes_everything() {
    let env = Env::new(call_scripts());
    let engine = env.open().await;
    let id = start_call(&env, &engine, "Отмена").await;
    engine.cancel_recording().await.expect("cancel");
    assert!(!env.recording_dir(id).exists());
    assert!(engine.recording().is_none());
    assert!(engine.list_meetings(10, 0).await.unwrap().is_empty());
    assert!(engine.stop_recording().await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_one_recording_at_a_time() {
    let env = Env::new(call_scripts());
    let engine = env.open().await;
    let _ = engine
        .start_recording(Default::default())
        .await
        .expect("first");
    assert!(engine.start_recording(Default::default()).await.is_err());
    engine.cancel_recording().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rename_and_delete_meeting() {
    let env = Env::new(call_scripts());
    let engine = env.open().await;
    let id = record_call(&env, &engine, "Старое имя").await;
    ready(&engine, id).await;
    engine.rename_meeting(id, "Новое имя".into()).await.unwrap();
    let detail = engine.get_meeting(id).await.unwrap();
    assert_eq!(detail.meeting.title, "Новое имя");
    assert!(detail.folder.contains("Новое имя"));
    assert!(
        env.library().join(&detail.folder).join("audio").is_dir(),
        "audio travelled with the folder"
    );
    engine
        .wait_ready(id, Duration::from_secs(10))
        .await
        .unwrap();
    engine.delete_meeting(id).await.unwrap();
    assert!(engine.get_meeting(id).await.is_err());
    assert!(!env.library().join(&detail.folder).exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ask_streams_an_answer_over_search_hits() {
    let env = Env::new(call_scripts());
    let engine = env.open().await;
    let id = record_call(&env, &engine, "Для вопросов").await;
    ready(&engine, id).await;
    let mut rx = engine.ask("созвониться".into()).await.expect("ask");
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        let done = matches!(ev, yc_engine::ports::AnswerEvent::Done);
        events.push(ev);
        if done {
            break;
        }
    }
    assert!(
        matches!(events.first(), Some(yc_engine::ports::AnswerEvent::Egress(m)) if m.starts_with('1') || m.contains("excerpts"))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, yc_engine::ports::AnswerEvent::Token(_)))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, yc_engine::ports::AnswerEvent::Citations(c) if !c.is_empty()))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn settings_persist_across_reopen() {
    let env = Env::new(call_scripts());
    {
        let engine = env.open().await;
        engine
            .update_settings(|s| {
                s.accent = "rose".into();
                s.hotkeys.bookmark = "F9".into();
            })
            .await
            .unwrap();
    }
    let engine = env.open().await;
    let s = engine.settings();
    assert_eq!(s.accent, "rose");
    assert_eq!(s.hotkeys.bookmark, "F9");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_asr_model_keeps_the_audio_and_retry_recovers_the_text() {
    let env = Env::new(call_scripts());
    env.asr
        .missing
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let engine = env.open().await;
    // Even with retention "none" the only copy of the words must survive.
    engine
        .update_settings(|s| s.keep_audio = yc_engine::settings::KeepAudio::None)
        .await
        .unwrap();
    let id = record_call(&env, &engine, "Без модели").await;
    let status = ready(&engine, id).await;
    assert!(
        matches!(
            status,
            MeetingStatus::Failed {
                stage: Stage::Transcribe,
                retryable: true,
                ..
            }
        ),
        "{status:?}"
    );
    let detail = engine.get_meeting(id).await.unwrap();
    assert!(detail.segments.is_empty());
    assert_eq!(detail.audio.len(), 2, "audio kept as FLAC");

    env.asr
        .missing
        .store(false, std::sync::atomic::Ordering::Relaxed);
    engine.retry_failed(id).await.expect("retry");
    assert_eq!(ready(&engine, id).await, MeetingStatus::Ready);
    let detail = engine.get_meeting(id).await.unwrap();
    let texts: Vec<_> = detail.segments.iter().map(|s| s.text.as_str()).collect();
    assert!(
        texts.contains(&TEXT_A) && texts.contains(&TEXT_B) && texts.contains(&TEXT_C),
        "{texts:?}"
    );
    // Retention "none" now applies: the text is safe, the audio goes.
    eventually(10, "audio deleted", || {
        !env.library().join(&detail.folder).join("audio").exists()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn frames_lost_live_are_transcribed_in_the_finalize_pass() {
    // One long utterance whose middle (1.28 s – 2.56 s) never reaches the live pipeline: the three
    // parts have different levels so the fake recogniser tells them apart.
    let mut sys = silence(500);
    sys.extend(speech(0.10, 780));
    sys.extend(speech(0.15, 1280));
    sys.extend(speech(0.40, 1440));
    sys.extend(silence(1000));
    let scripts = vec![TrackScript {
        kind: yc_core::meeting::TrackKind::System,
        offset_ms: 0,
        audio: sys,
    }];
    let mut recorder = SyntheticRecorder::new(scripts);
    recorder.drop_frames_after = Some(40);
    let mut env = Env::with_recorder(recorder);
    env.asr = std::sync::Arc::new(FakeAsrFactory::new(&[
        (0.10, "начало фразы"),
        (0.15, "середина фразы"),
        (0.40, "конец фразы"),
    ]));
    let engine = env.open().await;
    let id = engine.start_recording(Default::default()).await.unwrap();
    let dir = env.recording_dir(id);
    eventually(10, "audio written", || {
        std::fs::metadata(dir.join("them.pcm")).is_ok_and(|m| m.len() >= 5_000 * 32)
    })
    .await;
    engine.stop_recording().await.unwrap();
    ready(&engine, id).await;
    let detail = engine.get_meeting(id).await.unwrap();
    let texts: Vec<_> = detail.segments.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(
        texts,
        vec!["начало фразы", "середина фразы", "конец фразы"],
        "{:?}",
        detail.segments
    );
    let middle = &detail.segments[1];
    assert!(
        (1_200..=1_400).contains(&middle.start_ms),
        "{}",
        middle.start_ms
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_leaves_a_gap_on_the_timeline() {
    let mut recorder = SyntheticRecorder::new(call_scripts());
    recorder.pace = Some(Duration::from_millis(3));
    let env = Env::with_recorder(recorder);
    let engine = env.open().await;
    let id = engine.start_recording(Default::default()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    engine.pause_recording().unwrap();
    assert!(engine.recording().unwrap().paused);
    tokio::time::sleep(Duration::from_millis(600)).await;
    engine.resume_recording().unwrap();
    assert!(!engine.recording().unwrap().paused);
    // Wait for the scripted audio to finish (the file stops growing).
    let pcm = env.recording_dir(id).join("them.pcm");
    let mut last = (0u64, 0);
    eventually(30, "audio settles", || {
        let len = std::fs::metadata(&pcm).map_or(0, |m| m.len());
        last = if len == last.0 {
            (len, last.1 + 1)
        } else {
            (len, 0)
        };
        last.1 >= 10 && len > 0
    })
    .await;
    engine.stop_recording().await.unwrap();
    let detail = engine.get_meeting(id).await.unwrap();
    let audio_ms = detail
        .meeting
        .tracks
        .iter()
        .map(|t| t.samples * 1000 / 16_000)
        .max()
        .unwrap();
    // The paused ~0.6 s is on the timeline but not in the files.
    assert!(
        detail.meeting.duration_ms >= audio_ms + 550,
        "duration {} vs audio {audio_ms}",
        detail.meeting.duration_ms
    );
    assert!(audio_ms < 12_800);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recorder_events_become_levels_notices_and_track_events() {
    use yc_audio::RecorderEvent;
    use yc_core::error::PermissionKind;
    use yc_core::events::NoticeLevel;
    let mut recorder = SyntheticRecorder::new(call_scripts());
    recorder.pace = Some(Duration::from_millis(10)); // keep the "call" playing for ~4 s
    let env = Env::with_recorder(recorder);
    let engine = env.open().await;
    let mut rx = engine.events();
    let id = engine.start_recording(Default::default()).await.unwrap();

    let ev = wait_event(&mut rx, 5, |e| matches!(e, EngineEvent::Levels { .. })).await;
    assert!(matches!(ev, EngineEvent::Levels { .. }));
    wait_event(
        &mut rx,
        5,
        |e| matches!(e, EngineEvent::Levels { mic, .. } if mic.rms > 0.0),
    )
    .await;
    wait_event(&mut rx, 5, |e| {
        matches!(
            e,
            EngineEvent::Recorder {
                state: yc_core::events::RecorderState::Recording,
                ..
            }
        )
    })
    .await;

    env.recorder.emit(RecorderEvent::PermissionSuspected {
        track: TrackKind::System,
        permission: PermissionKind::SystemAudio,
        silent_for_ms: 10_000,
    });
    wait_event(&mut rx, 5, |e| {
        matches!(
            e,
            EngineEvent::Notice {
                level: NoticeLevel::Warning,
                ..
            }
        )
    })
    .await;

    env.recorder.emit(RecorderEvent::TrackSilent {
        track: TrackKind::Mic,
        silent_for_ms: 60_000,
    });
    let ev = wait_event(&mut rx, 5, |e| matches!(e, EngineEvent::TrackSilent { .. })).await;
    assert!(
        matches!(ev, EngineEvent::TrackSilent { meeting, track: TrackKind::Mic, silent_for_ms: 60_000 } if meeting == id)
    );
    wait_event(&mut rx, 5, |e| matches!(e, EngineEvent::Notice { .. })).await;

    env.recorder.emit(RecorderEvent::DeviceChanged {
        track: TrackKind::Mic,
        device: "AirPods".into(),
    });
    wait_event(
        &mut rx,
        5,
        |e| matches!(e, EngineEvent::DeviceChanged { device, .. } if device == "AirPods"),
    )
    .await;

    engine.cancel_recording().await.unwrap();
    wait_event(&mut rx, 5, |e| {
        matches!(
            e,
            EngineEvent::Recorder {
                state: yc_core::events::RecorderState::Cancelled,
                ..
            }
        )
    })
    .await;
}
