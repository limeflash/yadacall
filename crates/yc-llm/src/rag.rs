//! Excerpt-grounded question answering with streamed text and citation mapping.

use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use yc_core::{Error, Result, meeting::MeetingId};

use crate::types::{ChatMessage, ChatProvider, ChatRequest};

/// One search result passed as evidence to a meeting question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextHit {
    /// Source meeting id.
    pub meeting_id: MeetingId,
    /// Meeting title.
    pub meeting_title: String,
    /// Meeting date, formatted for display.
    pub date: String,
    /// Speaker display name.
    pub speaker: String,
    /// Start position in the meeting timeline.
    pub start_ms: u64,
    /// Transcript excerpt.
    pub text: String,
}

/// Citation target returned with an answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerCitation {
    /// Citation label in the answer.
    pub n: usize,
    /// Source meeting id.
    pub meeting_id: MeetingId,
    /// Excerpt start position.
    pub start_ms: u64,
}

/// Privacy information that lets the UI explain exactly what text left the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressNote {
    /// Provider label shown to the user.
    pub provider: String,
    /// Number of excerpts included in the request.
    pub excerpts_sent: usize,
    /// Characters of excerpt text included in the request.
    pub chars_sent: usize,
    /// Audio is never sent to an LLM provider.
    pub audio_sent: bool,
}

/// Completed streamed answer and its evidence mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    /// Answer text, with citation labels such as `[1]`.
    pub text: String,
    /// Evidence sources cited by the answer.
    pub citations: Vec<AnswerCitation>,
    /// Privacy/egress details for the UI.
    pub egress: EgressNote,
}

