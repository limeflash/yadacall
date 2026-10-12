//! Handlers of the follow-up jobs: diarise, summarise, archive audio, index for search,
//! re-transcribe.

use std::collections::BTreeMap;
use std::io::{BufWriter, Write};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use yc_audio::writer::{TrackMeta, meta_path, pcm_path, read_meta};
use yc_core::events::EngineEvent;
use yc_core::meeting::{Meeting, MeetingStatus, Participant, Stage, TrackKind};
use yc_core::transcript::SPEAKER_THEM;
use yc_core::{Error, Result, SAMPLE_RATE, ms_to_samples};
use yc_store::Job;
use yc_text::{build_turns, chunk_turns};

use crate::audio::{TrackAudio, write_s16};
use crate::diarize::{
    ClusterConfig, KnownVoice, assign_unlabelled, centroid, cluster, from_bytes, match_voices,
};
use crate::engine::Engine;
use crate::finalize::{FinalizeOpts, audio_dir};
use crate::jobs::JobPayload;
use crate::meta::{SessionMeta, Source};
use crate::ports::SummaryInput;
use crate::timeline::track_to_file_time;

/// Longest stretch of one utterance fed to the speaker model.
const MAX_EMBED_MS: u64 = 15_000;
/// Utterances shorter than this are labelled by their temporal neighbours instead.
const MIN_EMBED_MS: u64 = 1_000;

/// `speakers.json` in the meeting folder: the voice of each diarised speaker, kept so naming a
/// speaker later can teach the voice library.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SpeakerProfiles {
    pub speakers: BTreeMap<String, Profile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Profile {
    pub centroid: Vec<f32>,
    /// Utterances the centroid averages.
    pub samples: u64,
}

pub(crate) const PROFILES_FILE: &str = "speakers.json";

