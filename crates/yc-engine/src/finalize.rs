//! Turning a recording folder (live or recovered) into a saved meeting.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use yc_audio::writer::{PauseSpan, meta_path, read_meta};
use yc_core::events::{EngineEvent, NoticeLevel};
use yc_core::meeting::{Meeting, MeetingId, MeetingStatus, Stage, TrackInfo, TrackKind};
use yc_core::transcript::{Segment, Transcript};
use yc_core::{Error, Result, samples_to_ms};
use yc_text::{
    EchoFilterConfig, HallucinationConfig, filter_echo_duplicates, filter_hallucinations,
    merge_tracks,
};

use crate::engine::Engine;
use crate::jobs::{JobPayload, kind_stage};
use crate::livelog::{LiveLine, LiveLog, open_gaps, read_live};
use crate::meta::{LIVE_FILE, SessionMeta, read_asr_model, write_asr_model};
use crate::offline::{OfflineEnv, transcribe_range};
use crate::timeline::{file_to_track_time, segment_to_track_time};

/// How [`Engine::finalize_dir`] should treat the folder.
pub(crate) struct FinalizeOpts {
    pub language: String,
    /// Also transcribe whatever follows the last live segment of each track (crash recovery,
    /// imports): the live pipeline did not get to it.
    pub fill_tail: bool,
}

impl Engine {
    /// Finalises a recording folder into a saved meeting; see the module docs. Async wrapper.
    pub(crate) async fn finalize_dir(
        self: &Arc<Self>,
        dir: &Path,
        opts: FinalizeOpts,
    ) -> Result<MeetingId> {
        let dir = dir.to_path_buf();
        self.blocking(move |e| e.finalize_blocking(&dir, &opts))
            .await
    }

    pub(crate) fn finalize_blocking(&self, dir: &Path, opts: &FinalizeOpts) -> Result<MeetingId> {
        let meta = SessionMeta::read(dir)?;
        let id = meta.id;

        // Everything already moved into the library by an earlier, interrupted attempt?
        if !TrackKind::ALL
            .iter()
            .any(|k| yc_audio::writer::pcm_path(dir, *k).is_file())
            && self.store.get_meeting(id).is_ok()
        {
            let _ = std::fs::remove_dir_all(dir);
            return Ok(id);
        }

        let transcribe_error = self.catch_up(dir, &meta, opts).err();
        if let Some(e) = &transcribe_error {
            tracing::error!("transcription of {id} failed: {e}");
        }

        let tracks = yc_audio::recover(dir)?;
        if tracks.iter().all(|t| t.samples == 0) {
            let _ = std::fs::remove_dir_all(dir);
            return Err(Error::Invalid("nothing was recorded".into()));
        }
        let pauses: HashMap<TrackKind, Vec<PauseSpan>> = tracks
            .iter()
            .map(|t| {
                (
                    t.kind,
                    read_meta(&meta_path(dir, t.kind))
                        .map(|m| m.pauses)
                        .unwrap_or_default(),
                )
            })
            .collect();

        let duration_ms = tracks
            .iter()
            .map(|t| {
                t.offset_ms
                    + file_to_track_time(
                        samples_to_ms(t.samples),
                        pauses.get(&t.kind).map_or(&[][..], |p| p.as_slice()),
                    )
            })
            .max()
            .unwrap_or(0);

        let mut meeting = Meeting {
            id,
            title: meta.title.clone(),
            started_at: meta.started_at,
            duration_ms,
            language: meta.language.clone(),
            expected_speakers: meta.expected_speakers,
            tracks: tracks.clone(),
            bookmarks: meta.bookmarks.clone(),
            participants: Vec::new(),
            asr_model: read_asr_model(dir),
            status: MeetingStatus::Ready,
        };
        let segments = build_transcript(dir, &meeting, &pauses)?;
        let has_system_speech = segments.iter().any(|s| s.track == TrackKind::System);

        let plan = self.plan_pipeline(
            &meeting,
            has_system_speech,
            !segments.is_empty(),
            &tracks,
            transcribe_error.is_some(),
        );
        meeting.status = match &transcribe_error {
            Some(e) => MeetingStatus::Failed {
                stage: Stage::Transcribe,
                message: e.to_string(),
                retryable: true,
            },
            None => MeetingStatus::Processing {
                stage: plan.first().map_or(Stage::Index, |k| kind_stage(k)),
                progress: 0.0,
            },
        };

        let transcript = Transcript { segments };
        let folder = {
            let _w = self
                .write_lock
                .lock()
                .map_err(|_| Error::Storage("write lock poisoned".into()))?;
            self.store.save_meeting(&meeting, &transcript, None)?;
            self.store.get_meeting(id)?.folder
        };
        self.adopt_audio(dir, &folder, &tracks)?;
        let _ = std::fs::remove_dir_all(dir);

        let auto_title = meta.title_auto.then(|| meta.title.clone());
        for (step, kind) in plan.iter().enumerate() {
            let payload = JobPayload {
                plan: plan.iter().map(|k| (*k).to_owned()).collect(),
                step: step as u32,
                reindex_only: false,
                auto_title: auto_title.clone(),
            };
            self.enqueue_job(id, kind, &payload)?;
        }
        self.emit(EngineEvent::Status {
            meeting: id,
            status: meeting.status.clone(),
        });
        match &transcribe_error {
            None => self.notice(
                NoticeLevel::Info,
                &format!("Расшифровка готова: {}", meeting.title),
                &format!("Transcript ready: {}", meeting.title),
            ),
            Some(e) => self.notice(
                NoticeLevel::Error,
                &format!(
                    "Не удалось расшифровать «{}»: {e}. Аудио сохранено — можно повторить.",
                    meeting.title
                ),
                &format!(
                    "Could not transcribe \"{}\": {e}. The audio is kept; you can retry.",
                    meeting.title
                ),
            ),
        }
        Ok(id)
    }

