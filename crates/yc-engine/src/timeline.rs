//! Mapping between positions in a track's `.pcm` file and the meeting timeline.
//!
//! A pause writes nothing to the file, so audio after a pause sits earlier in the file than on the
//! wall clock: `timeline = offset + position + Σ durations of pauses at earlier positions`.

use yc_audio::writer::PauseSpan;
use yc_core::transcript::Segment;

/// File position (ms) → ms since the track start on the wall clock (pauses re-inserted).
pub fn file_to_track_time(position_ms: u64, pauses: &[PauseSpan]) -> u64 {
    position_ms
        + pauses
            .iter()
            .filter(|p| p.at_ms <= position_ms)
            .map(|p| p.duration_ms)
            .sum::<u64>()
}

/// Inverse of [`file_to_track_time`]; a time inside a pause maps to the pause position.
pub fn track_to_file_time(track_ms: u64, pauses: &[PauseSpan]) -> u64 {
    let mut shift = 0u64;
    for p in pauses {
        let pause_start = p.at_ms + shift;
        if track_ms < pause_start {
            break;
        }
        if track_ms < pause_start + p.duration_ms {
            return p.at_ms;
        }
        shift += p.duration_ms;
    }
    track_ms - shift
}

/// Converts a segment recognised against the file (times = file positions) to track time.
pub fn segment_to_track_time(segment: &mut Segment, pauses: &[PauseSpan]) {
    if pauses.is_empty() {
        return;
    }
    segment.start_ms = file_to_track_time(segment.start_ms, pauses);
    segment.end_ms = file_to_track_time(segment.end_ms, pauses);
    for w in &mut segment.words {
        w.start_ms = file_to_track_time(w.start_ms, pauses);
        w.end_ms = file_to_track_time(w.end_ms, pauses);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pause(at_ms: u64, duration_ms: u64) -> PauseSpan {
        PauseSpan { at_ms, duration_ms }
    }

    #[test]
    fn maps_around_pauses_and_back() {
        let pauses = [pause(1000, 1500), pause(4000, 500)];
        assert_eq!(file_to_track_time(500, &pauses), 500);
        assert_eq!(file_to_track_time(1000, &pauses), 2500);
        assert_eq!(file_to_track_time(3999, &pauses), 5499);
        assert_eq!(file_to_track_time(4000, &pauses), 6000);
        for t in [0, 999, 1000, 2500, 3999, 5499, 6000, 9000] {
            let file = track_to_file_time(t, &pauses);
            if !(1000..2500).contains(&t) && !(5500..6000).contains(&t) {
                assert_eq!(file_to_track_time(file, &pauses), t, "t={t}");
            }
        }
        assert_eq!(track_to_file_time(1700, &pauses), 1000, "inside a pause");
    }
}
