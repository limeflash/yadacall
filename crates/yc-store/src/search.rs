use rusqlite::{named_params, params};
use yc_core::{Error, Result, meeting::MeetingId};
use yc_text::stem_text;

use crate::{
    Store, db, parse_meeting_id_sql, storage_error, to_sql_conversion_error,
    types::{Hit, SearchFilters},
};

const RRF_K: f64 = 60.0;

pub(crate) fn search(
    store: &Store,
    query_text: &str,
    query_vector: Option<&[f32]>,
    filters: &SearchFilters,
    limit: usize,
) -> Result<Vec<Hit>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let connection = store.lock()?;
    let query_tokens = stem_text(query_text)
        .split_whitespace()
        .map(|word| format!("\"{word}\""))
        .collect::<Vec<_>>();
    let query_expression = query_tokens.join(" AND ");
    let mut fused = Vec::<Hit>::new();

    if !query_expression.is_empty() {
        let mut statement = connection.prepare(
            "SELECT s.meeting_id,m.title,m.started_at,s.idx,s.start_ms,s.end_ms,s.speaker,s.text,bm25(segments_fts)
             FROM segments_fts JOIN segments s ON s.id=segments_fts.rowid
             JOIN meetings m ON m.id=s.meeting_id
             WHERE segments_fts MATCH :query
               AND (:meeting_id IS NULL OR s.meeting_id=:meeting_id)
               AND (:language IS NULL OR m.language=:language)
               AND (:speaker IS NULL OR s.speaker=:speaker)
               AND (:started_after IS NULL OR m.started_at>=:started_after)
               AND (:started_before IS NULL OR m.started_at<=:started_before)
             ORDER BY bm25(segments_fts) ASC LIMIT :limit",
        ).map_err(storage_error)?;
        let rows = statement
            .query_map(
                named_params! {
                    ":query": query_expression,
                    ":meeting_id": filters.meeting_id.map(|id| id.to_string()),
                    ":language": filters.language.as_deref(),
                    ":speaker": filters.speaker.as_deref(),
                    ":started_after": filters.started_after.as_deref(),
                    ":started_before": filters.started_before.as_deref(),
                    ":limit": limit.saturating_mul(4) as i64,
                },
                |row| {
                    let meeting_id = parse_meeting_id_sql(row.get::<_, String>(0)?, 0)?;
                    let start_ms = row.get::<_, i64>(4)?.max(0) as u64;
                    let end_ms = row.get::<_, i64>(5)?.max(0) as u64;
                    let text: String = row.get(7)?;
                    Ok((
                        Hit {
                            meeting_id,
                            meeting_title: row.get(1)?,
                            started_at: row.get(2)?,
                            start_ms,
                            end_ms,
                            speakers: vec![row.get(6)?],
                            snippet: highlight_snippet(&text, &query_tokens),
                            score: 0.0,
                            chunk_id: None,
                            segment_idx: Some(row.get::<_, i64>(3)?.max(0) as u32),
                        },
                        row.get::<_, f64>(8)?,
                    ))
                },
            )
            .map_err(storage_error)?;
        for (rank, row) in rows.enumerate() {
            let (mut hit, _bm25) = row.map_err(storage_error)?;
            hit.score += reciprocal_rank(rank + 1);
            fused.push(hit);
        }
    }

    if let Some(query_vector) = query_vector {
        if query_vector.iter().any(|value| !value.is_finite()) {
            return Err(Error::Invalid(
                "query vector contains a non-finite value".to_owned(),
            ));
        }
        let (model, dimensions) = db::active_embedding_model(&connection)?;
        if query_vector.len() != dimensions {
            return Err(Error::Invalid(format!(
                "query vector has {} dimensions; active model `{model}` requires {dimensions}",
                query_vector.len()
            )));
        }
        let vector_bytes = vector_to_bytes(query_vector);
        let mut statement = connection.prepare(
            "SELECT c.id,c.meeting_id,m.title,m.started_at,c.start_ms,c.end_ms,c.speakers,c.text,v.distance
             FROM chunk_vectors v JOIN chunks c ON c.id=v.chunk_id
             JOIN meetings m ON m.id=c.meeting_id
             WHERE v.embedding MATCH ?1 AND k=?2
               AND (?3 IS NULL OR c.meeting_id=?3)
               AND (?4 IS NULL OR m.language=?4)
               AND (?5 IS NULL OR EXISTS (
                    SELECT 1 FROM json_each(c.speakers) WHERE json_each.value=?5
               ))
               AND (?6 IS NULL OR m.started_at>=?6)
               AND (?7 IS NULL OR m.started_at<=?7)
             ORDER BY v.distance ASC",
        ).map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![
                    vector_bytes,
                    limit.saturating_mul(4) as i64,
                    filters.meeting_id.map(|id| id.to_string()),
                    filters.language.as_deref(),
                    filters.speaker.as_deref(),
                    filters.started_after.as_deref(),
                    filters.started_before.as_deref(),
                ],
                |row| {
                    let meeting_id = parse_meeting_id_sql(row.get::<_, String>(1)?, 1)?;
                    let speakers_json: String = row.get(6)?;
                    let speakers =
                        serde_json::from_str(&speakers_json).map_err(to_sql_conversion_error)?;
                    let text: String = row.get(7)?;
                    Ok((
                        Hit {
                            meeting_id,
                            meeting_title: row.get(2)?,
                            started_at: row.get(3)?,
                            start_ms: row.get::<_, i64>(4)?.max(0) as u64,
                            end_ms: row.get::<_, i64>(5)?.max(0) as u64,
                            speakers,
                            snippet: highlight_snippet(&text, &query_tokens),
                            score: 0.0,
                            chunk_id: Some(row.get(0)?),
                            segment_idx: None,
                        },
                        row.get::<_, f64>(8)?,
                    ))
                },
            )
            .map_err(storage_error)?;
        for (rank, row) in rows.enumerate() {
            let (mut hit, _distance) = row.map_err(storage_error)?;
            hit.score += reciprocal_rank(rank + 1);
            fuse_vector_hit(&mut fused, hit);
        }
    }

    fused.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.started_at.cmp(&right.started_at))
    });
    fused.truncate(limit);
    Ok(fused)
}

