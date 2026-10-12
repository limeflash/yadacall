//! Turns a stream of audio blocks into closed speech regions with their audio, for both the live
//! path (frames from the recorder) and the offline path (pcm read from disk).

use yc_core::Result;
use yc_core::SAMPLE_RATE;
use yc_core::meeting::TrackKind;
use yc_core::ports::VoiceActivityDetector;

/// A closed region of speech with its samples. Positions are absolute sample indices in the track
/// file (16 kHz).
#[derive(Debug, Clone)]
pub struct Region {
    pub track: TrackKind,
    pub start: u64,
    pub end: u64,
    pub pcm: Vec<f32>,
}

/// Audio kept behind the VAD so a region that closes late can still be cut out (max region ≈ 20 s
/// plus trailing silence).
const KEEP_SAMPLES: usize = 60 * SAMPLE_RATE as usize;
const TRIM_AT: usize = 2 * KEEP_SAMPLES;

/// Feeds a VAD and cuts the regions it reports out of a rolling buffer.
pub struct RegionExtractor {
    track: TrackKind,
    vad: Box<dyn VoiceActivityDetector>,
    /// Absolute index of the first sample the VAD saw.
    origin: u64,
    buf: Vec<f32>,
    /// Absolute index of `buf[0]`.
    buf_start: u64,
}

impl RegionExtractor {
    /// `origin` is the absolute sample index of the first sample that will be fed.
    pub fn new(track: TrackKind, vad: Box<dyn VoiceActivityDetector>, origin: u64) -> Self {
        Self {
            track,
            vad,
            origin,
            buf: Vec::new(),
            buf_start: origin,
        }
    }

    /// Absolute index one past the last fed sample.
    pub fn position(&self) -> u64 {
        self.buf_start + self.buf.len() as u64
    }

    /// Feeds contiguous audio; returns regions that closed.
    pub fn feed(&mut self, samples: &[f32]) -> Result<Vec<Region>> {
        self.buf.extend_from_slice(samples);
        let closed = self.vad.push(samples)?;
        let regions = closed
            .into_iter()
            .map(|r| self.cut(r.start, r.end))
            .collect();
        self.trim();
        Ok(regions)
    }

    /// Feeds `n` samples of silence (a gap in the input) without keeping them in the buffer twice.
    pub fn feed_silence(&mut self, n: usize) -> Result<Vec<Region>> {
        self.feed(&vec![0.0; n])
    }

    /// Closes the open region at end of stream.
    pub fn flush(&mut self) -> Vec<Region> {
        let closed = self.vad.flush();
        closed
            .into_iter()
            .map(|r| self.cut(r.start, r.end))
            .collect()
    }

    fn cut(&self, start: u64, end: u64) -> Region {
        let start = (self.origin + start).max(self.buf_start);
        let end = (self.origin + end).min(self.position()).max(start);
        let a = (start - self.buf_start) as usize;
        let b = (end - self.buf_start) as usize;
        Region {
            track: self.track,
            start,
            end,
            pcm: self.buf[a..b].to_vec(),
        }
    }

    fn trim(&mut self) {
        if self.buf.len() > TRIM_AT {
            let drop = self.buf.len() - KEEP_SAMPLES;
            self.buf.drain(..drop);
            self.buf_start += drop as u64;
        }
    }
}

/// Splits `pcm` into chunks of at most `max_samples`, cutting each at the quietest 50 ms in the
/// last 40 % of the window so words are rarely cut in half. Returns `(offset, len)` pairs.
pub fn split_chunks(pcm: &[f32], max_samples: usize) -> Vec<(usize, usize)> {
    let max_samples = max_samples.max(SAMPLE_RATE as usize);
    let mut out = Vec::new();
    let mut start = 0usize;
    while pcm.len() - start > max_samples {
        let window_end = start + max_samples;
        let search_from = start + max_samples * 6 / 10;
        let win = (SAMPLE_RATE as usize) / 20;
        let hop = (win / 2).max(1);
        let mut best = (f32::MAX, window_end);
        let mut pos = search_from;
        while pos + win <= window_end {
            let energy: f32 = pcm[pos..pos + win].iter().map(|s| s * s).sum();
            if energy < best.0 {
                best = (energy, pos + win / 2);
            }
            pos += hop;
        }
        out.push((start, best.1 - start));
        start = best.1;
    }
    if start < pcm.len() {
        out.push((start, pcm.len() - start));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::EnergyVad;

    #[test]
    fn cuts_regions_with_absolute_positions() {
        let mut ex = RegionExtractor::new(TrackKind::Mic, Box::new(EnergyVad::default()), 16_000);
        let mut audio = vec![0.0; 8_000];
        audio.extend(vec![0.3; 16_000]);
        audio.extend(vec![0.0; 16_000]);
        let mut regions = ex.feed(&audio).unwrap();
        regions.extend(ex.flush());
        assert_eq!(regions.len(), 1);
        let r = &regions[0];
        // The VAD works on 512-sample windows, so the start is within one window of the truth.
        assert!(
            (16_000 + 7_680..=16_000 + 8_000).contains(&r.start),
            "{}",
            r.start
        );
        assert!(r.end - r.start >= 16_000);
        assert_eq!(r.pcm.len() as u64, r.end - r.start);
        assert!(r.pcm.iter().any(|&s| (s - 0.3).abs() < 1e-6));
        assert_eq!(ex.position(), 16_000 + audio.len() as u64);
    }

    #[test]
    fn splits_at_quiet_points() {
        // 3 s of tone with a dip around 2.2 s; max chunk 2.5 s -> cut near the dip.
        let mut pcm = vec![0.5f32; 48_000];
        for s in &mut pcm[35_000..36_000] {
            *s = 0.0;
        }
        let chunks = split_chunks(&pcm, 40_000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks.iter().map(|c| c.1).sum::<usize>(), pcm.len());
        let cut = chunks[0].1;
        assert!((35_000..36_000).contains(&cut), "cut at {cut}");
        assert_eq!(split_chunks(&pcm[..1000], 40_000), vec![(0, 1000)]);
    }
}
