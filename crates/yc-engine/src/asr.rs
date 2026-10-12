//! The shared ASR worker: one model instance, one queue ordered by region end time.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use yc_core::meeting::TrackKind;
use yc_core::ports::{ModelInfo, SpeechRecognizer};
use yc_core::transcript::{SPEAKER_ME, SPEAKER_THEM, Segment, Word};
use yc_core::{Error, Result, SAMPLE_RATE, samples_to_ms};

use crate::livelog::{LiveLine, LiveLog};
use crate::ports::AsrFactory;
use crate::regions::{Region, split_chunks};

/// Speaker key of a track before diarisation.
pub fn track_speaker(track: TrackKind) -> &'static str {
    match track {
        TrackKind::Mic => SPEAKER_ME,
        TrackKind::System => SPEAKER_THEM,
    }
}

/// Recognises one region (split into model-sized chunks) and returns segments with times as
/// **file positions** in ms (region start + offset inside the region).
pub fn recognize_region(
    asr: &mut dyn SpeechRecognizer,
    language: &str,
    region: &Region,
) -> Result<Vec<Segment>> {
    let max = (asr.max_chunk_ms() * u64::from(SAMPLE_RATE) / 1000) as usize;
    let mut out = Vec::new();
    for (offset, len) in split_chunks(&region.pcm, max) {
        let base_ms = samples_to_ms(region.start + offset as u64);
        for rec in asr.recognize(&region.pcm[offset..offset + len], language)? {
            let text = rec.text.trim();
            if text.is_empty() {
                continue;
            }
            out.push(Segment {
                start_ms: base_ms + rec.start_ms,
                end_ms: base_ms + rec.end_ms,
                track: region.track,
                speaker: track_speaker(region.track).to_owned(),
                text: text.to_owned(),
                words: rec
                    .words
                    .iter()
                    .map(|w| Word {
                        start_ms: base_ms + w.start_ms,
                        end_ms: base_ms + w.end_ms,
                        text: w.text.clone(),
                    })
                    .collect(),
                confidence: rec.confidence,
            });
        }
    }
    Ok(out)
}

struct QueueState {
    tasks: BTreeMap<(u64, u64), Region>,
    seq: u64,
    closed: bool,
    aborted: bool,
    queued_ms: u64,
    total_ms: u64,
    done_ms: u64,
}

/// Regions waiting for the recogniser, popped in order of their end time on the (approximate)
/// shared timeline so mic and system interleave naturally.
pub struct AsrQueue {
    state: Mutex<QueueState>,
    cv: Condvar,
}

impl AsrQueue {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(QueueState {
                tasks: BTreeMap::new(),
                seq: 0,
                closed: false,
                aborted: false,
                queued_ms: 0,
                total_ms: 0,
                done_ms: 0,
            }),
            cv: Condvar::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Queues a region. Never drops audio, however far behind the worker is.
    pub fn push(&self, region: Region) {
        let ms = samples_to_ms(region.end - region.start);
        let key = samples_to_ms(region.end);
        let mut s = self.lock();
        if s.closed {
            return;
        }
        s.seq += 1;
        let seq = s.seq;
        s.queued_ms += ms;
        s.total_ms += ms;
        s.tasks.insert((key, seq), region);
        self.cv.notify_one();
    }

    /// Blocks for the next region; `None` once closed and drained, or aborted.
    pub fn pop(&self) -> Option<Region> {
        let mut s = self.lock();
        loop {
            if s.aborted {
                return None;
            }
            if let Some((_, region)) = s.tasks.pop_first() {
                s.queued_ms = s
                    .queued_ms
                    .saturating_sub(samples_to_ms(region.end - region.start));
                return Some(region);
            }
            if s.closed {
                return None;
            }
            s = self.cv.wait(s).unwrap_or_else(|p| p.into_inner());
        }
    }

    pub fn mark_done(&self, region: &Region) {
        self.lock().done_ms += samples_to_ms(region.end - region.start);
    }

    /// No more regions will be pushed; the worker exits after draining.
    pub fn close(&self) {
        self.lock().closed = true;
        self.cv.notify_all();
    }

    /// Worker exits as soon as possible, leaving queued regions unprocessed.
    pub fn abort(&self) {
        self.lock().aborted = true;
        self.cv.notify_all();
    }

