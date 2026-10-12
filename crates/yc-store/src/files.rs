use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use chrono::Utc;
use rusqlite::Transaction;
use yc_core::{
    Error, Result,
    meeting::{Meeting, MeetingId},
    summary::Summary,
    transcript::Transcript,
};
use yc_text::{
    build_turns, chunk_turns, meeting_folder_name, render_transcript_markdown, stem_text,
};

use crate::{Store, json_error, storage_error, types::MeetingRecord};

pub(crate) fn save_meeting(
    store: &Store,
    meeting: &Meeting,
    transcript: &Transcript,
    summary: Option<&Summary>,
) -> Result<()> {
    let previous = store
        .indexed_folder(meeting.id)?
        .filter(|folder| store.root().join(folder).join("meeting.json").is_file())
        .or_else(|| find_folder_by_id(store.root(), meeting.id).ok().flatten());
    let desired = unique_folder(store.root(), meeting, previous.as_deref())?;
    let target_path = store.root().join(&desired);
    if let Some(old_folder) = previous.as_deref() {
        let old_path = store.root().join(old_folder);
        if old_path != target_path && old_path.exists() {
            if target_path.exists() && folder_has_id(&target_path, meeting.id)? {
                fs::remove_dir_all(old_path)?;
            } else {
                fs::rename(&old_path, &target_path)?;
            }
        }
    } else {
        fs::create_dir_all(&target_path)?;
    }
    fs::create_dir_all(&target_path)?;
    atomic_write_json(&target_path.join("meeting.json"), meeting)?;
    atomic_write_json(&target_path.join("transcript.json"), transcript)?;
    atomic_write(
        &target_path.join("transcript.md"),
        render_transcript_markdown(meeting, &transcript.segments).as_bytes(),
    )?;
    match summary {
        Some(summary) => atomic_write_json(&target_path.join("summary.json"), summary)?,
        None => {
            let summary_path = target_path.join("summary.json");
            if summary_path.exists() {
                fs::remove_file(summary_path)?;
            }
        }
    }

    let connection = store.lock()?;
    let transaction = connection.unchecked_transaction().map_err(storage_error)?;
    index_meeting(&transaction, meeting, transcript, summary, &desired)?;
    transaction.commit().map_err(storage_error)
}

pub(crate) fn scan(store: &Store) -> Result<usize> {
    let mut records = Vec::new();
    for entry in fs::read_dir(store.root())? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let meeting_path = entry.path().join("meeting.json");
        if !meeting_path.is_file() {
            continue;
        }
        let folder = entry.file_name().to_string_lossy().into_owned();
        let meeting = read_json::<Meeting>(&meeting_path)?;
        let transcript_path = entry.path().join("transcript.json");
        let transcript = if transcript_path.exists() {
            read_json::<Transcript>(&transcript_path)?
        } else {
            Transcript::default()
        };
        let summary_path = entry.path().join("summary.json");
        let summary = if summary_path.exists() {
            Some(read_json::<Summary>(&summary_path)?)
        } else {
            None
        };
        records.push((meeting, transcript, summary, folder));
    }
    records.sort_by_key(|(meeting, _, _, _)| meeting.started_at);

    let connection = store.lock()?;
    let transaction = connection.unchecked_transaction().map_err(storage_error)?;
    transaction
        .execute_batch(
            "DROP TABLE IF EXISTS temp.preserved_chunk_vectors;
             CREATE TEMP TABLE preserved_chunk_vectors AS
             SELECT c.meeting_id,c.start_ms,c.end_ms,c.text,v.embedding
             FROM chunk_vectors v JOIN chunks c ON c.id=v.chunk_id;",
        )
        .map_err(storage_error)?;
    transaction
        .execute("DELETE FROM segments_fts", [])
        .map_err(storage_error)?;
    transaction
        .execute("DELETE FROM chunk_vectors", [])
        .map_err(storage_error)?;
    let retained_ids = records
        .iter()
        .map(|(meeting, _, _, _)| meeting.id.to_string())
        .collect::<HashSet<_>>();
    let mut statement = transaction
        .prepare("SELECT id FROM meetings")
        .map_err(storage_error)?;
    let indexed_ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(storage_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(storage_error)?;
    drop(statement);
    for id in indexed_ids {
        if !retained_ids.contains(&id) {
            transaction
                .execute("DELETE FROM meetings WHERE id=?1", [&id])
                .map_err(storage_error)?;
        }
    }
    for (meeting, transcript, summary, folder) in &records {
        index_meeting(&transaction, meeting, transcript, summary.as_ref(), folder)?;
    }
    transaction
        .execute(
            "INSERT INTO chunk_vectors(chunk_id,embedding)
         SELECT c.id,p.embedding FROM chunks c
         JOIN temp.preserved_chunk_vectors p ON p.meeting_id=c.meeting_id
           AND p.start_ms=c.start_ms AND p.end_ms=c.end_ms AND p.text=c.text",
            [],
        )
        .map_err(storage_error)?;
    transaction
        .execute_batch("DROP TABLE temp.preserved_chunk_vectors")
        .map_err(storage_error)?;
    transaction.commit().map_err(storage_error)?;
    Ok(records.len())
}

