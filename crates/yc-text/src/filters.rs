use std::collections::HashMap;

use yc_core::{meeting::TrackKind, transcript::Segment};

use crate::normalize::normalize_tokens;

const HALLUCINATIONS: &str = include_str!("data/hallucinations.txt");

/// Echo matching thresholds and the maximum tolerated timing distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EchoFilterConfig {
    /// Fraction of microphone tokens that must occur in the system segment (0.0..=1.0).
    pub word_overlap_threshold: f32,
    /// Expand each segment's interval by this amount when comparing likely echo timing.
    pub timing_tolerance_ms: u64,
}

impl Default for EchoFilterConfig {
    fn default() -> Self {
        Self {
            word_overlap_threshold: 0.7,
            timing_tolerance_ms: 1_000,
        }
    }
}

/// Transcript and source indices for microphone segments removed as system-audio echo.
#[derive(Debug, Clone, PartialEq)]
pub struct EchoFilterResult {
    /// Segments that remain after echo removal.
    pub kept: Vec<Segment>,
    /// Removed segments with their source and matching system segment indices.
    pub dropped: Vec<DroppedEcho>,
}

/// An echo candidate and the system segment that matched it.
#[derive(Debug, Clone, PartialEq)]
pub struct DroppedEcho {
    /// Index of the removed microphone segment in the input slice.
    pub segment_index: usize,
    /// Index of the system segment that matched the echo.
    pub echoed_by_index: usize,
    /// Removed segment data for debugging and UI review.
    pub segment: Segment,
}

/// Filter duplicated microphone speech that is already present in a nearby system segment.
pub fn filter_echo_duplicates(segments: &[Segment], config: EchoFilterConfig) -> EchoFilterResult {
    let threshold = config.word_overlap_threshold.clamp(0.0, 1.0);
    let system_segments = segments
        .iter()
        .enumerate()
        .filter(|(_, segment)| segment.track == TrackKind::System || segment.speaker == "them")
        .collect::<Vec<_>>();
    let mut echoed_by = HashMap::new();

    for (index, segment) in segments.iter().enumerate() {
        if segment.track != TrackKind::Mic && segment.speaker != "me" {
            continue;
        }
        let words = normalize_tokens(&segment.text);
        if words.is_empty() {
            continue;
        }
        for (other_index, other) in &system_segments {
            let near = segment.start_ms <= other.end_ms.saturating_add(config.timing_tolerance_ms)
                && other.start_ms <= segment.end_ms.saturating_add(config.timing_tolerance_ms);
            if !near {
                continue;
            }
            let contained = multiset_overlap(&words, &normalize_tokens(&other.text));
            if contained as f32 / words.len() as f32 >= threshold {
                echoed_by.insert(index, *other_index);
                break;
            }
        }
    }

    let mut kept = Vec::with_capacity(segments.len());
    let mut dropped = Vec::with_capacity(echoed_by.len());
    for (index, segment) in segments.iter().enumerate() {
        if let Some(echoed_by_index) = echoed_by.get(&index) {
            dropped.push(DroppedEcho {
                segment_index: index,
                echoed_by_index: *echoed_by_index,
                segment: segment.clone(),
            });
        } else {
            kept.push(segment.clone());
        }
    }
    EchoFilterResult { kept, dropped }
}

fn multiset_overlap(source: &[String], target: &[String]) -> usize {
    let mut counts = HashMap::<&str, usize>::new();
    for token in target {
        *counts.entry(token).or_default() += 1;
    }
    source
        .iter()
        .filter(|token| {
            if let Some(count) = counts.get_mut(token.as_str())
                && *count > 0
            {
                *count -= 1;
                return true;
            }
            false
        })
        .count()
}

/// Confidence gate and token-loop cleanup controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HallucinationConfig {
    /// Drop segments with an available confidence below this value.
    pub min_confidence: Option<f32>,
    /// Maximum repeated n-gram width that is collapsed in a segment.
    pub max_ngram_words: usize,
}

impl Default for HallucinationConfig {
    fn default() -> Self {
        Self {
            min_confidence: Some(0.15),
            max_ngram_words: 12,
        }
    }
}

/// Why a segment was removed by the hallucination filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HallucinationReason {
    /// The text matched an embedded ASR hallucination phrase.
    StopList,
    /// Available confidence was lower than the configured threshold.
    LowConfidence,
    /// Loop cleanup removed all of the segment's words.
    EmptyAfterLoopCut,
    /// An adjacent repeated phrase was collapsed to its first occurrence.
    RepeatedPhrase,
}

/// Debug record for a segment removed by the hallucination filter.
#[derive(Debug, Clone, PartialEq)]
pub struct HallucinationDrop {
    /// Index of the removed segment in the input slice.
    pub segment_index: usize,
    /// Filter rule responsible for removing the segment.
    pub reason: HallucinationReason,
    /// Removed segment data for debugging and UI review.
    pub segment: Segment,
}

/// Kept transcript plus dropped segments and loop-cleaned text.
#[derive(Debug, Clone, PartialEq)]
pub struct FilteredTranscript {
    /// Remaining segments, including segments with cleaned loop text.
    pub kept: Vec<Segment>,
    /// Removed segments and their reasons.
    pub dropped: Vec<HallucinationDrop>,
}