    /// Which follow-up jobs this meeting needs, in execution order.
    fn plan_pipeline(
        &self,
        meeting: &Meeting,
        has_system_speech: bool,
        has_text: bool,
        tracks: &[TrackInfo],
        transcribe_failed: bool,
    ) -> Vec<&'static str> {
        let mut plan = Vec::new();
        if !transcribe_failed {
            let system_audio = tracks
                .iter()
                .any(|t| t.kind == TrackKind::System && t.samples > 0);
            if meeting.expected_speakers != Some(1)
                && has_system_speech
                && system_audio
                && self.ports.speaker.is_available()
            {
                plan.push("diarize");
            }
            if has_text
                && self
                    .ports
                    .summarizer
                    .as_ref()
                    .is_some_and(|s| s.is_configured())
            {
                plan.push("summarize");
            }
        }
        plan.push("archive");
        plan.push("index");
        plan
    }

    /// Moves the raw tracks (and their metadata) from the recording folder into
    /// `<library>/<folder>/audio/`.
    fn adopt_audio(&self, dir: &Path, folder: &str, tracks: &[TrackInfo]) -> Result<()> {
        let audio_dir = self.library_root().join(folder).join("audio");
        std::fs::create_dir_all(&audio_dir)?;
        for t in tracks {
            for src in [
                yc_audio::writer::pcm_path(dir, t.kind),
                meta_path(dir, t.kind),
            ] {
                if let Some(name) = src.file_name()
                    && src.is_file()
                {
                    move_file(&src, &audio_dir.join(name))?;
                }
            }
        }
        Ok(())
    }

    /// Transcribes what live processing did not: logged gaps and, when asked, the tail after the
    /// last segment of each track. A model that cannot load is an error.
    fn catch_up(&self, dir: &Path, meta: &SessionMeta, opts: &FinalizeOpts) -> Result<()> {
        let lines = read_live(&dir.join(LIVE_FILE))?;
        let mut ranges = open_gaps(&lines);
        if opts.fill_tail {
            for kind in TrackKind::ALL {
                let pcm = yc_audio::writer::pcm_path(dir, kind);
                let Ok(md) = std::fs::metadata(&pcm) else {
                    continue;
                };
                let total = md.len() / 2;
                let last_end = lines
                    .iter()
                    .filter_map(|l| match l {
                        LiveLine::Segment { segment } if segment.track == kind => {
                            Some(yc_core::ms_to_samples(segment.end_ms))
                        }
                        _ => None,
                    })
                    .max()
                    .unwrap_or(0);
                if last_end < total {
                    ranges.push((kind, last_end, total));
                }
            }
        }
        ranges.retain(|(_, s, e)| e > s);
        let ranges = merge_ranges(ranges);
        if ranges.is_empty() {
            return Ok(());
        }
        let total_samples: u64 = ranges.iter().map(|(_, s, e)| e - s).sum();
        let mut asr = self.ports.asr.load(&opts.language)?;
        let _ = write_asr_model(dir, &asr.info().id);
        let log = LiveLog::open(&dir.join(LIVE_FILE))?;
        let mut env = OfflineEnv {
            vad: self.ports.vad.as_ref(),
            asr: asr.as_mut(),
            language: &opts.language,
            log: &log,
            cancel: &self.cancel_offline,
        };
        let mut done_before = 0u64;
        let mut last_emit = Instant::now() - Duration::from_secs(1);
        for (kind, start, end) in &ranges {
            let id = meta.id;
            transcribe_range(&mut env, dir, *kind, *start, *end, &mut |done| {
                if last_emit.elapsed() >= Duration::from_millis(250) {
                    last_emit = Instant::now();
                    let p = (done_before + done) as f32 / total_samples.max(1) as f32;
                    self.emit(EngineEvent::Status {
                        meeting: id,
                        status: MeetingStatus::Processing {
                            stage: Stage::Transcribe,
                            progress: p.clamp(0.0, 1.0),
                        },
                    });
                }
            })?;
            done_before += end - start;
        }
        for l in &lines {
            if let LiveLine::Gap {
                track,
                start_sample,
                end_sample,
            } = l
            {
                log.append(&LiveLine::Filled {
                    track: *track,
                    start_sample: *start_sample,
                    end_sample: *end_sample,
                })?;
            }
        }
        log.sync()?;
        if self.cancel_offline.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        Ok(())
    }
}

