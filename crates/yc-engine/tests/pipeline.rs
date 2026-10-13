//! Crash recovery, imports and the job runner's retry behaviour.

mod common;

use std::sync::atomic::Ordering;

use common::*;
use yc_core::Error;
use yc_core::events::EngineEvent;
use yc_core::meeting::{MeetingStatus, Stage};
use yc_engine::ImportOptions;
use yc_engine::testing::{silence, speech};

fn assert_standard_transcript(texts: &[(String, String)]) {
    let got: Vec<(&str, &str)> = texts
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![("them", TEXT_A), ("me", TEXT_B), ("them", TEXT_C)],
        "no duplicates, nothing lost"
    );
}

async fn transcript_of(
    engine: &std::sync::Arc<yc_engine::Engine>,
    id: yc_core::meeting::MeetingId,
) -> Vec<(String, String)> {
    engine
        .get_meeting(id)
        .await
        .unwrap()
        .segments
        .into_iter()
        .map(|s| (s.speaker, s.text))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killed_mid_recording_then_recovered_on_next_start() {
    let env = Env::new(call_scripts());
    let id = {
        let engine = env.open().await;
        let mut rx = engine.events();
        let id = start_call(&env, &engine, "Упавшая запись").await;
        // Let live transcription save something, then "kill" the process: drop without stop.
        wait_event(&mut rx, 10, |e| {
            matches!(e, EngineEvent::LiveSegment { .. })
        })
        .await;
        id
    };
    // (engine dropped here)
    assert!(env.recording_dir(id).join("live.jsonl").is_file());
    assert!(
        env.library()
            .read_dir()
            .map_or(true, |mut d| d.next().is_none()),
        "nothing finalised yet"
    );

    let engine = env.open().await;
    let pending = engine.pending_recoveries().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, id);
    assert_eq!(pending[0].title, "Упавшая запись");
    assert!(pending[0].audio_ms >= 12_700);
    assert!(pending[0].live_segments >= 1);

    assert_eq!(engine.recover(id).await.unwrap(), id);
    assert_eq!(ready(&engine, id).await, MeetingStatus::Ready);
    assert_standard_transcript(&transcript_of(&engine, id).await);
    let detail = engine.get_meeting(id).await.unwrap();
    assert_eq!(detail.meeting.title, "Упавшая запись");
    assert!(detail.meeting.duration_ms >= 12_700);
    assert!(engine.pending_recoveries().unwrap().is_empty());
    assert!(!env.recording_dir(id).exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovery_without_any_live_text_transcribes_everything_offline() {
    let env = Env::new(call_scripts());
    env.asr.missing.store(true, Ordering::Relaxed); // no live transcription at all
    let id = {
        let engine = env.open().await;
        start_call(&env, &engine, "Без живого текста").await
    };
    env.asr.missing.store(false, Ordering::Relaxed);
    let engine = env.open().await;
    assert_eq!(engine.pending_recoveries().unwrap()[0].live_segments, 0);
    engine.recover(id).await.unwrap();
    assert_eq!(ready(&engine, id).await, MeetingStatus::Ready);
    assert_standard_transcript(&transcript_of(&engine, id).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovery_survives_a_missing_model_and_can_be_retried() {
    let env = Env::new(call_scripts());
    env.asr.missing.store(true, Ordering::Relaxed);
    let id = {
        let engine = env.open().await;
        start_call(&env, &engine, "Модель позже").await
    };
    let engine = env.open().await;
    let mut rx = engine.events();
    engine.recover(id).await.unwrap();
    // Without a model the meeting is saved as failed-but-retryable with its audio intact.
    let status = ready(&engine, id).await;
    assert!(
        matches!(
            status,
            MeetingStatus::Failed {
                stage: Stage::Transcribe,
                ..
            }
        ),
        "{status:?}"
    );
    drop(rx.try_recv());
    env.asr.missing.store(false, Ordering::Relaxed);
    engine.retry_failed(id).await.unwrap();
    assert_eq!(ready(&engine, id).await, MeetingStatus::Ready);
    assert_standard_transcript(&transcript_of(&engine, id).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discarding_a_recovery_removes_it() {
    let env = Env::new(call_scripts());
    let id = {
        let engine = env.open().await;
        start_call(&env, &engine, "Выбросить").await
    };
    let engine = env.open().await;
    assert_eq!(engine.pending_recoveries().unwrap().len(), 1);
    engine.discard_recovery(id).await.unwrap();
    assert!(engine.pending_recoveries().unwrap().is_empty());
}

fn wav(samples: &[f32], rate: u32) -> Vec<u8> {
    let data: Vec<u8> = samples
        .iter()
        .flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
        .collect();
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * 2).to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(&data);
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn import_audio_file_becomes_a_diarised_meeting() {
    let env = Env::new(vec![]);
    let engine = env.open().await;
    let mut audio = silence(500);
    audio.extend(speech(A, 9000));
    audio.extend(silence(3000));
    audio.extend(speech(C, 9000));
    audio.extend(silence(1000));
    let file = env.dir.path().join("Созвон с клиентом.wav");
    std::fs::write(&file, wav(&audio, 16_000)).unwrap();

    let id = engine
        .import_file(&file, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(ready(&engine, id).await, MeetingStatus::Ready);
    let detail = engine.get_meeting(id).await.unwrap();
    assert_eq!(detail.meeting.title, "Созвон с клиентом");
    let texts: Vec<_> = detail.segments.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(texts, vec![TEXT_A, TEXT_C]);
    // Only a "them" track; two voices found.
    assert!(
        detail
            .segments
            .iter()
            .all(|s| s.track == yc_core::meeting::TrackKind::System)
    );
    let speakers: Vec<_> = detail.segments.iter().map(|s| s.speaker.as_str()).collect();
    assert_eq!(speakers, vec!["s1", "s2"]);
    assert!(detail.summary.is_some());
    assert!(detail.audio.contains(&yc_core::meeting::TrackKind::System));
    assert!(engine.pending_recoveries().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_expected_speaker_skips_diarisation() {
    let env = Env::new(vec![]);
    let engine = env.open().await;
    let mut audio = silence(500);
    audio.extend(speech(A, 3500));
    audio.extend(silence(2000));
    audio.extend(speech(C, 3500));
    audio.extend(silence(1000));
    let file = env.dir.path().join("lecture.wav");
    std::fs::write(&file, wav(&audio, 16_000)).unwrap();
    let id = engine
        .import_file(
            &file,
            ImportOptions {
                expected_speakers: Some(1),
                expected_speakers_explicit: true,
                title: Some("Лекция".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    ready(&engine, id).await;
    let detail = engine.get_meeting(id).await.unwrap();
    assert!(detail.segments.iter().all(|s| s.speaker == "them"));
    assert_eq!(detail.meeting.title, "Лекция");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn importing_garbage_fails_cleanly() {
    let env = Env::new(vec![]);
    let engine = env.open().await;
    let mut rx = engine.events();
    let file = env.dir.path().join("notes.mp3");
    std::fs::write(&file, b"this is not audio").unwrap();
    let id = engine
        .import_file(&file, ImportOptions::default())
        .await
        .unwrap();
    let ev = wait_event(&mut rx, 10, |e| {
        matches!(e, EngineEvent::Status { meeting, status: MeetingStatus::Failed { .. } } if *meeting == id)
    })
    .await;
    assert!(matches!(ev, EngineEvent::Status { .. }));
    assert!(engine.pending_recoveries().unwrap().is_empty());
    assert!(engine.list_meetings(10, 0).await.unwrap().is_empty());
    assert!(
        engine
            .import_file(env.dir.path().join("missing.wav"), ImportOptions::default())
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retryable_failures_are_retried_with_backoff() {
    let env = Env::new(call_scripts());
    env.summarizer.fail_first.store(2, Ordering::SeqCst);
    let engine = env.open().await;
    let id = record_call(&env, &engine, "Ретраи").await;
    assert_eq!(ready(&engine, id).await, MeetingStatus::Ready);
    assert_eq!(
        env.summarizer.calls.load(Ordering::SeqCst),
        3,
        "two failures, then success"
    );
    assert!(engine.get_meeting(id).await.unwrap().summary.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exhausted_retries_fail_the_stage_but_not_the_rest() {
    let env = Env::new(call_scripts());
    env.summarizer.fail_first.store(100, Ordering::SeqCst);
    let engine = env.open().await;
    let id = record_call(&env, &engine, "Сломанный провайдер").await;
    let status = ready(&engine, id).await;
    match status {
        MeetingStatus::Failed {
            stage,
            retryable,
            ref message,
        } => {
            assert_eq!(stage, Stage::Summarize);
            assert!(retryable);
            assert!(message.contains("provider unavailable"), "{message}");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        env.summarizer.calls.load(Ordering::SeqCst),
        3,
        "max_attempts"
    );
    // The transcript, audio archive and search index are unaffected.
    let detail = engine.get_meeting(id).await.unwrap();
    assert_eq!(detail.segments.len(), 3);
    assert!(detail.summary.is_none());
    assert_eq!(detail.audio.len(), 2);
    let flac = env.library().join(&detail.folder).join("audio/them.flac");
    eventually(10, "archived", || flac.is_file()).await;
    assert_eq!(
        engine
            .search("созвониться".into(), Default::default(), 3)
            .await
            .unwrap()
            .len(),
        1
    );

    // The provider comes back: regenerate.
    env.summarizer.fail_first.store(0, Ordering::SeqCst);
    engine.regenerate_summary(id).await.unwrap();
    assert_eq!(ready(&engine, id).await, MeetingStatus::Ready);
    assert!(engine.get_meeting(id).await.unwrap().summary.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_retryable_failures_are_not_retried_and_say_why() {
    let env = Env::new(call_scripts());
    env.summarizer.fail_first.store(1, Ordering::SeqCst);
    *env.summarizer.error.lock().unwrap() = Some(|| Error::Invalid("API key rejected".into()));
    let engine = env.open().await;
    let id = record_call(&env, &engine, "Плохой ключ").await;
    let status = ready(&engine, id).await;
    assert!(
        matches!(&status, MeetingStatus::Failed { stage: Stage::Summarize, retryable: false, message } if message.contains("API key rejected")),
        "{status:?}"
    );
    assert_eq!(env.summarizer.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn jobs_interrupted_by_a_restart_run_again() {
    let env = Env::new(call_scripts());
    // The runner stops right after the transcript is saved; the follow-up jobs wait in the queue.
    let id = {
        let engine = env.open().await;
        engine
            .update_settings(|s| s.keep_audio = yc_engine::settings::KeepAudio::Forever)
            .await
            .unwrap();
        let id = record_call(&env, &engine, "Перезапуск").await;
        // Stop the runner right after the transcript is saved.
        engine.shutdown();
        id
    };
    let engine = env.open().await;
    assert_eq!(ready(&engine, id).await, MeetingStatus::Ready);
    assert!(engine.get_meeting(id).await.unwrap().summary.is_some());
}

/// Throughput of the offline path (decode → VAD → ASR → finalize → diarise → index) on a long
/// file: `cargo test -p yc-engine --release --test pipeline -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "benchmark"]
async fn long_import_throughput() {
    let env = Env::new(vec![]);
    let engine = env.open().await;
    let minutes = 60u64;
    let file = env.dir.path().join("long.wav");
    {
        use std::io::Write;
        let mut audio = Vec::new();
        for i in 0..(minutes * 60 / 12) {
            audio.extend(silence(1500));
            audio.extend(speech(if i % 2 == 0 { A } else { C }, 4500));
            audio.extend(silence(2000));
            audio.extend(speech(B, 4000));
        }
        let mut f = std::fs::File::create(&file).unwrap();
        f.write_all(&wav(&audio, 16_000)).unwrap();
    }
    let t = std::time::Instant::now();
    let id = engine
        .import_file(&file, ImportOptions::default())
        .await
        .unwrap();
    let mut rx = engine.events();
    let transcript_ready = loop {
        if let EngineEvent::Notice { .. } = rx.recv().await.unwrap() {
            break t.elapsed();
        }
    };
    ready(&engine, id).await;
    let detail = engine.get_meeting(id).await.unwrap();
    println!(
        "{minutes} min audio: transcript in {transcript_ready:?}, all stages in {:?}, {} segments",
        t.elapsed(),
        detail.segments.len()
    );
}
