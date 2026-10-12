use chrono::Utc;
use rusqlite::{OptionalExtension, TransactionBehavior, params, params_from_iter};
use yc_core::{Error, Result, meeting::MeetingId};

use crate::{
    Store, json_error, storage_error, to_sql_conversion_error,
    types::{Job, JobState},
};

pub(crate) fn enqueue(
    store: &Store,
    meeting_id: MeetingId,
    kind: &str,
    payload: &serde_json::Value,
) -> Result<i64> {
    if kind.trim().is_empty() {
        return Err(Error::Invalid("job kind cannot be empty".to_owned()));
    }
    let payload = serde_json::to_string(payload).map_err(json_error)?;
    let now = Utc::now().to_rfc3339();
    let connection = store.lock()?;
    connection.execute(
        "INSERT INTO jobs(meeting_id,kind,state,attempts,payload_json,created_at,updated_at) VALUES(?1,?2,'queued',0,?3,?4,?4)",
        params![meeting_id.to_string(), kind, payload, now],
    ).map_err(storage_error)?;
    Ok(connection.last_insert_rowid())
}

pub(crate) fn claim_next(store: &Store, kinds: &[&str]) -> Result<Option<Job>> {
    if kinds.is_empty() {
        return Ok(None);
    }
    let placeholders = (1..=kinds.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut connection = store.lock()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    let sql = format!(
        "SELECT id FROM jobs WHERE state='queued' AND kind IN ({placeholders}) ORDER BY created_at,id LIMIT 1"
    );
    let id = transaction
        .query_row(&sql, params_from_iter(kinds.iter()), |row| {
            row.get::<_, i64>(0)
        })
        .optional()
        .map_err(storage_error)?;
    let Some(id) = id else {
        transaction.commit().map_err(storage_error)?;
        return Ok(None);
    };
    let updated_at = Utc::now().to_rfc3339();
    transaction.execute("UPDATE jobs SET state='running', attempts=attempts+1, error=NULL, updated_at=?1 WHERE id=?2 AND state='queued'", params![updated_at, id]).map_err(storage_error)?;
    let job = transaction.query_row(
        "SELECT id,meeting_id,kind,state,attempts,error,payload_json,created_at,updated_at FROM jobs WHERE id=?1",
        [id],
        read_job,
    ).map_err(storage_error)?;
    transaction.commit().map_err(storage_error)?;
    Ok(Some(job))
}

pub(crate) fn complete(store: &Store, job_id: i64) -> Result<()> {
    transition(store, job_id, "done", None)
}

pub(crate) fn fail(store: &Store, job_id: i64, error: &str, retryable: bool) -> Result<()> {
    transition(
        store,
        job_id,
        if retryable { "queued" } else { "failed" },
        Some(error),
    )
}

fn transition(store: &Store, job_id: i64, state: &str, error: Option<&str>) -> Result<()> {
    let connection = store.lock()?;
    let updated = connection
        .execute(
            "UPDATE jobs SET state=?1,error=?2,updated_at=?3 WHERE id=?4 AND state='running'",
            params![state, error, Utc::now().to_rfc3339(), job_id],
        )
        .map_err(storage_error)?;
    if updated == 0 {
        return Err(Error::NotFound(format!("running job {job_id}")));
    }
    Ok(())
}

pub(crate) fn reset_running_on_startup(store: &Store) -> Result<usize> {
    let connection = store.lock()?;
    let updated = connection
        .execute(
            "UPDATE jobs SET state='queued',error=NULL,updated_at=?1 WHERE state='running'",
            [Utc::now().to_rfc3339()],
        )
        .map_err(storage_error)?;
    Ok(updated)
}

fn read_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    let meeting_id: String = row.get(1)?;
    let meeting_id = meeting_id.parse::<MeetingId>().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let state: String = row.get(3)?;
    let payload: String = row.get(6)?;
    Ok(Job {
        id: row.get(0)?,
        meeting_id,
        kind: row.get(2)?,
        state: JobState::parse(&state)?,
        attempts: row.get::<_, i64>(4)?.max(0) as u32,
        error: row.get(5)?,
        payload: serde_json::from_str(&payload).map_err(to_sql_conversion_error)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}
