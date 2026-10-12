//! Offline transcription of a stretch of a track's `.pcm`: VAD + ASR over the file in bounded
//! blocks. Used for crash recovery, imports, live-pipeline gaps and re-transcription.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use yc_core::meeting::TrackKind;
use yc_core::ports::SpeechRecognizer;
use yc_core::{Error, Result, SAMPLE_RATE};

use crate::asr::recognize_region;
use crate::audio::read_pcm_block;
use crate::livelog::{LiveLine, LiveLog};
use crate::ports::VadFactory;
use crate::regions::{Region, RegionExtractor};

/// Samples read per block (10 s keeps memory flat for any recording length).
const BLOCK: usize = 10 * SAMPLE_RATE as usize;

/// Everything an offline pass needs besides the audio.
pub struct OfflineEnv<'a> {
    pub vad: &'a dyn VadFactory,
    pub asr: &'a mut dyn SpeechRecognizer,
    pub language: &'a str,
    pub log: &'a LiveLog,
    pub cancel: &'a AtomicBool,
}

/// Transcribes samples `[start, end)` of `<dir>/<track>.pcm` and appends the segments to `env.log`.
/// `progress` receives the number of samples processed so far. Returns the number of segments.
pub fn transcribe_range(
    env: &mut OfflineEnv<'_>,
    dir: &Path,
    track: TrackKind,
    start: u64,
    end: u64,
    progress: &mut dyn FnMut(u64),
) -> Result<usize> {
    let pcm = yc_audio::writer::pcm_path(dir, track);
    let mut file = match std::fs::File::open(&pcm) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let total = file.metadata()?.len() / 2;
    let end = end.min(total);
    if start >= end {
        return Ok(0);
    }
    let mut extractor = RegionExtractor::new(track, env.vad.create()?, start);
    let mut produced = 0usize;
    let mut pos = start;
    while pos < end {
        if env.cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let want = BLOCK.min((end - pos) as usize);
        let block = read_pcm_block(&mut file, pos, want)?;
        if block.is_empty() {
            break;
        }
        pos += block.len() as u64;
        for region in extractor.feed(&block)? {
            produced += handle_region(env, &region)?;
        }
        progress(pos - start);
    }
    for region in extractor.flush() {
        produced += handle_region(env, &region)?;
    }
    env.log.sync()?;
    progress(end - start);
    Ok(produced)
}

fn handle_region(env: &mut OfflineEnv<'_>, region: &Region) -> Result<usize> {
    let segments = recognize_region(env.asr, env.language, region)?;
    let n = segments.len();
    for segment in segments {
        env.log.append(&LiveLine::Segment { segment })?;
    }
    Ok(n)
}
