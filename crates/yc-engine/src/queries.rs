//! Reading and editing the library; search and question answering.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use yc_core::meeting::{Meeting, MeetingId, TrackKind};
use yc_core::summary::Summary;
use yc_core::transcript::{Segment, Transcript, Turn};
use yc_core::{Error, Result};
use yc_store::{Hit, MeetingListItem, SearchFilters, TaskRecord};
use yc_text::{build_turns, copy_for_claude, render_summary_markdown, render_transcript_markdown};

use crate::engine::Engine;
use crate::finalize::audio_dir;
use crate::ports::{AnswerEvent, AnswerRequest, Excerpt};
use crate::stages::speaker_names;

/// Everything the meeting screen needs.
#[derive(Debug, Clone, PartialEq)]
pub struct MeetingDetail {
    pub meeting: Meeting,
    pub segments: Vec<Segment>,
    pub turns: Vec<Turn>,
    pub summary: Option<Summary>,
    /// Folder name inside the library.
    pub folder: String,
    /// Tracks whose audio is still on disk (for playback).
    pub audio: Vec<TrackKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    Markdown,
    Json,
}

/// Number of excerpts handed to the answerer.
const ASK_TOP_K: usize = 8;
/// Transcript context added on each side of a hit.
const CONTEXT_MS: u64 = 15_000;

