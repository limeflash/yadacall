use std::collections::HashSet;

use yc_core::{meeting::MeetingId, transcript::Turn};

/// Speaker-aware text chunk used by lexical and vector search.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchChunk {
    /// Parent meeting identifier.
    pub meeting_id: MeetingId,
    /// First included turn's start time.
    pub start_ms: u64,
    /// Last included turn's end time.
    pub end_ms: u64,
    /// Display names represented in this chunk.
    pub speakers: Vec<String>,
    /// Turn text with a speaker name prefix on each line.
    pub text: String,
}

/// Divide speaker turns into roughly 120–220 word chunks with a one-turn overlap.
pub fn chunk_turns(
    meeting_id: MeetingId,
    turns: &[Turn],
    speaker_names: &[(String, String)],
) -> Vec<SearchChunk> {
    if turns.is_empty() {
        return Vec::new();
    }
    let names = speaker_names
        .iter()
        .cloned()
        .collect::<std::collections::HashMap<_, _>>();
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < turns.len() {
        let mut end = start;
        let mut words = 0;
        while end < turns.len() {
            words += turns[end].text.split_whitespace().count();
            end += 1;
            if words >= 160 || (words >= 120 && end < turns.len()) || words >= 220 {
                break;
            }
        }
        if end == start {
            end += 1;
        }
        let included = &turns[start..end];
        let mut seen = HashSet::new();
        let speakers = included
            .iter()
            .filter(|turn| seen.insert(turn.speaker.clone()))
            .map(|turn| {
                names
                    .get(&turn.speaker)
                    .cloned()
                    .unwrap_or_else(|| default_speaker_name(&turn.speaker))
            })
            .collect();
        let text = included
            .iter()
            .map(|turn| {
                let speaker = names
                    .get(&turn.speaker)
                    .cloned()
                    .unwrap_or_else(|| default_speaker_name(&turn.speaker));
                format!("{speaker}: {}", turn.text.trim())
            })
            .collect::<Vec<_>>()
            .join("\n");
        chunks.push(SearchChunk {
            meeting_id,
            start_ms: included.first().map_or(0, |turn| turn.start_ms),
            end_ms: included.last().map_or(0, |turn| turn.end_ms),
            speakers,
            text,
        });
        if end == turns.len() {
            break;
        }
        start = if end - start > 1 { end - 1 } else { end };
    }
    chunks
}

fn default_speaker_name(speaker: &str) -> String {
    match speaker {
        "me" => "Я".to_owned(),
        "them" => "Собеседник".to_owned(),
        _ => speaker.to_owned(),
    }
}
