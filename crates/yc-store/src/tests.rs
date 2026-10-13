use std::{fs, path::PathBuf};

use chrono::{TimeZone, Utc};
use yc_core::{
    meeting::{Meeting, MeetingId, MeetingStatus, TrackInfo, TrackKind},
    summary::{Summary, Task},
    transcript::{Segment, Transcript},
};

use crate::{JobState, SearchFilters, Store};

struct TestLibrary(PathBuf);

impl TestLibrary {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("yc-store-test-{}", MeetingId::new()));
        fs::create_dir_all(&root).expect("create temporary library");
        Self(root)
    }
}

impl Drop for TestLibrary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn meeting(title: &str) -> Meeting {
    Meeting {
        id: MeetingId::new(),
        title: title.to_owned(),
        title_is_auto: false,
        started_at: Utc
            .with_ymd_and_hms(2026, 9, 28, 14, 30, 0)
            .single()
            .expect("valid date"),
        duration_ms: 60_000,
        language: "ru".to_owned(),
        expected_speakers: None,
        expected_speakers_explicit: false,
        participant_hints: Vec::new(),
        tracks: vec![
            TrackInfo {
                kind: TrackKind::Mic,
                device: "mic".to_owned(),
                offset_ms: 0,
                samples: 0,
            },
            TrackInfo {
                kind: TrackKind::System,
                device: "loopback".to_owned(),
                offset_ms: 0,
                samples: 0,
            },
        ],
        bookmarks: Vec::new(),
        participants: Vec::new(),
        asr_model: Some("test-asr".to_owned()),
        status: MeetingStatus::Ready,
    }
}

fn transcript() -> Transcript {
    Transcript {
        segments: vec![
            Segment {
                start_ms: 1_000,
                end_ms: 2_300,
                track: TrackKind::Mic,
                speaker: "me".to_owned(),
                text: "Проверили настройки экрана разрешений для нового приложения".to_owned(),
                words: Vec::new(),
                confidence: Some(0.98),
                corrections: Vec::new(),
            },
            Segment {
                start_ms: 2_500,
                end_ms: 3_500,
                track: TrackKind::System,
                speaker: "them".to_owned(),
                text: "Экран разрешений теперь открывается правильно".to_owned(),
                words: Vec::new(),
                confidence: Some(0.95),
                corrections: Vec::new(),
            },
        ],
    }
}

fn summary() -> Summary {
    Summary {
        title: None,
        about: vec!["Обсудили настройки доступа.".to_owned()],
        decisions: Vec::new(),
        tasks: vec![Task {
            text: "Проверить разрешения".to_owned(),
            owner: Some("me".to_owned()),
            due: None,
            at_ms: Some(3_000),
            done: false,
        }],
        open_questions: Vec::new(),
        language: "ru".to_owned(),
        model: "test-summary".to_owned(),
    }
}

#[test]
fn empty_database_migrates_and_meeting_round_trips_through_files_index_and_search() {
    let library = TestLibrary::new();
    let store = Store::open_in_library(&library.0).expect("open and migrate empty database");
    let meeting = meeting("Проверка доступа");
    let transcript = transcript();
    let summary = summary();
    store
        .save_meeting(&meeting, &transcript, Some(&summary))
        .expect("save meeting files and index");

    let folder = store
        .list_meetings(10, 0)
        .expect("list meetings")
        .remove(0)
        .folder;
    let folder_path = library.0.join(&folder);
    for name in [
        "meeting.json",
        "transcript.json",
        "transcript.md",
        "summary.json",
    ] {
        assert!(
            folder_path.join(name).is_file(),
            "expected {name} in {folder}"
        );
    }
    let loaded = store.get_meeting(meeting.id).expect("load meeting");
    assert_eq!(loaded.meeting, meeting);
    assert_eq!(loaded.transcript, transcript);
    assert_eq!(loaded.summary, Some(summary.clone()));
    assert_eq!(store.list_tasks(true).expect("list open tasks").len(), 1);

    let hits = store
        .search("экрана разрешений", None, &SearchFilters::default(), 10)
        .expect("stemmed search");
    assert!(
        !hits.is_empty(),
        "query word forms should find indexed segment text"
    );
    assert_eq!(hits[0].meeting_id, meeting.id);
    assert!(hits[0].snippet.contains("**экрана**") || hits[0].snippet.contains("**Экран**"));

    let mut renamed = meeting.clone();
    renamed.title = "Новый заголовок".to_owned();
    store
        .save_meeting(&renamed, &transcript, Some(&summary))
        .expect("rename folder with title");
    assert!(!folder_path.exists());
    assert!(
        store
            .get_meeting(meeting.id)
            .expect("load renamed folder")
            .folder
            .contains("Новый заголовок")
    );
}