/// Remove common ASR fabrications and low-confidence stretches, then cut repeated n-gram loops.
pub fn filter_hallucinations(
    segments: &[Segment],
    config: HallucinationConfig,
) -> FilteredTranscript {
    let stop_phrases = HALLUCINATIONS
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(normalize_tokens)
        .collect::<Vec<_>>();
    let mut candidates = Vec::with_capacity(segments.len());
    let mut dropped = Vec::new();

    for (index, original) in segments.iter().enumerate() {
        let normalized = normalize_tokens(&original.text);
        let stop_match = stop_phrases
            .iter()
            .any(|phrase| !phrase.is_empty() && contains_phrase(&normalized, phrase));
        let low_confidence = config.min_confidence.is_some_and(|threshold| {
            original
                .confidence
                .is_some_and(|confidence| confidence < threshold)
        });
        let reason = if stop_match {
            Some(HallucinationReason::StopList)
        } else if low_confidence {
            Some(HallucinationReason::LowConfidence)
        } else {
            None
        };
        if let Some(reason) = reason {
            dropped.push(HallucinationDrop {
                segment_index: index,
                reason,
                segment: original.clone(),
            });
            continue;
        }

        let mut segment = original.clone();
        segment.text = cut_ngram_loops(&segment.text, config.max_ngram_words.max(1));
        if normalize_tokens(&segment.text).is_empty() {
            dropped.push(HallucinationDrop {
                segment_index: index,
                reason: HallucinationReason::EmptyAfterLoopCut,
                segment: original.clone(),
            });
        } else {
            candidates.push((index, original.clone(), segment));
        }
    }

    let mut kept = Vec::with_capacity(candidates.len());
    let mut cursor = 0;
    while cursor < candidates.len() {
        let (_, _, cleaned) = &candidates[cursor];
        let phrase = normalize_tokens(&cleaned.text);
        let mut run_end = cursor + 1;
        if phrase.len() >= 2 {
            while run_end < candidates.len() {
                let (_, _, previous) = &candidates[run_end - 1];
                let (_, _, next) = &candidates[run_end];
                let gap = next.start_ms.saturating_sub(previous.end_ms);
                if previous.speaker != next.speaker
                    || previous.track != next.track
                    || gap >= 1_500
                    || normalize_tokens(&next.text) != phrase
                {
                    break;
                }
                run_end += 1;
            }
        }
        if run_end - cursor >= 3 {
            kept.push(cleaned.clone());
            for (repeated_index, repeated_original, _) in &candidates[cursor + 1..run_end] {
                dropped.push(HallucinationDrop {
                    segment_index: *repeated_index,
                    reason: HallucinationReason::RepeatedPhrase,
                    segment: repeated_original.clone(),
                });
            }
            cursor = run_end;
        } else {
            kept.push(cleaned.clone());
            cursor += 1;
        }
    }
    dropped.sort_by_key(|drop| drop.segment_index);
    FilteredTranscript { kept, dropped }
}

fn contains_phrase(haystack: &[String], needle: &[String]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[derive(Clone)]
struct WordSpan {
    start: usize,
    end: usize,
    token: String,
}

fn word_spans(text: &str) -> Vec<WordSpan> {
    let mut words = Vec::new();
    let mut start = None;
    for (index, character) in text.char_indices() {
        if character.is_alphanumeric() {
            start.get_or_insert(index);
        } else if let Some(word_start) = start.take() {
            words.push(WordSpan {
                start: word_start,
                end: index,
                token: normalize_tokens(&text[word_start..index])
                    .into_iter()
                    .next()
                    .unwrap_or_default(),
            });
        }
    }
    if let Some(word_start) = start {
        words.push(WordSpan {
            start: word_start,
            end: text.len(),
            token: normalize_tokens(&text[word_start..])
                .into_iter()
                .next()
                .unwrap_or_default(),
        });
    }
    words
}

fn cut_ngram_loops(text: &str, max_words: usize) -> String {
    let words = word_spans(text);
    if words.len() < 4 {
        return text.to_owned();
    }
    let mut remove = Vec::<(usize, usize)>::new();
    let mut cursor = 0;
    while cursor < words.len() {
        let remaining = words.len() - cursor;
        let max = max_words.min(remaining / 2);
        let mut selected = None;
        for width in (1..=max).rev() {
            let mut repeats = 1;
            while cursor + (repeats + 1) * width <= words.len()
                && (0..width).all(|offset| {
                    words[cursor + offset].token == words[cursor + repeats * width + offset].token
                })
            {
                repeats += 1;
            }
            if repeats >= 2 {
                selected = Some((width, repeats));
                break;
            }
        }
        if let Some((width, repeats)) = selected {
            let from = words[cursor + width].start;
            let to = words[cursor + width * repeats - 1].end;
            remove.push((from, to));
            cursor += width * repeats;
        } else {
            cursor += 1;
        }
    }
    if remove.is_empty() {
        return text.to_owned();
    }
    let mut output = String::with_capacity(text.len());
    let mut last = 0;
    for (from, to) in remove {
        output.push_str(&text[last..from]);
        last = to;
    }
    output.push_str(&text[last..]);
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}