impl Engine {
    /// Newest first.
    pub async fn list_meetings(
        self: &Arc<Self>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<MeetingListItem>> {
        self.blocking(move |e| e.store.list_meetings(limit, offset))
            .await
    }

    pub async fn get_meeting(self: &Arc<Self>, id: MeetingId) -> Result<MeetingDetail> {
        self.blocking(move |e| {
            let rec = e.store.get_meeting(id)?;
            let audio_path = audio_dir(&e.library_root(), &rec.folder);
            let audio = TrackKind::ALL
                .into_iter()
                .filter(|k| {
                    audio_path.join(format!("{}.pcm", k.file_stem())).is_file()
                        || audio_path.join(format!("{}.flac", k.file_stem())).is_file()
                })
                .collect();
            Ok(MeetingDetail {
                turns: build_turns(&rec.transcript.segments),
                segments: rec.transcript.segments,
                meeting: rec.meeting,
                summary: rec.summary,
                folder: rec.folder,
                audio,
            })
        })
        .await
    }

    /// Renames the meeting (and its folder).
    pub async fn rename_meeting(self: &Arc<Self>, id: MeetingId, title: String) -> Result<()> {
        let title = title.trim().to_owned();
        if title.is_empty() {
            return Err(Error::Invalid("a title cannot be empty".into()));
        }
        self.blocking(move |e| {
            e.mutate_meeting(id, |rec| {
                rec.meeting.title = title;
                Ok(((), true))
            })
        })
        .await?;
        self.schedule_reindex(id);
        Ok(())
    }

    /// Permanently deletes the meeting folder, its audio and its index rows.
    pub async fn delete_meeting(self: &Arc<Self>, id: MeetingId) -> Result<()> {
        self.blocking(move |e| {
            let _w = e
                .write_lock
                .lock()
                .map_err(|_| Error::Storage("write lock poisoned".into()))?;
            let _ = std::fs::remove_dir_all(e.recording_root().join(id.to_string()));
            e.store.delete_meeting(id)
        })
        .await
    }

    pub async fn set_task_done(
        self: &Arc<Self>,
        id: MeetingId,
        index: u32,
        done: bool,
    ) -> Result<()> {
        self.blocking(move |e| {
            let _w = e
                .write_lock
                .lock()
                .map_err(|_| Error::Storage("write lock poisoned".into()))?;
            e.store.set_task_done(id, index, done)
        })
        .await?;
        self.schedule_reindex(id);
        Ok(())
    }

    /// Tasks across all meetings, newest meeting first.
    pub async fn list_tasks(self: &Arc<Self>, open_only: bool) -> Result<Vec<TaskRecord>> {
        self.blocking(move |e| e.store.list_tasks(open_only)).await
    }

    /// Hybrid search (full text + semantic when an embedding model is installed).
    pub async fn search(
        self: &Arc<Self>,
        query: String,
        filters: SearchFilters,
        limit: usize,
    ) -> Result<Vec<Hit>> {
        self.blocking(move |e| e.search_blocking(&query, &filters, limit))
            .await
    }

    fn search_blocking(
        &self,
        query: &str,
        filters: &SearchFilters,
        limit: usize,
    ) -> Result<Vec<Hit>> {
        let vector = if query.trim().is_empty() {
            None
        } else {
            match self.with_text_embedder(|emb| {
                Ok(emb
                    .embed(&[query], yc_core::ports::EmbedKind::Query)?
                    .into_iter()
                    .next())
            }) {
                Ok(v) => v.flatten(),
                Err(e) => {
                    tracing::warn!("query embedding failed, searching by text only: {e}");
                    None
                }
            }
        };
        if let Some(v) = &vector {
            match self.store.search(query, Some(v), filters, limit) {
                Ok(hits) => return Ok(hits),
                Err(e) => tracing::warn!("semantic search failed, falling back to text: {e}"),
            }
        }
        self.store.search(query, None, filters, limit)
    }

    /// Answers a question over the library: retrieves the best passages and streams the
    /// [`Answerer`](crate::ports::Answerer)'s reply, including its egress note and citations.
    pub async fn ask(self: &Arc<Self>, question: String) -> Result<mpsc::Receiver<AnswerEvent>> {
        let answerer = self
            .ports
            .answerer
            .clone()
            .ok_or_else(|| Error::Invalid("no answering provider is configured".into()))?;
        let language = self.settings().ui_language;
        let q = question.clone();
        let excerpts = self
            .blocking(move |e| {
                let hits = e.search_blocking(&q, &SearchFilters::default(), ASK_TOP_K)?;
                Ok(e.excerpts_for(&hits))
            })
            .await?;
        let (tx, rx) = mpsc::channel(64);
        let request = AnswerRequest {
            question,
            language,
            excerpts,
        };
        tokio::spawn(async move {
            if let Err(e) = answerer.answer(request, tx.clone()).await {
                let _ = tx.send(AnswerEvent::Failed(e.to_string())).await;
            }
        });
        Ok(rx)
    }

    /// Turns hits into passages with some surrounding dialogue.
    fn excerpts_for(&self, hits: &[Hit]) -> Vec<Excerpt> {
        let mut cache: HashMap<MeetingId, (Meeting, Transcript)> = HashMap::new();
        hits.iter()
            .map(|hit| {
                if !cache.contains_key(&hit.meeting_id)
                    && let Ok(rec) = self.store.get_meeting(hit.meeting_id)
                {
                    cache.insert(hit.meeting_id, (rec.meeting, rec.transcript));
                }
                let text = cache.get(&hit.meeting_id).map_or_else(
                    || hit.snippet.replace("**", ""),
                    |(meeting, transcript)| {
                        let names: HashMap<String, String> =
                            speaker_names(meeting).into_iter().collect();
                        let lines: Vec<String> = transcript
                            .segments
                            .iter()
                            .filter(|s| {
                                s.end_ms + CONTEXT_MS >= hit.start_ms
                                    && s.start_ms <= hit.end_ms + CONTEXT_MS
                            })
                            .map(|s| {
                                let who = names.get(&s.speaker).unwrap_or(&s.speaker);
                                format!("{who}: {}", s.text)
                            })
                            .collect();
                        if lines.is_empty() {
                            hit.snippet.replace("**", "")
                        } else {
                            lines.join("\n")
                        }
                    },
                );
                Excerpt {
                    meeting_id: hit.meeting_id,
                    meeting_title: hit.meeting_title.clone(),
                    started_at: hit.started_at.clone(),
                    start_ms: hit.start_ms,
                    end_ms: hit.end_ms,
                    speakers: hit.speakers.clone(),
                    text,
                }
            })
            .collect()
    }

    /// The meeting as one document: summary (if any) and transcript.
    pub async fn export(self: &Arc<Self>, id: MeetingId, format: ExportFormat) -> Result<String> {
        self.blocking(move |e| {
            let rec = e.store.get_meeting(id)?;
            match format {
                ExportFormat::Markdown => {
                    let mut out = String::new();
                    if let Some(s) = &rec.summary {
                        out.push_str(&render_summary_markdown(s));
                        out.push('\n');
                    }
                    out.push_str(&render_transcript_markdown(
                        &rec.meeting,
                        &rec.transcript.segments,
                    ));
                    Ok(out)
                }
                ExportFormat::Json => serde_json::to_string_pretty(&serde_json::json!({
                    "meeting": rec.meeting,
                    "transcript": rec.transcript,
                    "summary": rec.summary,
                }))
                .map_err(|e| Error::Storage(e.to_string())),
            }
        })
        .await
    }

    /// Transcript with an instruction line, ready to paste into Claude.
    pub async fn copy_for_claude(self: &Arc<Self>, id: MeetingId) -> Result<String> {
        self.blocking(move |e| {
            let rec = e.store.get_meeting(id)?;
            Ok(copy_for_claude(&rec.meeting, &rec.transcript.segments))
        })
        .await
    }
}