impl SpeakerProfiles {
    pub fn read(folder: &std::path::Path) -> Self {
        std::fs::read(folder.join(PROFILES_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn write(&self, folder: &std::path::Path) -> Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| Error::Storage(e.to_string()))?;
        crate::meta::write_atomic(&folder.join(PROFILES_FILE), &json)
    }
}

/// Display names of the speaker keys used in a meeting (`me`, `them`, diarised keys).
pub(crate) fn speaker_names(meeting: &Meeting) -> Vec<(String, String)> {
    let english = meeting
        .language
        .split('-')
        .next()
        .is_some_and(|l| l.eq_ignore_ascii_case("en"));
    let mut names = vec![
        ("me".to_owned(), if english { "Me" } else { "Я" }.to_owned()),
        (
            "them".to_owned(),
            if english {
                "Them"
            } else {
                "Собеседник"
            }
            .to_owned(),
        ),
    ];
    for p in &meeting.participants {
        match names.iter_mut().find(|(k, _)| k == &p.key) {
            Some((_, n)) => *n = p.name.clone(),
            None => names.push((p.key.clone(), p.name.clone())),
        }
    }
    names
}

fn is_diarised_key(key: &str) -> bool {
    key == SPEAKER_THEM
        || (key.starts_with('s') && key.len() > 1 && key[1..].chars().all(|c| c.is_ascii_digit()))
}

impl Engine {
    pub(crate) fn run_diarize(&self, job: &Job, payload: &JobPayload) -> Result<()> {
        let id = job.meeting_id;
        let rec = self.store.get_meeting(id)?;
        let settings = self.settings();
        let segments = &rec.transcript.segments;
        let them: Vec<usize> = (0..segments.len())
            .filter(|&i| segments[i].track == TrackKind::System)
            .collect();
        let skip = |me: &Engine| me.settle_status_blocking(id, payload);
        if them.is_empty() {
            return skip(self);
        }
        let Some(mut embedder) = self.ports.speaker.create()? else {
            return skip(self);
        };
        let audio_path = audio_dir(&self.library_root(), &rec.folder);
        let Some(mut audio) = TrackAudio::open(&audio_path, TrackKind::System, &self.tmp_dir())?
        else {
            return skip(self);
        };
        let pauses = read_meta(&meta_path(&audio_path, TrackKind::System))
            .map(|m| m.pauses)
            .unwrap_or_default();
        let offset = rec
            .meeting
            .tracks
            .iter()
            .find(|t| t.kind == TrackKind::System)
            .map_or(0, |t| t.offset_ms);

        // Embed every utterance that is long enough to carry a voice.
        let mut embedded: Vec<(usize, Vec<f32>, f32)> = Vec::new(); // (position in `them`, vector, secs)
        let mut failures = 0usize;
        for (pos, &i) in them.iter().enumerate() {
            let seg = &segments[i];
            let dur = seg.end_ms.saturating_sub(seg.start_ms);
            if dur < MIN_EMBED_MS {
                continue;
            }
            let file_ms = track_to_file_time(seg.start_ms.saturating_sub(offset), &pauses);
            let pcm = audio.read(
                ms_to_samples(file_ms),
                ms_to_samples(dur.min(MAX_EMBED_MS)) as usize,
            )?;
            if pcm.len() < SAMPLE_RATE as usize / 2 {
                continue;
            }
            match embedder.embed(&pcm) {
                Ok(v) => embedded.push((pos, v, dur as f32 / 1000.0)),
                Err(e) => {
                    failures += 1;
                    tracing::warn!("speaker embedding failed: {e}");
                }
            }
        }
        if embedded.is_empty() {
            if failures > 0 {
                return Err(Error::Model {
                    model: embedder.info().id.clone(),
                    message: "speaker embedding failed for every utterance".into(),
                });
            }
            return skip(self);
        }

        let vectors: Vec<Vec<f32>> = embedded.iter().map(|(_, v, _)| v.clone()).collect();
        let secs: Vec<f32> = embedded.iter().map(|(_, _, s)| *s).collect();
        let cfg = ClusterConfig {
            distance_threshold: settings.speaker_cluster_threshold,
            num_speakers: rec.meeting.expected_speakers.map(usize::from),
            ..Default::default()
        };
        let cluster_labels = cluster(&vectors, &secs, &cfg);
        let mut per_segment: Vec<Option<usize>> = vec![None; them.len()];
        for ((pos, _, _), l) in embedded.iter().zip(&cluster_labels) {
            per_segment[*pos] = Some(*l);
        }
        let spans: Vec<(u64, u64)> = them
            .iter()
            .map(|&i| (segments[i].start_ms, segments[i].end_ms))
            .collect();
        let labels = assign_unlabelled(&spans, &per_segment);
        let k = labels.iter().copied().max().map_or(1, |m| m + 1);
        let key_of = |c: usize| {
            if k == 1 {
                SPEAKER_THEM.to_owned()
            } else {
                format!("s{}", c + 1)
            }
        };

        // Cluster voices, matched against the saved ones.
        let centroids: Vec<Vec<f32>> = (0..k)
            .map(|c| {
                centroid(
                    embedded
                        .iter()
                        .zip(&cluster_labels)
                        .filter(|(_, l)| **l == c)
                        .map(|((_, v, _), _)| v.as_slice()),
                )
            })
            .collect();
        let known: Vec<KnownVoice> = if settings.recognise_voices {
            self.store
                .list_voices()?
                .into_iter()
                .map(|v| KnownVoice {
                    id: v.id,
                    name: v.name,
                    centroid: from_bytes(&v.centroid),
                })
                .collect()
        } else {
            Vec::new()
        };
        let matches = match_voices(&centroids, &known, settings.voice_match_threshold);

        let english = rec.meeting.language.to_ascii_lowercase().starts_with("en");
        let mut participants: Vec<Participant> = rec
            .meeting
            .participants
            .iter()
            .filter(|p| !is_diarised_key(&p.key))
            .cloned()
            .collect();
        for (c, matched) in matches.iter().enumerate() {
            let voice = matched.map(|v| &known[v]);
            if k == 1 && voice.is_none() {
                continue; // a single unnamed remote speaker stays plain "them"
            }
            participants.push(Participant {
                key: key_of(c),
                name: voice.map_or_else(
                    || format!("{} {}", if english { "Speaker" } else { "Спикер" }, c + 1),
                    |v| v.name.clone(),
                ),
                voice_id: voice.map(|v| v.id.clone()),
            });
        }

        let mut profiles = SpeakerProfiles::default();
        for (c, cen) in centroids.iter().enumerate() {
            let samples = cluster_labels.iter().filter(|l| **l == c).count() as u64;
            profiles.speakers.insert(
                key_of(c),
                Profile {
                    centroid: cen.clone(),
                    samples,
                },
            );
        }
        profiles.write(&self.library_root().join(&rec.folder))?;

        let n = segments.len();
        self.mutate_meeting(id, |rec| {
            if rec.transcript.segments.len() != n {
                return Err(Error::Invalid(
                    "the transcript changed during diarisation".into(),
                ));
            }
            for (pos, &i) in them.iter().enumerate() {
                rec.transcript.segments[i].speaker = key_of(labels[pos]);
            }
            rec.meeting.participants = participants;
            rec.meeting.status = Self::next_status(&rec.meeting.status, payload);
            Ok(((), true))
        })
    }