#[test]
fn toy_four_dimension_vectors_drive_cosine_search() {
    let library = TestLibrary::new();
    let store = Store::open_in_library(&library.0).expect("open store");
    let meeting = meeting("Семантический поиск");
    store
        .save_meeting(&meeting, &transcript(), None)
        .expect("save meeting");
    store
        .configure_embedding_model("toy-four", 4)
        .expect("configure test embedding model");
    store
        .set_chunk_embeddings(meeting.id, "toy-four", &[vec![1.0, 0.0, 0.0, 0.0]])
        .expect("index vector");

    let query = [0.99, 0.01, 0.0, 0.0];
    let hits = store
        .search(
            "нового приложения",
            Some(&query),
            &SearchFilters::default(),
            5,
        )
        .expect("vector search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].meeting_id, meeting.id);
    assert!(hits[0].chunk_id.is_some());
    assert!(
        hits[0].segment_idx.is_some(),
        "overlapping FTS and vector spans should fuse"
    );

    store
        .scan()
        .expect("rebuild text index while preserving vectors");
    let rebuilt = store
        .search("", Some(&query), &SearchFilters::default(), 5)
        .expect("search vectors after scan");
    assert_eq!(rebuilt.len(), 1);
    assert!(rebuilt[0].chunk_id.is_some());
}

#[test]
fn jobs_claim_retry_complete_and_reset_running() {
    let library = TestLibrary::new();
    let store = Store::open_in_library(&library.0).expect("open store");
    let meeting = meeting("Jobs");
    store
        .save_meeting(&meeting, &Transcript::default(), None)
        .expect("save meeting");
    let job_id = store
        .enqueue_job(
            meeting.id,
            "transcribe",
            &serde_json::json!({"track": "mic"}),
        )
        .expect("enqueue job");

    let first = store
        .claim_next(&["transcribe"])
        .expect("claim job")
        .expect("job available");
    assert_eq!(first.id, job_id);
    assert_eq!(first.state, JobState::Running);
    assert_eq!(first.attempts, 1);
    store
        .fail(job_id, "temporary issue", true)
        .expect("retry job");
    let second = store
        .claim_next(&["transcribe"])
        .expect("claim retry")
        .expect("retry available");
    assert_eq!(second.attempts, 2);
    store.complete(job_id).expect("complete retry");
    assert!(
        store
            .claim_next(&["transcribe"])
            .expect("no more jobs")
            .is_none()
    );

    let reset_id = store
        .enqueue_job(meeting.id, "index", &serde_json::Value::Null)
        .expect("enqueue index job");
    assert_eq!(
        store
            .claim_next(&["index"])
            .expect("claim index")
            .expect("job available")
            .id,
        reset_id
    );
    assert_eq!(store.scan().expect("reindex source files"), 1);
    assert_eq!(
        store
            .reset_running_on_startup()
            .expect("requeue running jobs"),
        1
    );
    assert_eq!(
        store
            .claim_next(&["index"])
            .expect("claim requeued")
            .expect("job available")
            .attempts,
        2
    );
}

#[test]
fn scan_rebuilds_the_index_from_meeting_folders() {
    let library = TestLibrary::new();
    let database_path = library.0.join("index.sqlite");
    let meeting = meeting("Rebuild index");
    {
        let store = Store::open(&library.0, &database_path).expect("open store");
        store
            .save_meeting(&meeting, &transcript(), Some(&summary()))
            .expect("save files");
    }
    fs::remove_file(&database_path).expect("remove disposable index");
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{}", database_path.display(), suffix));
        if sidecar.exists() {
            fs::remove_file(sidecar).expect("remove database sidecar");
        }
    }
    let store = Store::open(&library.0, &database_path).expect("recreate database");
    assert_eq!(store.scan().expect("scan source folders"), 1);
    assert_eq!(
        store
            .list_meetings(10, 0)
            .expect("list rebuilt index")
            .len(),
        1
    );
    assert!(
        !store
            .search("разрешений", None, &SearchFilters::default(), 10)
            .expect("search rebuilt index")
            .is_empty()
    );
}

#[test]
fn task_done_is_mirrored_to_summary_and_meeting_delete_removes_the_folder() {
    let library = TestLibrary::new();
    let store = Store::open_in_library(&library.0).expect("open store");
    let meeting = meeting("Task lifecycle");
    store
        .save_meeting(&meeting, &Transcript::default(), Some(&summary()))
        .expect("save meeting with task");
    let folder = store
        .list_meetings(1, 0)
        .expect("list meeting")
        .remove(0)
        .folder;

    store
        .set_task_done(meeting.id, 0, true)
        .expect("mark task done");
    assert!(store.list_tasks(true).expect("list open tasks").is_empty());
    assert!(
        store
            .get_meeting(meeting.id)
            .expect("read updated files")
            .summary
            .expect("summary")
            .tasks[0]
            .done
    );

    store
        .delete_meeting(meeting.id)
        .expect("delete meeting explicitly");
    assert!(!library.0.join(folder).exists());
    assert!(
        store
            .list_meetings(10, 0)
            .expect("list after delete")
            .is_empty()
    );
}

#[test]
fn store_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Store>();
}
