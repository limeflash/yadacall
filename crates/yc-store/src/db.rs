use rusqlite::{Connection, TransactionBehavior};
use yc_core::{Error, Result};

use crate::storage_error;

pub(crate) fn migrate(connection: &mut Connection) -> Result<()> {
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(storage_error)?;
    if version == 0 {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Exclusive)
            .map_err(storage_error)?;
        transaction.execute_batch(
            "CREATE TABLE meetings (
                id TEXT PRIMARY KEY NOT NULL,
                title TEXT NOT NULL,
                started_at TEXT NOT NULL,
                duration_ms INTEGER NOT NULL,
                language TEXT NOT NULL,
                folder TEXT NOT NULL,
                status_json TEXT NOT NULL,
                asr_model TEXT,
                favorite INTEGER NOT NULL DEFAULT 0,
                folder_id TEXT,
                updated_at TEXT NOT NULL
             );
             CREATE TABLE segments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                idx INTEGER NOT NULL,
                start_ms INTEGER NOT NULL,
                end_ms INTEGER NOT NULL,
                speaker TEXT NOT NULL,
                track TEXT NOT NULL,
                text TEXT NOT NULL,
                stemmed_text TEXT NOT NULL,
                UNIQUE(meeting_id, idx)
             );
             CREATE VIRTUAL TABLE segments_fts USING fts5(stemmed_text, tokenize='unicode61');
             CREATE INDEX segments_meeting_start ON segments(meeting_id, start_ms);
             CREATE TABLE chunks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                start_ms INTEGER NOT NULL,
                end_ms INTEGER NOT NULL,
                speakers TEXT NOT NULL,
                text TEXT NOT NULL
             );
             CREATE INDEX chunks_meeting_start ON chunks(meeting_id, start_ms);
             CREATE TABLE tasks (
                meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                idx INTEGER NOT NULL,
                text TEXT NOT NULL,
                owner TEXT,
                due TEXT,
                at_ms INTEGER,
                done INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(meeting_id, idx)
             );
             CREATE TABLE voices (
                id TEXT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL,
                centroid BLOB NOT NULL,
                samples INTEGER NOT NULL,
                updated_at TEXT NOT NULL
             );
             CREATE TABLE jobs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                kind TEXT NOT NULL,
                state TEXT NOT NULL CHECK(state IN ('queued','running','done','failed')),
                attempts INTEGER NOT NULL DEFAULT 0,
                error TEXT,
                payload_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
             );
             CREATE INDEX jobs_queue ON jobs(state, created_at, id);
             CREATE TABLE settings (key TEXT PRIMARY KEY NOT NULL, value_json TEXT NOT NULL);
             CREATE TABLE meta (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);
             INSERT INTO meta(key, value) VALUES('embedding_model_id', 'unconfigured'), ('embedding_dims', '1');
             CREATE VIRTUAL TABLE chunk_vectors USING vec0(chunk_id INTEGER PRIMARY KEY, embedding float[1] distance_metric=cosine);
             PRAGMA user_version=2;",
        ).map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
    } else if version == 1 {
        connection.execute_batch("ALTER TABLE meetings ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0; ALTER TABLE meetings ADD COLUMN folder_id TEXT; PRAGMA user_version=2;").map_err(storage_error)?;
    } else if version != 2 {
        return Err(Error::Storage(format!(
            "unsupported index schema version {version}"
        )));
    }
    Ok(())
}

pub(crate) fn configure_embedding_model(
    connection: &mut Connection,
    model_id: &str,
    dimensions: usize,
) -> Result<()> {
    if dimensions > 65_536 {
        return Err(Error::Invalid(
            "embedding dimension exceeds 65536".to_owned(),
        ));
    }
    let transaction = connection.transaction().map_err(storage_error)?;
    let existing_model: String = transaction
        .query_row(
            "SELECT value FROM meta WHERE key='embedding_model_id'",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    let existing_dimensions: String = transaction
        .query_row(
            "SELECT value FROM meta WHERE key='embedding_dims'",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    if existing_model != model_id || existing_dimensions != dimensions.to_string() {
        transaction
            .execute("DROP TABLE chunk_vectors", [])
            .map_err(storage_error)?;
        transaction.execute_batch(&format!(
            "CREATE VIRTUAL TABLE chunk_vectors USING vec0(chunk_id INTEGER PRIMARY KEY, embedding float[{dimensions}] distance_metric=cosine);"
        )).map_err(storage_error)?;
        transaction
            .execute(
                "UPDATE meta SET value=?1 WHERE key='embedding_model_id'",
                [model_id],
            )
            .map_err(storage_error)?;
        transaction
            .execute(
                "UPDATE meta SET value=?1 WHERE key='embedding_dims'",
                [dimensions.to_string()],
            )
            .map_err(storage_error)?;
    }
    transaction.commit().map_err(storage_error)
}

pub(crate) fn active_embedding_model(connection: &Connection) -> Result<(String, usize)> {
    let model: String = connection
        .query_row(
            "SELECT value FROM meta WHERE key='embedding_model_id'",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    let dimensions: String = connection
        .query_row(
            "SELECT value FROM meta WHERE key='embedding_dims'",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    let dimensions = dimensions.parse::<usize>().map_err(|error| {
        Error::Storage(format!("invalid embedding dimension metadata: {error}"))
    })?;
    Ok((model, dimensions))
}