    pub(crate) async fn run_summarize(
        self: &Arc<Self>,
        job: &Job,
        payload: &JobPayload,
    ) -> Result<()> {
        let id = job.meeting_id;
        let Some(summarizer) = self.ports.summarizer.clone().filter(|s| s.is_configured()) else {
            return self.settle_status(id, payload).await;
        };
        let rec = self.blocking(move |e| e.store.get_meeting(id)).await?;
        let turns = build_turns(&rec.transcript.segments);
        if turns.is_empty() {
            return self.settle_status(id, payload).await;
        }
        let participants = speaker_names(&rec.meeting)
            .into_iter()
            .map(|(key, name)| Participant {
                voice_id: rec
                    .meeting
                    .participants
                    .iter()
                    .find(|p| p.key == key)
                    .and_then(|p| p.voice_id.clone()),
                key,
                name,
            })
            .collect();
        let input = SummaryInput {
            meeting_id: id,
            title: rec.meeting.title.clone(),
            started_at: rec.meeting.started_at,
            language: rec.meeting.language.clone(),
            duration_ms: rec.meeting.duration_ms,
            turns,
            participants,
            bookmarks: rec.meeting.bookmarks.clone(),
        };
        let events = self.events_sender();
        let (step, steps) = (payload.step as f32, payload.plan.len().max(1) as f32);
        let progress = move |p: f32| {
            let _ = events.send(EngineEvent::Status {
                meeting: id,
                status: MeetingStatus::Processing {
                    stage: Stage::Summarize,
                    progress: ((step + p.clamp(0.0, 1.0)) / steps).clamp(0.0, 1.0),
                },
            });
        };
        let summary = summarizer.summarize(input, &progress).await?;
        let payload = payload.clone();
        self.blocking(move |e| {
            e.mutate_meeting(id, |rec| {
                let title_untouched = payload
                    .auto_title
                    .as_deref()
                    .is_some_and(|t| t == rec.meeting.title);
                if title_untouched
                    && let Some(t) = summary
                        .title
                        .as_deref()
                        .map(str::trim)
                        .filter(|t| !t.is_empty())
                {
                    rec.meeting.title = t.to_owned();
                }
                rec.summary = Some(summary);
                rec.meeting.status = Self::next_status(&rec.meeting.status, &payload);
                Ok(((), true))
            })
        })
        .await
    }

    /// Applies the retention policy to the raw tracks: FLAC for the retained, nothing for "none" —
    /// and only once the transcript is on disk. A meeting whose transcription failed keeps its audio.
    pub(crate) fn run_archive(&self, job: &Job, _payload: &JobPayload) -> Result<()> {
        let rec = self.store.get_meeting(job.meeting_id)?;
        let folder = self.library_root().join(&rec.folder);
        let audio = audio_dir(&self.library_root(), &rec.folder);
        if !audio.is_dir() {
            return Ok(());
        }
        let transcribe_failed = matches!(
            rec.meeting.status,
            MeetingStatus::Failed {
                stage: Stage::Transcribe,
                ..
            }
        );
        let transcript_on_disk = folder.join("transcript.json").is_file();
        let delete_all = self.settings().keep_audio.days() == Some(0)
            && transcript_on_disk
            && !transcribe_failed;
        for kind in TrackKind::ALL {
            let pcm = pcm_path(&audio, kind);
            if !pcm.is_file() {
                continue;
            }
            if !delete_all {
                let flac = audio.join(format!("{}.flac", kind.file_stem()));
                yc_audio::encode_flac(&pcm, &flac)?;
            }
            std::fs::remove_file(&pcm)?;
        }
        if delete_all {
            std::fs::remove_dir_all(&audio)?;
        }
        Ok(())
    }

