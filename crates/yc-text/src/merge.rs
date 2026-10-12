use yc_core::{
    meeting::{Meeting, TrackKind},
    transcript::{SPEAKER_ME, SPEAKER_THEM, Segment, Turn},
};

/// Align both recognized tracks on the meeting timeline and sort by speech start.
pub fn merge_tracks(mic: &[Segment], them: &[Segment], meeting: &Meeting) -> Vec<Segment> {
    let mic_offset = track_offset(meeting, TrackKind::Mic);
    let them_offset = track_offset(meeting, TrackKind::System);
    let mut merged = Vec::with_capacity(mic.len() + them.len());
    append_track(&mut merged, mic, TrackKind::Mic, mic_offset);
    append_track(&mut merged, them, TrackKind::System, them_offset);
    merged.sort_by_key(|segment| segment.start_ms);
    merged
}

fn track_offset(meeting: &Meeting, kind: TrackKind) -> u64 {
    meeting
        .tracks
        .iter()
        .find(|track| track.kind == kind)
        .map_or(0, |track| track.offset_ms)
}

fn append_track(output: &mut Vec<Segment>, segments: &[Segment], kind: TrackKind, offset: u64) {
    for segment in segments {
        let mut shifted = segment.clone();
        shifted.track = kind;
        shifted.start_ms = shifted.start_ms.saturating_add(offset);
        shifted.end_ms = shifted.end_ms.saturating_add(offset);
        for word in &mut shifted.words {
            word.start_ms = word.start_ms.saturating_add(offset);
            word.end_ms = word.end_ms.saturating_add(offset);
        }
        if shifted.speaker.is_empty()
            || matches!(shifted.speaker.as_str(), SPEAKER_ME | SPEAKER_THEM)
        {
            shifted.speaker = match kind {
                TrackKind::Mic => SPEAKER_ME,
                TrackKind::System => SPEAKER_THEM,
            }
            .to_owned();
        }
        output.push(shifted);
    }
}

/// Combine adjacent segments from the same speaker into dialogue turns.
pub fn build_turns(segments: &[Segment]) -> Vec<Turn> {
    let mut ordered = segments.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|segment| segment.start_ms);
    let mut turns: Vec<Turn> = Vec::new();
    for segment in ordered {
        if let Some(previous) = turns.last_mut()
            && previous.speaker == segment.speaker
            && segment.start_ms.saturating_sub(previous.end_ms) < 1_500
        {
            previous.end_ms = previous.end_ms.max(segment.end_ms);
            if !previous.text.is_empty() && !segment.text.is_empty() {
                previous.text.push(' ');
            }
            previous.text.push_str(&segment.text);
            continue;
        }
        turns.push(Turn {
            speaker: segment.speaker.clone(),
            start_ms: segment.start_ms,
            end_ms: segment.end_ms,
            text: segment.text.clone(),
        });
    }
    turns
}