pub(crate) fn get_meeting(store: &Store, meeting_id: MeetingId) -> Result<MeetingRecord> {
    let folder = store
        .indexed_folder(meeting_id)?
        .or_else(|| find_folder_by_id(store.root(), meeting_id).ok().flatten())
        .ok_or_else(|| Error::NotFound(format!("meeting {meeting_id}")))?;
    let directory = store.root().join(&folder);
    let meeting = read_json::<Meeting>(&directory.join("meeting.json"))?;
    if meeting.id != meeting_id {
        return Err(Error::Storage(format!(
            "meeting id mismatch in folder {folder}"
        )));
    }
    let transcript_path = directory.join("transcript.json");
    let transcript = if transcript_path.exists() {
        read_json::<Transcript>(&transcript_path)?
    } else {
        Transcript::default()
    };
    let summary_path = directory.join("summary.json");
    let summary = if summary_path.exists() {
        Some(read_json::<Summary>(&summary_path)?)
    } else {
        None
    };
    Ok(MeetingRecord {
        meeting,
        transcript,
        summary,
        folder,
    })
}

pub(crate) fn delete_meeting(store: &Store, meeting_id: MeetingId) -> Result<()> {
    let folder = store
        .indexed_folder(meeting_id)?
        .or_else(|| find_folder_by_id(store.root(), meeting_id).ok().flatten());
    let had_folder = folder.is_some();
    if let Some(folder_name) = folder {
        let directory = store.root().join(folder_name);
        if directory.exists() {
            fs::remove_dir_all(directory)?;
        }
    }
    let connection = store.lock()?;
    let transaction = connection.unchecked_transaction().map_err(storage_error)?;
    transaction
        .execute(
            "DELETE FROM segments_fts WHERE rowid IN (SELECT id FROM segments WHERE meeting_id=?1)",
            [meeting_id.to_string()],
        )
        .map_err(storage_error)?;
    transaction.execute("DELETE FROM chunk_vectors WHERE chunk_id IN (SELECT id FROM chunks WHERE meeting_id=?1)", [meeting_id.to_string()]).map_err(storage_error)?;
    let count = transaction
        .execute("DELETE FROM meetings WHERE id=?1", [meeting_id.to_string()])
        .map_err(storage_error)?;
    if count == 0 && !had_folder {
        return Err(Error::NotFound(format!("meeting {meeting_id}")));
    }
    transaction.commit().map_err(storage_error)
}