    /// Speech audio waiting to be recognised, in ms.
    pub fn backlog_ms(&self) -> u64 {
        self.lock().queued_ms
    }

    /// `(recognised, total queued so far)` in ms of speech audio.
    pub fn progress(&self) -> (u64, u64) {
        let s = self.lock();
        (s.done_ms, s.total_ms)
    }
}

impl Default for AsrQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// What the worker needs from its surroundings.
pub struct AsrWorker {
    pub queue: Arc<AsrQueue>,
    pub factory: Arc<dyn AsrFactory>,
    pub language: String,
    pub log: Arc<LiveLog>,
    /// Called for every segment after it was appended to the log.
    pub on_segment: Box<dyn Fn(Segment) + Send>,
    /// Called for load/recognition failures (rate-limited by the callee).
    pub on_error: Box<dyn Fn(&Error) + Send>,
    /// Receives the model info once loaded.
    pub model: Arc<Mutex<Option<ModelInfo>>>,
    pub stop: Arc<AtomicBool>,
}

impl AsrWorker {
    pub fn spawn(self) -> std::io::Result<JoinHandle<()>> {
        std::thread::Builder::new()
            .name("yc-asr".into())
            .spawn(move || self.run())
    }

    fn run(self) {
        let mut asr = match self.factory.load(&self.language) {
            Ok(a) => {
                if let Ok(mut m) = self.model.lock() {
                    *m = Some(a.info().clone());
                }
                Some(a)
            }
            Err(e) => {
                tracing::error!("ASR model failed to load: {e}");
                (self.on_error)(&e);
                None
            }
        };
        while let Some(region) = self.queue.pop() {
            if self.stop.load(Ordering::Relaxed) {
                break;
            }
            // A panicking model must not take the queue with it: the region becomes a gap that the
            // finalize pass transcribes with a fresh instance.
            let result = match asr.as_mut() {
                Some(a) => {
                    let language = &self.language;
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        recognize_region(a.as_mut(), language, &region)
                    }))
                    .unwrap_or_else(|_| {
                        Err(Error::Model {
                            model: a.info().id.clone(),
                            message: "the recogniser panicked".into(),
                        })
                    })
                }
                None => Err(Error::ModelMissing("asr".into())),
            };
            if matches!(&result, Err(Error::Model { message, .. }) if message.contains("panicked"))
            {
                asr = None;
            }
            match result {
                Ok(segments) => {
                    for segment in segments {
                        if let Err(e) = self.log.append(&LiveLine::Segment {
                            segment: segment.clone(),
                        }) {
                            tracing::error!("live.jsonl append failed: {e}");
                        }
                        (self.on_segment)(segment);
                    }
                }
                Err(e) => {
                    tracing::warn!("recognition failed, region left for the finalize pass: {e}");
                    if asr.is_some() {
                        (self.on_error)(&e);
                    }
                    let _ = self.log.append(&LiveLine::Gap {
                        track: region.track,
                        start_sample: region.start,
                        end_sample: region.end,
                    });
                }
            }
            self.queue.mark_done(&region);
        }
        let _ = self.log.sync();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(track: TrackKind, start: u64, end: u64) -> Region {
        Region {
            track,
            start,
            end,
            pcm: vec![0.0; (end - start) as usize],
        }
    }

    #[test]
    fn pops_in_end_time_order_across_tracks() {
        let q = AsrQueue::new();
        q.push(region(TrackKind::System, 32_000, 64_000)); // ends 4 s
        q.push(region(TrackKind::Mic, 0, 16_000)); // ends 1 s
        q.push(region(TrackKind::Mic, 16_000, 48_000)); // ends 3 s
        assert_eq!(q.backlog_ms(), 5_000);
        q.close();
        let order: Vec<_> = std::iter::from_fn(|| q.pop()).map(|r| r.end).collect();
        assert_eq!(order, vec![16_000, 48_000, 64_000]);
        assert_eq!(q.backlog_ms(), 0);
    }

    #[test]
    fn abort_wakes_a_blocked_worker() {
        let q = Arc::new(AsrQueue::new());
        let q2 = Arc::clone(&q);
        let h = std::thread::spawn(move || q2.pop().is_none());
        std::thread::sleep(std::time::Duration::from_millis(50));
        q.abort();
        assert!(h.join().unwrap());
    }
}