/// Sorts sample ranges per track and unions the overlapping ones, so no audio is transcribed twice.
fn merge_ranges(mut ranges: Vec<(TrackKind, u64, u64)>) -> Vec<(TrackKind, u64, u64)> {
    ranges.sort_by_key(|r| (r.0 as u8, r.1));
    let mut out: Vec<(TrackKind, u64, u64)> = Vec::new();
    for r in ranges {
        match out.last_mut() {
            Some(last) if last.0 == r.0 && r.1 <= last.2 => last.2 = last.2.max(r.2),
            _ => out.push(r),
        }
    }
    out
}

/// Live segments of the folder → one cleaned, merged, speaker-labelled list on the meeting timeline.
fn build_transcript(
    dir: &Path,
    meeting: &Meeting,
    pauses: &HashMap<TrackKind, Vec<PauseSpan>>,
) -> Result<Vec<Segment>> {
    let lines = read_live(&dir.join(LIVE_FILE))?;
    let mut per_track: HashMap<TrackKind, Vec<Segment>> = HashMap::new();
    for line in lines {
        if let LiveLine::Segment { mut segment } = line {
            let p = pauses.get(&segment.track).map_or(&[][..], |p| p.as_slice());
            segment_to_track_time(&mut segment, p);
            per_track.entry(segment.track).or_default().push(segment);
        }
    }
    let mut mic = per_track.remove(&TrackKind::Mic).unwrap_or_default();
    let mut them = per_track.remove(&TrackKind::System).unwrap_or_default();
    mic.sort_by_key(|s| s.start_ms);
    them.sort_by_key(|s| s.start_ms);
    let merged = merge_tracks(&mic, &them, meeting);
    let echo = filter_echo_duplicates(&merged, EchoFilterConfig::default());
    let clean = filter_hallucinations(&echo.kept, HallucinationConfig::default());
    Ok(clean.kept)
}

/// Renames, falling back to copy + delete across file systems.
pub(crate) fn move_file(src: &Path, dst: &Path) -> Result<()> {
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    std::fs::copy(src, dst)?;
    std::fs::remove_file(src)?;
    Ok(())
}

/// `<library>/<folder>/audio`.
pub(crate) fn audio_dir(library: &Path, folder: &str) -> PathBuf {
    library.join(folder).join("audio")
}