    /// Chunks the transcript, embeds the chunks and stores the vectors. The full-text index is
    /// always current (every save rewrites it); without an embedding model this only settles the
    /// meeting's status.
    pub(crate) fn run_index(&self, job: &Job, payload: &JobPayload) -> Result<()> {
        let id = job.meeting_id;
        if !payload.reindex_only {
            // Final status first: saving rewrites the index rows and would drop fresh vectors.
            self.settle_status_blocking(id, payload)?;
        }
        if !self.ports.text_embedder.is_available() {
            return Ok(());
        }
        let rec = self.store.get_meeting(id)?;
        let turns = build_turns(&rec.transcript.segments);
        let chunks = chunk_turns(id, &turns, &speaker_names(&rec.meeting));
        if chunks.is_empty() {
            return Ok(());
        }
        let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
        let embedded = self.with_text_embedder(|embedder| {
            let mut all = Vec::with_capacity(texts.len());
            for batch in texts.chunks(16) {
                all.extend(embedder.embed(batch, yc_core::ports::EmbedKind::Passage)?);
            }
            Ok((embedder.info().id.clone(), all))
        })?;
        let Some((model, vectors)) = embedded else {
            return Ok(());
        };
        let stored = {
            let _guard = self
                .write_lock
                .lock()
                .map_err(|_| Error::Storage("write lock poisoned".into()))?;
            self.store.set_chunk_embeddings(id, &model, &vectors)
        };
        match stored {
            Err(Error::Invalid(m)) => {
                // The transcript changed while we embedded; embed the new chunks next.
                tracing::info!("re-index of {id} raced with an edit ({m}); retrying later");
                self.schedule_reindex(id);
                Ok(())
            }
            other => other,
        }
    }

    /// Rebuilds a recording folder from the stored audio and runs the whole finalisation again.
    pub(crate) fn run_retranscribe(&self, job: &Job, _payload: &JobPayload) -> Result<()> {
        let id = job.meeting_id;
        let rec = self.store.get_meeting(id)?;
        let audio = audio_dir(&self.library_root(), &rec.folder);
        let dir = self.recording_root().join(id.to_string());
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let mut found = false;
        for kind in TrackKind::ALL {
            let Some(mut src) = TrackAudio::open(&audio, kind, &self.tmp_dir())? else {
                continue;
            };
            found = true;
            let mut out = BufWriter::new(std::fs::File::create(pcm_path(&dir, kind))?);
            let mut pos = 0u64;
            while pos < src.samples() {
                let block = src.read(pos, 10 * SAMPLE_RATE as usize)?;
                if block.is_empty() {
                    break;
                }
                pos += block.len() as u64;
                write_s16(&mut out, &block)?;
            }
            out.flush()?;
            let meta_src = meta_path(&audio, kind);
            if meta_src.is_file() {
                std::fs::copy(&meta_src, meta_path(&dir, kind))?;
            } else {
                let offset = rec
                    .meeting
                    .tracks
                    .iter()
                    .find(|t| t.kind == kind)
                    .map_or(0, |t| t.offset_ms);
                let meta = TrackMeta::new(kind, "unknown", 0, offset);
                yc_audio::writer::write_meta(&meta_path(&dir, kind), &meta)?;
            }
        }
        if !found {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(Error::Invalid(
                "the audio of this meeting is no longer on disk".into(),
            ));
        }
        SessionMeta {
            id,
            title: rec.meeting.title.clone(),
            title_auto: false,
            started_at: rec.meeting.started_at,
            language: rec.meeting.language.clone(),
            expected_speakers: rec.meeting.expected_speakers,
            source: Source::Retranscribe,
            bookmarks: rec.meeting.bookmarks.clone(),
        }
        .write(&dir)?;
        let language = rec.meeting.language.clone();
        let result = self.finalize_blocking(
            &dir,
            &FinalizeOpts {
                language,
                fill_tail: true,
            },
        );
        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                // Keep the stored audio; drop the half-built folder.
                let _ = std::fs::remove_dir_all(&dir);
                Err(e)
            }
        }
    }
}
