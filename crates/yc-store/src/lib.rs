//! Blocking meeting library, SQLite index, hybrid search, and background job queue.
//!
//! All methods do synchronous disk and database work. Call them from a blocking thread when used
//! by an async runtime.

mod db;
mod files;
mod jobs;
mod search;
mod types;

#[cfg(test)]
mod tests;

pub use types::{
    Hit, Job, JobState, MeetingListItem, MeetingRecord, SearchFilters, StoreVoice, TaskRecord,
};
pub use yc_core::meeting::Folder;

use std::{
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, Once},
};

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use yc_core::{
    Error, Result,
    meeting::{Meeting, MeetingId},
    summary::Summary,
    transcript::Transcript,
};

/// Synchronous library and rebuildable SQLite search index.
pub struct Store {
    library_root: PathBuf,
    connection: Mutex<Connection>,
}

impl Store {
    /// Open an existing library index without creating files or applying migrations.
    pub fn open_read_only(
        library_root: impl Into<PathBuf>,
        database_path: impl Into<PathBuf>,
    ) -> Result<Self> {
        register_sqlite_vec();
        let library_root = library_root.into();
        let database_path = database_path.into();
        let connection = Connection::open_with_flags(
            database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(storage_error)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(storage_error)?;
        Ok(Self {
            library_root,
            connection: Mutex::new(connection),
        })
    }

    /// Open a meeting library and its index database, creating either when absent.
    pub fn open(
        library_root: impl Into<PathBuf>,
        database_path: impl Into<PathBuf>,
    ) -> Result<Self> {
        let library_root = library_root.into();
        let database_path = database_path.into();
        std::fs::create_dir_all(&library_root)?;
        if let Some(parent) = database_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        register_sqlite_vec();
        let mut connection = Connection::open(database_path).map_err(storage_error)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(storage_error)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(storage_error)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(storage_error)?;
        db::migrate(&mut connection)?;
        Ok(Self {
            library_root,
            connection: Mutex::new(connection),
        })
    }

    /// Open a library with `index.sqlite` stored inside its root directory.
    pub fn open_in_library(library_root: impl Into<PathBuf>) -> Result<Self> {
        let library_root = library_root.into();
        Self::open(&library_root, library_root.join("index.sqlite"))
    }

    /// Return the library root used for meeting folders.
    pub fn library_root(&self) -> &Path {
        &self.library_root
    }

    /// Read user-managed folders from `folders.json` in the library root.
    pub fn list_folders(&self) -> Result<Vec<Folder>> {
        files::list_folders(self)
    }

    /// Create a folder with a stable UUID v7 identifier.
    pub fn create_folder(&self, name: String, color: Option<String>) -> Result<Folder> {
        let name = name.trim().to_owned();
        if name.is_empty() {
            return Err(Error::Invalid("a folder name cannot be empty".into()));
        }
        let mut folders = self.list_folders()?;
        if folders.iter().any(|f| f.name.eq_ignore_ascii_case(&name)) {
            return Err(Error::Invalid(
                "a folder with this name already exists".into(),
            ));
        }
        let folder = Folder {
            id: uuid::Uuid::now_v7().to_string(),
            name,
            color,
            order: folders
                .iter()
                .map(|f| f.order)
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        };
        folders.push(folder.clone());
        files::save_folders(self, &folders)?;
        Ok(folder)
    }

    /// Update folder fields, preserving omitted values.
    pub fn update_folder(
        &self,
        id: &str,
        name: Option<String>,
        color: Option<Option<String>>,
        order: Option<u32>,
    ) -> Result<Folder> {
        let mut folders = self.list_folders()?;
        if let Some(candidate) = name.as_ref().map(|n| n.trim()).filter(|n| !n.is_empty())
            && folders
                .iter()
                .any(|f| f.id != id && f.name.eq_ignore_ascii_case(candidate))
        {
            return Err(Error::Invalid(
                "a folder with this name already exists".into(),
            ));
        }
        let folder = folders
            .iter_mut()
            .find(|f| f.id == id)
            .ok_or_else(|| Error::NotFound(format!("folder {id}")))?;
        if let Some(name) = name {
            let name = name.trim().to_owned();
            if name.is_empty() {
                return Err(Error::Invalid("a folder name cannot be empty".into()));
            }
            folder.name = name;
        }
        if let Some(color) = color {
            folder.color = color;
        }
        if let Some(order) = order {
            folder.order = order;
        }
        let result = folder.clone();
        files::save_folders(self, &folders)?;
        Ok(result)
    }

    /// Delete a folder and move its meetings to the unassigned group.
    pub fn delete_folder(&self, id: &str) -> Result<()> {
        files::delete_folder(self, id)
    }

    /// Write meeting files atomically and synchronize the corresponding database rows.
    pub fn save_meeting(
        &self,
        meeting: &Meeting,
        transcript: &Transcript,
        summary: Option<&Summary>,
    ) -> Result<()> {
        files::save_meeting(self, meeting, transcript, summary)
    }

    /// Atomically save metadata-only changes without rebuilding transcript chunks or vectors.
    pub fn save_meeting_metadata(&self, meeting: &Meeting) -> Result<()> {
        files::save_meeting_metadata(self, meeting)
    }

    /// Rebuild all indexed records from meeting folders in the library.
    pub fn scan(&self) -> Result<usize> {
        files::scan(self)
    }

    /// Read a meeting and its transcript/summary from the source-of-truth files.
    pub fn get_meeting(&self, meeting_id: MeetingId) -> Result<MeetingRecord> {
        files::get_meeting(self, meeting_id)
    }

    /// Permanently delete a meeting folder and its indexed records.
    pub fn delete_meeting(&self, meeting_id: MeetingId) -> Result<()> {
        files::delete_meeting(self, meeting_id)
    }

    /// List meetings newest first for day-grouped library views.
    pub fn list_meetings(&self, limit: usize, offset: usize) -> Result<Vec<MeetingListItem>> {
        self.list_meetings_filtered(limit, offset, None, false)
    }

    /// List meetings with optional folder and favourite-only filters.
    pub fn list_meetings_filtered(
        &self,
        limit: usize,
        offset: usize,
        folder_id: Option<&str>,
        favorites_only: bool,
    ) -> Result<Vec<MeetingListItem>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id, title, started_at, duration_ms, language, folder, status_json, asr_model, updated_at, favorite, folder_id
             FROM meetings WHERE (?1 IS NULL OR folder_id=?1) AND (?2=0 OR favorite=1)
             ORDER BY started_at DESC, id DESC LIMIT ?3 OFFSET ?4",
        ).map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![
                    folder_id,
                    i64::from(favorites_only),
                    limit as i64,
                    offset as i64
                ],
                |row| {
                    let status_json: String = row.get(6)?;
                    let status =
                        serde_json::from_str(&status_json).map_err(to_sql_conversion_error)?;
                    Ok(MeetingListItem {
                        id: parse_meeting_id_sql(row.get(0)?, 0)?,
                        title: row.get(1)?,
                        started_at: row.get(2)?,
                        duration_ms: row.get::<_, i64>(3)?.max(0) as u64,
                        language: row.get(4)?,
                        folder: row.get(5)?,
                        status,
                        asr_model: row.get(7)?,
                        updated_at: row.get(8)?,
                        favorite: row.get::<_, i64>(9)? != 0,
                        folder_id: row.get(10)?,
                    })
                },
            )
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    /// List tasks, optionally limiting results to tasks not yet marked done.
    pub fn list_tasks(&self, open_only: bool) -> Result<Vec<TaskRecord>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT t.meeting_id, m.title, t.idx, t.text, t.owner, t.due, t.at_ms, t.done, m.folder_id, m.favorite
             FROM tasks t JOIN meetings m ON m.id=t.meeting_id
             WHERE (?1 = 0 OR t.done=0) ORDER BY m.started_at DESC, t.idx ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![i64::from(open_only)], |row| {
                Ok(TaskRecord {
                    meeting_id: parse_meeting_id_sql(row.get(0)?, 0)?,
                    meeting_title: row.get(1)?,
                    idx: row.get::<_, i64>(2)?.max(0) as u32,
                    text: row.get(3)?,
                    owner: row.get(4)?,
                    due: row.get(5)?,
                    at_ms: row
                        .get::<_, Option<i64>>(6)?
                        .map(|value| value.max(0) as u64),
                    done: row.get::<_, i64>(7)? != 0,
                    meeting_folder_id: row.get(8)?,
                    meeting_favorite: row.get::<_, i64>(9)? != 0,
                })
            })
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    /// Mark a task complete in summary.json and refresh its database mirror.
    pub fn set_task_done(&self, meeting_id: MeetingId, task_index: u32, done: bool) -> Result<()> {
        let mut record = self.get_meeting(meeting_id)?;
        let summary = record
            .summary
            .as_mut()
            .ok_or_else(|| Error::NotFound(format!("summary for meeting {meeting_id}")))?;
        let task = summary.tasks.get_mut(task_index as usize).ok_or_else(|| {
            Error::NotFound(format!("task {task_index} for meeting {meeting_id}"))
        })?;
        task.done = done;
        self.save_meeting(&record.meeting, &record.transcript, record.summary.as_ref())
    }

    /// Run hybrid BM25 and cosine-vector search, fusing rankings with reciprocal-rank fusion.
    pub fn search(
        &self,
        query_text: &str,
        query_vector: Option<&[f32]>,
        filters: &SearchFilters,
        limit: usize,
    ) -> Result<Vec<Hit>> {
        search::search(self, query_text, query_vector, filters, limit)
    }

    /// Set the active vector model and dimensions, recreating vec0 when either changes.
    pub fn configure_embedding_model(&self, model_id: &str, dimensions: usize) -> Result<()> {
        if model_id.trim().is_empty() || dimensions == 0 {
            return Err(Error::Invalid(
                "embedding model id and dimensions must be non-empty".to_owned(),
            ));
        }
        let mut connection = self.lock()?;
        db::configure_embedding_model(&mut connection, model_id, dimensions)
    }

    /// Return the active indexed embedding model and dimensions without changing the index.
    pub fn active_embedding_model(&self) -> Result<Option<(String, usize)>> {
        let connection = self.lock()?;
        let model = connection
            .query_row(
                "SELECT value FROM meta WHERE key='embedding_model_id'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?;
        let dimensions = connection
            .query_row(
                "SELECT value FROM meta WHERE key='embedding_dims'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?;
        match (model, dimensions) {
            (Some(model), Some(dimensions)) => {
                let dimensions = dimensions.parse::<usize>().map_err(|error| {
                    Error::Storage(format!("invalid embedding dimension metadata: {error}"))
                })?;
                Ok(Some((model, dimensions)))
            }
            _ => Ok(None),
        }
    }

    /// Store one embedding per indexed chunk for a meeting, replacing its previous vectors.
    pub fn set_chunk_embeddings(
        &self,
        meeting_id: MeetingId,
        model_id: &str,
        embeddings: &[Vec<f32>],
    ) -> Result<()> {
        search::set_chunk_embeddings(self, meeting_id, model_id, embeddings)
    }

    /// Add a queued background job and return its database id.
    pub fn enqueue_job(&self, meeting_id: MeetingId, kind: &str, payload: &Value) -> Result<i64> {
        jobs::enqueue(self, meeting_id, kind, payload)
    }

    /// Claim the oldest queued job matching one of the requested kinds.
    pub fn claim_next(&self, kinds: &[&str]) -> Result<Option<Job>> {
        jobs::claim_next(self, kinds)
    }

    /// Mark a running job as successfully completed.
    pub fn complete(&self, job_id: i64) -> Result<()> {
        jobs::complete(self, job_id)
    }

    /// Mark a running job failed, or return it to the queue when the failure is retryable.
    pub fn fail(&self, job_id: i64, error: &str, retryable: bool) -> Result<()> {
        jobs::fail(self, job_id, error, retryable)
    }

    /// Requeue any jobs left in the running state by a previous process.
    pub fn reset_running_on_startup(&self) -> Result<usize> {
        jobs::reset_running_on_startup(self)
    }

    /// Read a JSON setting by key.
    pub fn get_setting(&self, key: &str) -> Result<Option<Value>> {
        let connection = self.lock()?;
        let value = connection
            .query_row(
                "SELECT value_json FROM settings WHERE key=?1",
                [key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?;
        value
            .map(|json| serde_json::from_str(&json).map_err(json_error))
            .transpose()
    }

    /// Store a JSON setting by key.
    pub fn set_setting(&self, key: &str, value: &Value) -> Result<()> {
        let json = serde_json::to_string(value).map_err(json_error)?;
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO settings(key, value_json) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",
            params![key, json],
        ).map_err(storage_error)?;
        Ok(())
    }

    /// Insert or update a known voice centroid.
    pub fn upsert_voice(&self, voice: &StoreVoice) -> Result<()> {
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO voices(id, name, centroid, samples, updated_at) VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET name=excluded.name, centroid=excluded.centroid, samples=excluded.samples, updated_at=excluded.updated_at",
            params![voice.id, voice.name, voice.centroid, voice.samples as i64, voice.updated_at],
        ).map_err(storage_error)?;
        Ok(())
    }

    /// Return known voice centroids ordered by display name.
    pub fn list_voices(&self) -> Result<Vec<StoreVoice>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT id, name, centroid, samples, updated_at FROM voices ORDER BY name")
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(StoreVoice {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    centroid: row.get(2)?,
                    samples: row.get::<_, i64>(3)?.max(0) as u64,
                    updated_at: row.get(4)?,
                })
            })
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub(crate) fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| Error::Storage("database mutex was poisoned".to_owned()))
    }

    pub(crate) fn root(&self) -> &Path {
        &self.library_root
    }

    pub(crate) fn indexed_folder(&self, meeting_id: MeetingId) -> Result<Option<String>> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT folder FROM meetings WHERE id=?1",
                [meeting_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)
    }
}

fn register_sqlite_vec() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| unsafe {
        type EntryPoint = unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut std::ffi::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> std::ffi::c_int;
        let entry_point =
            std::mem::transmute::<*const (), EntryPoint>(sqlite_vec::sqlite3_vec_init as *const ());
        rusqlite::ffi::sqlite3_auto_extension(Some(entry_point));
    });
}

pub(crate) fn storage_error(error: rusqlite::Error) -> Error {
    Error::Storage(error.to_string())
}

pub(crate) fn json_error(error: serde_json::Error) -> Error {
    Error::Storage(error.to_string())
}

pub(crate) fn to_sql_conversion_error(error: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

pub(crate) fn parse_meeting_id_sql(value: String, column: usize) -> rusqlite::Result<MeetingId> {
    value.parse().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}