fn index_meeting(
    transaction: &Transaction<'_>,
    meeting: &Meeting,
    transcript: &Transcript,
    summary: Option<&Summary>,
    folder: &str,
) -> Result<()> {
    let id = meeting.id.to_string();
    let started_at = meeting.started_at.to_rfc3339();
    let status = serde_json::to_string(&meeting.status).map_err(json_error)?;
    let now = Utc::now().to_rfc3339();
    transaction.execute(
        "INSERT INTO meetings(id,title,started_at,duration_ms,language,folder,status_json,asr_model,updated_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
         ON CONFLICT(id) DO UPDATE SET title=excluded.title, started_at=excluded.started_at,
         duration_ms=excluded.duration_ms, language=excluded.language, folder=excluded.folder,
         status_json=excluded.status_json, asr_model=excluded.asr_model, updated_at=excluded.updated_at",
        rusqlite::params![id, meeting.title, started_at, meeting.duration_ms.min(i64::MAX as u64) as i64,
            meeting.language, folder, status, meeting.asr_model, now],
    ).map_err(storage_error)?;
    transaction
        .execute(
            "DELETE FROM segments_fts WHERE rowid IN (SELECT id FROM segments WHERE meeting_id=?1)",
            [&id],
        )
        .map_err(storage_error)?;
    transaction.execute("DELETE FROM chunk_vectors WHERE chunk_id IN (SELECT id FROM chunks WHERE meeting_id=?1)", [&id]).map_err(storage_error)?;
    transaction
        .execute("DELETE FROM segments WHERE meeting_id=?1", [&id])
        .map_err(storage_error)?;
    transaction
        .execute("DELETE FROM chunks WHERE meeting_id=?1", [&id])
        .map_err(storage_error)?;
    transaction
        .execute("DELETE FROM tasks WHERE meeting_id=?1", [&id])
        .map_err(storage_error)?;

    for (index, segment) in transcript.segments.iter().enumerate() {
        let stemmed = stem_text(&segment.text);
        transaction.execute(
            "INSERT INTO segments(meeting_id,idx,start_ms,end_ms,speaker,track,text,stemmed_text) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![id, index as i64, segment.start_ms.min(i64::MAX as u64) as i64,
                segment.end_ms.min(i64::MAX as u64) as i64, segment.speaker,
                segment.track.file_stem(), segment.text, stemmed],
        ).map_err(storage_error)?;
        let rowid = transaction.last_insert_rowid();
        transaction
            .execute(
                "INSERT INTO segments_fts(rowid,stemmed_text) VALUES(?1,?2)",
                rusqlite::params![rowid, stemmed],
            )
            .map_err(storage_error)?;
    }

    let turns = build_turns(&transcript.segments);
    let is_english = meeting
        .language
        .split('-')
        .next()
        .is_some_and(|language| language.eq_ignore_ascii_case("en"));
    let mut speaker_names = vec![
        (
            "me".to_owned(),
            if is_english { "Me" } else { "Я" }.to_owned(),
        ),
        (
            "them".to_owned(),
            if is_english {
                "Them"
            } else {
                "Собеседник"
            }
            .to_owned(),
        ),
    ];
    for participant in &meeting.participants {
        if let Some((_, name)) = speaker_names
            .iter_mut()
            .find(|(key, _)| key == &participant.key)
        {
            *name = participant.name.clone();
        } else {
            speaker_names.push((participant.key.clone(), participant.name.clone()));
        }
    }
    for chunk in chunk_turns(meeting.id, &turns, &speaker_names) {
        let speakers = serde_json::to_string(&chunk.speakers).map_err(json_error)?;
        transaction.execute(
            "INSERT INTO chunks(meeting_id,start_ms,end_ms,speakers,text) VALUES(?1,?2,?3,?4,?5)",
            rusqlite::params![id, chunk.start_ms.min(i64::MAX as u64) as i64, chunk.end_ms.min(i64::MAX as u64) as i64, speakers, chunk.text],
        ).map_err(storage_error)?;
    }
    if let Some(summary) = summary {
        for (index, task) in summary.tasks.iter().enumerate() {
            transaction.execute(
                "INSERT INTO tasks(meeting_id,idx,text,owner,due,at_ms,done) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                rusqlite::params![id, index as i64, task.text, task.owner, task.due,
                    task.at_ms.map(|at_ms| at_ms.min(i64::MAX as u64) as i64), i64::from(task.done)],
            ).map_err(storage_error)?;
        }
    }
    Ok(())
}

fn unique_folder(root: &Path, meeting: &Meeting, current: Option<&str>) -> Result<String> {
    let desired = meeting_folder_name(meeting);
    let desired_path = root.join(&desired);
    if !desired_path.exists()
        || current == Some(desired.as_str())
        || folder_has_id(&desired_path, meeting.id)?
    {
        return Ok(desired);
    }
    let suffix = meeting.id.to_string();
    for width in [8, 12, 16, 32, 36] {
        let suffix = &suffix[..width];
        let candidate = format!(
            "{} {suffix}",
            desired
                .chars()
                .take(80usize.saturating_sub(width + 1))
                .collect::<String>()
        );
        let candidate_path = root.join(&candidate);
        if !candidate_path.exists() || folder_has_id(&candidate_path, meeting.id)? {
            return Ok(candidate);
        }
    }
    Err(Error::Storage(format!(
        "could not find a unique folder name for meeting {}",
        meeting.id
    )))
}

fn folder_has_id(path: &Path, meeting_id: MeetingId) -> Result<bool> {
    let metadata = path.join("meeting.json");
    if !metadata.is_file() {
        return Ok(false);
    }
    let meeting: Meeting = read_json(&metadata)?;
    Ok(meeting.id == meeting_id)
}

fn find_folder_by_id(root: &Path, meeting_id: MeetingId) -> Result<Option<String>> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let metadata = entry.path().join("meeting.json");
        if !metadata.is_file() {
            continue;
        }
        let meeting: Meeting = read_json(&metadata)?;
        if meeting.id == meeting_id {
            return Ok(Some(entry.file_name().to_string_lossy().into_owned()));
        }
    }
    Ok(None)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| Error::Storage(format!("{}: {error}", path.display())))
}

fn atomic_write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(json_error)?;
    atomic_write(path, &bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("atomic file write requires a parent folder".to_owned()))?;
    fs::create_dir_all(parent)?;
    let temp_path: PathBuf = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file"),
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed),
    ));
    let write_result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp_path, path)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result
}
