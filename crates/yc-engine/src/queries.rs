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

fn safe_meeting_relative(path: &str) -> bool {
    let path = std::path::Path::new(path);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn clean_bookmark_label(label: Option<String>) -> Result<Option<String>> {
    let label = label
        .map(|label| label.trim().to_owned())
        .filter(|label| !label.is_empty());
    if label
        .as_ref()
        .is_some_and(|label| label.chars().count() > 120)
    {
        return Err(Error::Invalid(
            "bookmark labels must be 120 characters or fewer".into(),
        ));
    }
    Ok(label)
}

fn ensure_finished(status: &yc_core::meeting::MeetingStatus) -> Result<()> {
    if matches!(
        status,
        yc_core::meeting::MeetingStatus::Recording
            | yc_core::meeting::MeetingStatus::Processing { .. }
    ) {
        return Err(Error::Invalid(
            "bookmarks can only be edited after recording finishes".into(),
        ));
    }
    Ok(())
}

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

    /// List meetings with optional folder and favourites-only filters.
    pub async fn list_meetings_filtered(
        self: &Arc<Self>,
        limit: usize,
        offset: usize,
        folder_id: Option<String>,
        favorites_only: bool,
    ) -> Result<Vec<MeetingListItem>> {
        self.blocking(move |e| {
            e.store
                .list_meetings_filtered(limit, offset, folder_id.as_deref(), favorites_only)
        })
        .await
    }

    /// Folder CRUD stored in the library root's `folders.json`.
    pub async fn list_folders(self: &Arc<Self>) -> Result<Vec<yc_core::meeting::Folder>> {
        self.blocking(|e| e.store.list_folders()).await
    }
    /// Create a library folder.
    pub async fn create_folder(
        self: &Arc<Self>,
        name: String,
        color: Option<String>,
    ) -> Result<yc_core::meeting::Folder> {
        self.blocking(move |e| e.store.create_folder(name, color))
            .await
    }
    /// Update selected folder fields.
    pub async fn update_folder(
        self: &Arc<Self>,
        id: String,
        name: Option<String>,
        color: Option<Option<String>>,
        order: Option<u32>,
    ) -> Result<yc_core::meeting::Folder> {
        self.blocking(move |e| e.store.update_folder(&id, name, color, order))
            .await
    }
    /// Delete a folder and unassign its meetings.
    pub async fn delete_folder(self: &Arc<Self>, id: String) -> Result<()> {
        self.blocking(move |e| {
            let meetings = e
                .store
                .list_meetings_filtered(100_000, 0, Some(&id), false)?;
            for item in meetings {
                e.mutate_meeting_metadata(item.id, |record| {
                    record.meeting.folder_id = None;
                    Ok(((), true))
                })?;
            }
            e.store.delete_folder(&id)
        })
        .await
    }

    /// Assign a meeting to a folder, or clear its assignment.
    pub async fn set_meeting_folder(
        self: &Arc<Self>,
        id: MeetingId,
        folder_id: Option<String>,
    ) -> Result<()> {
        self.blocking(move |e| {
            if let Some(folder_id) = &folder_id
                && !e.store.list_folders()?.iter().any(|f| f.id == *folder_id)
            {
                return Err(Error::NotFound(format!("folder {folder_id}")));
            }
            e.mutate_meeting_metadata(id, |r| {
                r.meeting.folder_id = folder_id;
                Ok(((), true))
            })?;
            Ok(())
        })
        .await?;
        Ok(())
    }

    /// Toggle whether a meeting is exempt from audio retention.
    pub async fn set_meeting_favorite(
        self: &Arc<Self>,
        id: MeetingId,
        favorite: bool,
    ) -> Result<()> {
        self.blocking(move |e| {
            e.mutate_meeting_metadata(id, |r| {
                r.meeting.favorite = favorite;
                Ok(((), true))
            })?;
            Ok(())
        })
        .await?;
        Ok(())
    }

    /// Add a post-recording bookmark, clamped to the meeting duration.
    pub async fn add_bookmark(
        self: &Arc<Self>,
        id: MeetingId,
        at_ms: u64,
        label: Option<String>,
    ) -> Result<yc_core::meeting::Bookmark> {
        let bookmark = self
            .blocking(move |e| {
                e.mutate_meeting_metadata(id, |r| {
                    ensure_finished(&r.meeting.status)?;
                    let label = clean_bookmark_label(label)?;
                    let bookmark = yc_core::meeting::Bookmark {
                        id: uuid::Uuid::now_v7().to_string(),
                        at_ms: at_ms.min(r.meeting.duration_ms),
                        label,
                        created_after: Some(true),
                        screenshot: None,
                    };
                    r.meeting.bookmarks.push(bookmark.clone());
                    r.meeting.bookmarks.sort_by_key(|b| b.at_ms);
                    Ok((bookmark, true))
                })
            })
            .await?;
        Ok(bookmark)
    }

    /// Update or clear a bookmark label.
    pub async fn update_bookmark(
        self: &Arc<Self>,
        id: MeetingId,
        bookmark_id: String,
        label: Option<String>,
    ) -> Result<yc_core::meeting::Bookmark> {
        let bookmark = self
            .blocking(move |e| {
                e.mutate_meeting_metadata(id, |r| {
                    ensure_finished(&r.meeting.status)?;
                    let label = clean_bookmark_label(label)?;
                    let bookmark = r
                        .meeting
                        .bookmarks
                        .iter_mut()
                        .find(|b| b.id == bookmark_id)
                        .ok_or_else(|| Error::NotFound(format!("bookmark {bookmark_id}")))?;
                    bookmark.label = label;
                    Ok((bookmark.clone(), true))
                })
            })
            .await?;
        Ok(bookmark)
    }

    /// Remove a bookmark from a meeting.
    pub async fn remove_bookmark(
        self: &Arc<Self>,
        id: MeetingId,
        bookmark_id: String,
    ) -> Result<()> {
        let screenshot = self
            .blocking(move |e| {
                let mut removed = None;
                let mut folder = None;
                e.mutate_meeting_metadata(id, |r| {
                    ensure_finished(&r.meeting.status)?;
                    folder = Some(r.folder.clone());
                    let n = r.meeting.bookmarks.len();
                    removed = r
                        .meeting
                        .bookmarks
                        .iter()
                        .find(|b| b.id == bookmark_id)
                        .and_then(|b| b.screenshot.clone());
                    r.meeting.bookmarks.retain(|b| b.id != bookmark_id);
                    if n == r.meeting.bookmarks.len() {
                        return Err(Error::NotFound(format!("bookmark {bookmark_id}")));
                    }
                    Ok(((), true))
                })?;
                Ok((folder, removed))
            })
            .await?;
        if let (Some(folder), Some(path)) = screenshot
            && safe_meeting_relative(&path)
        {
            match std::fs::remove_file(self.library_root().join(folder).join(path)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    /// Attaches a screenshot to a live bookmark, whether recording is still active or just finalized.
    pub async fn attach_bookmark_screenshot(
        self: &Arc<Self>,
        id: MeetingId,
        bookmark_id: String,
        bytes: Vec<u8>,
    ) -> Result<yc_core::meeting::Bookmark> {
        let live = {
            let mut active = self
                .active
                .lock()
                .map_err(|_| Error::Storage("active recording lock poisoned".into()))?;
            match active.as_mut().filter(|s| s.id == id) {
                Some(session) => Some(session.attach_bookmark_screenshot(&bookmark_id, &bytes)?),
                None => None,
            }
        };
        let bookmark = if let Some(bookmark) = live {
            bookmark
        } else {
            self.blocking(move |e| {
                e.mutate_meeting_metadata(id, |record| {
                    let folder = record.folder.clone();
                    let bookmark = record
                        .meeting
                        .bookmarks
                        .iter_mut()
                        .find(|b| b.id == bookmark_id)
                        .ok_or_else(|| Error::NotFound(format!("bookmark {bookmark_id}")))?;
                    let relative = format!("shots/{}.jpg", bookmark.at_ms);
                    let path = e.library_root().join(folder).join(&relative);
                    std::fs::create_dir_all(
                        path.parent()
                            .ok_or_else(|| Error::Storage("invalid screenshot path".into()))?,
                    )?;
                    crate::meta::write_atomic(&path, &bytes)?;
                    bookmark.screenshot = Some(relative);
                    Ok((bookmark.clone(), true))
                })
            })
            .await?
        };
        self.emit(yc_core::events::EngineEvent::BookmarkUpdated {
            meeting: id,
            bookmark: bookmark.clone(),
        });
        Ok(bookmark)
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
                rec.meeting.title_is_auto = false;
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
        Ok(self.ask_with_excerpts(question, None).await?.1)
    }

    /// Like [`Engine::ask`], and also returns the excerpts handed to the answerer, so a caller can
    /// resolve the indices of [`AnswerEvent::Citations`] into meetings and quotes.
    pub async fn ask_with_excerpts(
        self: &Arc<Self>,
        question: String,
        folder_id: Option<String>,
    ) -> Result<(Vec<Excerpt>, mpsc::Receiver<AnswerEvent>)> {
        let answerer = self
            .ports
            .answerer
            .clone()
            .ok_or_else(|| Error::Invalid("no answering provider is configured".into()))?;
        let language = self.settings().ui_language;
        let q = question.clone();
        let excerpts = self
            .blocking(move |e| {
                let hits = e.search_blocking(
                    &q,
                    &SearchFilters {
                        folder_id,
                        ..SearchFilters::default()
                    },
                    ASK_TOP_K,
                )?;
                Ok(e.excerpts_for(&hits))
            })
            .await?;
        let (tx, rx) = mpsc::channel(64);
        let request = AnswerRequest {
            question,
            language,
            excerpts: excerpts.clone(),
        };
        tokio::spawn(async move {
            if let Err(e) = answerer.answer(request, tx.clone()).await {
                let _ = tx.send(AnswerEvent::Failed(e.to_string())).await;
            }
        });
        Ok((excerpts, rx))
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