fn fuse_vector_hit(results: &mut Vec<Hit>, mut hit: Hit) {
    if let Some(existing) = results.iter_mut().find(|existing| {
        existing.chunk_id.is_none()
            && existing.segment_idx.is_some()
            && existing.meeting_id == hit.meeting_id
            && existing.start_ms <= hit.end_ms
            && hit.start_ms <= existing.end_ms
    }) {
        existing.score += hit.score;
        existing.start_ms = existing.start_ms.min(hit.start_ms);
        existing.end_ms = existing.end_ms.max(hit.end_ms);
        for speaker in hit.speakers.drain(..) {
            if !existing.speakers.contains(&speaker) {
                existing.speakers.push(speaker);
            }
        }
        if existing.chunk_id.is_none() {
            existing.chunk_id = hit.chunk_id;
        }
        if existing.segment_idx.is_none() {
            existing.segment_idx = hit.segment_idx;
        }
        if hit.snippet.contains("**") && !existing.snippet.contains("**") {
            existing.snippet = hit.snippet;
        }
    } else {
        results.push(hit);
    }
}

pub(crate) fn set_chunk_embeddings(
    store: &Store,
    meeting_id: MeetingId,
    model_id: &str,
    embeddings: &[Vec<f32>],
) -> Result<()> {
    if model_id.trim().is_empty() || embeddings.is_empty() {
        return Err(Error::Invalid(
            "model id and at least one chunk embedding are required".to_owned(),
        ));
    }
    let dimensions = embeddings[0].len();
    if dimensions == 0
        || embeddings.iter().any(|embedding| {
            embedding.len() != dimensions || embedding.iter().any(|value| !value.is_finite())
        })
    {
        return Err(Error::Invalid(
            "chunk embeddings must be finite vectors of one non-zero dimension".to_owned(),
        ));
    }
    let mut connection = store.lock()?;
    db::configure_embedding_model(&mut connection, model_id, dimensions)?;
    let transaction = connection.transaction().map_err(storage_error)?;
    let mut statement = transaction
        .prepare("SELECT id FROM chunks WHERE meeting_id=?1 ORDER BY start_ms,id")
        .map_err(storage_error)?;
    let chunk_ids = statement
        .query_map([meeting_id.to_string()], |row| row.get::<_, i64>(0))
        .map_err(storage_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(storage_error)?;
    drop(statement);
    if chunk_ids.len() != embeddings.len() {
        return Err(Error::Invalid(format!(
            "meeting has {} chunks but {} embeddings were provided",
            chunk_ids.len(),
            embeddings.len()
        )));
    }
    transaction
        .execute(
            "DELETE FROM chunk_vectors WHERE chunk_id IN (SELECT id FROM chunks WHERE meeting_id=?1)",
            [meeting_id.to_string()],
        )
        .map_err(storage_error)?;
    for (chunk_id, embedding) in chunk_ids.into_iter().zip(embeddings) {
        transaction
            .execute(
                "INSERT INTO chunk_vectors(chunk_id,embedding) VALUES(?1,?2)",
                params![chunk_id, vector_to_bytes(embedding)],
            )
            .map_err(storage_error)?;
    }
    transaction.commit().map_err(storage_error)
}

fn reciprocal_rank(rank: usize) -> f64 {
    1.0 / (RRF_K + rank as f64)
}

fn vector_to_bytes(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn highlight_snippet(text: &str, stemmed_query: &[String]) -> String {
    let query_stems = stemmed_query
        .iter()
        .map(|term| term.trim_matches('"').to_owned())
        .collect::<std::collections::HashSet<_>>();
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    let chars = text.char_indices().collect::<Vec<_>>();
    for (index, character) in chars.iter().enumerate() {
        if !character.1.is_alphanumeric() {
            continue;
        }
        let start = character.0;
        let mut end = start + character.1.len_utf8();
        let mut next_index = index + 1;
        while next_index < chars.len() && chars[next_index].1.is_alphanumeric() {
            end = chars[next_index].0 + chars[next_index].1.len_utf8();
            next_index += 1;
        }
        if start < cursor {
            continue;
        }
        output.push_str(&text[cursor..start]);
        let token = &text[start..end];
        if query_stems.contains(&stem_text(token)) {
            output.push_str("**");
            output.push_str(token);
            output.push_str("**");
        } else {
            output.push_str(token);
        }
        cursor = end;
    }
    output.push_str(&text[cursor..]);
    let chars = output.chars().collect::<Vec<_>>();
    if chars.len() > 240 {
        format!("{}…", chars.into_iter().take(237).collect::<String>())
    } else {
        output
    }
}