/// Stream an evidence-grounded answer. The callback receives each generated text delta.
pub async fn answer_question(
    provider: &dyn ChatProvider,
    question: &str,
    hits: Vec<ContextHit>,
    on_delta: impl Fn(&str) + Send + Sync,
) -> Result<Answer> {
    let russian = contains_cyrillic(question);
    let provider_name = provider_label(provider);
    let excerpt_chars = hits.iter().map(|hit| hit.text.chars().count()).sum();
    let egress = EgressNote {
        provider: provider_name,
        excerpts_sent: hits.len(),
        chars_sent: excerpt_chars,
        audio_sent: false,
    };
    if hits.is_empty() {
        return Ok(Answer {
            text: if russian {
                "В записях этого нет.".to_owned()
            } else {
                "The provided meeting excerpts do not contain that information.".to_owned()
            },
            citations: Vec::new(),
            egress,
        });
    }
    let system = if russian {
        "Отвечай на языке вопроса, используя только приведённые фрагменты расшифровок. Если ответа в них нет, скажи «В записях этого нет». Подкрепляй фактические утверждения ссылками вида [1], используя только номера предоставленных фрагментов. Не додумывай факты и не опирайся на внешние знания."
    } else {
        "Answer in the question's language using only the provided transcript excerpts. If they do not contain the answer, say so plainly. Support factual claims with citations like [1], using only the supplied excerpt numbers. Do not infer missing facts or use outside knowledge."
    };
    let excerpt_text = hits
        .iter()
        .enumerate()
        .map(|(index, hit)| {
            format!(
                "[{}] Meeting: {} ({})\nSpeaker: {}\nAt: {}\nExcerpt: {}",
                index + 1,
                hit.meeting_title,
                hit.date,
                hit.speaker,
                timestamp(hit.start_ms),
                hit.text.trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let request = ChatRequest::new(
        system,
        vec![ChatMessage::user(format!(
            "Question: {question}\n\nTranscript excerpts:\n{excerpt_text}"
        ))],
        1_200,
    );
    let mut stream = provider.stream(request).await?;
    let mut text = String::new();
    while let Some(delta) = stream.next().await {
        let delta = delta?;
        if !delta.text.is_empty() {
            on_delta(&delta.text);
            text.push_str(&delta.text);
        }
    }
    if text.trim().is_empty() {
        return Err(Error::Llm("provider returned an empty answer".to_owned()));
    }
    let citations = map_citations(&text, &hits);
    Ok(Answer {
        text,
        citations,
        egress,
    })
}

fn map_citations(text: &str, hits: &[ContextHit]) -> Vec<AnswerCitation> {
    let mut citations = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'[' {
            index += 1;
            continue;
        }
        let start = index + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end > start && end < bytes.len() && bytes[end] == b']' {
            if let Ok(number) = text[start..end].parse::<usize>()
                && number > 0
                && number <= hits.len()
                && !citations
                    .iter()
                    .any(|citation: &AnswerCitation| citation.n == number)
            {
                let hit = &hits[number - 1];
                citations.push(AnswerCitation {
                    n: number,
                    meeting_id: hit.meeting_id,
                    start_ms: hit.start_ms,
                });
            }
            index = end + 1;
        } else {
            index = start;
        }
    }
    citations
}

fn provider_label(provider: &dyn ChatProvider) -> String {
    if provider.provider_id() == "anthropic" {
        "Claude".to_owned()
    } else if provider.provider_id() == "ollama-cloud" {
        "Ollama Cloud".to_owned()
    } else if provider.provider_id() == "local" {
        "local model".to_owned()
    } else {
        provider.provider_id().to_owned()
    }
}

fn contains_cyrillic(text: &str) -> bool {
    text.chars()
        .any(|ch| ('\u{0400}'..='\u{04ff}').contains(&ch))
}

fn timestamp(ms: u64) -> String {
    let seconds = ms / 1_000;
    if seconds >= 3_600 {
        format!(
            "{:02}:{:02}:{:02}",
            seconds / 3_600,
            (seconds / 60) % 60,
            seconds % 60
        )
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

/// Build a context hit from strongly typed meeting metadata.
pub fn context_hit(
    meeting_id: MeetingId,
    title: impl Into<String>,
    date: DateTime<Utc>,
    speaker: impl Into<String>,
    start_ms: u64,
    text: impl Into<String>,
) -> ContextHit {
    ContextHit {
        meeting_id,
        meeting_title: title.into(),
        date: date.format("%Y-%m-%d").to_string(),
        speaker: speaker.into(),
        start_ms,
        text: text.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citation_numbers_map_to_hits_and_deduplicate() {
        let first = ContextHit {
            meeting_id: MeetingId::new(),
            meeting_title: "A".into(),
            date: "2026-10-12".into(),
            speaker: "me".into(),
            start_ms: 5_000,
            text: "a".into(),
        };
        let second = ContextHit {
            meeting_id: MeetingId::new(),
            meeting_title: "B".into(),
            date: "2026-10-12".into(),
            speaker: "them".into(),
            start_ms: 9_000,
            text: "b".into(),
        };
        let citations = map_citations(
            "The date is Friday [2], and it was repeated [2]. [9]",
            &[first, second.clone()],
        );
        assert_eq!(
            citations,
            vec![AnswerCitation {
                n: 2,
                meeting_id: second.meeting_id,
                start_ms: 9_000
            }]
        );
    }

    #[tokio::test]
    async fn no_hits_returns_a_grounded_empty_answer_without_http() {
        struct Noop;
        #[async_trait::async_trait]
        impl ChatProvider for Noop {
            async fn complete(&self, _request: ChatRequest) -> Result<crate::ChatResponse> {
                Err(Error::Llm("unused".into()))
            }
            async fn stream(&self, _request: ChatRequest) -> Result<crate::ChatStream> {
                Err(Error::Llm("unused".into()))
            }
            fn provider_id(&self) -> &str {
                "local"
            }
            fn model(&self) -> &str {
                "test"
            }
        }
        let answer = answer_question(&Noop, "Что решили?", Vec::new(), |_| {})
            .await
            .expect("answer");
        assert_eq!(answer.text, "В записях этого нет.");
        assert!(answer.citations.is_empty());
        assert!(!answer.egress.audio_sent);
    }
}
