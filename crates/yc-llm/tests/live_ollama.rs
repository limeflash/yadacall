//! Live check against the user's local Ollama daemon. Never pulls or deletes models.
//!
//! `YC_OLLAMA_LIVE=1 YC_OLLAMA_MODEL=glm-5.2:cloud cargo test -p yc-llm --test live_ollama -- --ignored --nocapture`

use std::time::Instant;

use chrono::{DateTime, Utc};
use serde_json::Value;
use yc_core::meeting::Bookmark;
use yc_llm::{
    ChatMessage, ChatProvider, ChatRequest, MeetingMetadata, OLLAMA_LOCAL_URL, OllamaProvider,
    SummaryInput, SummaryOptions, SummaryTemplate, SummaryTurn, summarize,
};

#[tokio::test]
#[ignore = "needs a running Ollama daemon; set YC_OLLAMA_LIVE=1 and YC_OLLAMA_MODEL"]
async fn ollama_summarises_the_russian_fixture() {
    if std::env::var_os("YC_OLLAMA_LIVE").is_none() {
        eprintln!("YC_OLLAMA_LIVE is not set, skipping");
        return;
    }
    let model = std::env::var("YC_OLLAMA_MODEL").unwrap_or_else(|_| "glm-5.2:cloud".to_owned());
    let probe = OllamaProvider::probe(OLLAMA_LOCAL_URL).await;
    assert!(probe.running, "Ollama is not running");
    eprintln!(
        "daemon {:?}, signed in: {}, models: {:?}",
        probe.version,
        probe.signed_in,
        probe.models.iter().map(|m| &m.name).collect::<Vec<_>>()
    );
    assert!(
        probe.models.iter().any(|m| m.name == model),
        "{model} is not installed; this test never pulls models"
    );
    let provider = OllamaProvider::new(OLLAMA_LOCAL_URL, None, &model).expect("provider");

    let started = Instant::now();
    let hello = provider
        .complete(ChatRequest::new(
            "",
            vec![ChatMessage::user("Скажи одним предложением: готов.")],
            200,
        ))
        .await
        .expect("one-sentence test");
    eprintln!(
        "one-sentence test: {:?} in {:.2}s",
        hello.text,
        started.elapsed().as_secs_f64()
    );

    let value: Value =
        serde_json::from_str(include_str!("fixtures/ru_demo.json")).expect("fixture JSON");
    let meeting = &value["meeting"];
    let participants = meeting["participants"]
        .as_array()
        .expect("participants")
        .iter()
        .filter_map(|p| {
            Some((
                p["key"].as_str()?.to_owned(),
                p["name"].as_str()?.to_owned(),
            ))
        })
        .collect::<std::collections::HashMap<_, _>>();
    let turns = value["transcript"]["segments"]
        .as_array()
        .expect("segments")
        .iter()
        .map(|s| {
            let speaker = s["speaker"].as_str().expect("speaker");
            SummaryTurn {
                speaker: participants
                    .get(speaker)
                    .cloned()
                    .unwrap_or_else(|| speaker.to_owned()),
                start_ms: s["startMs"].as_u64().expect("start"),
                text: s["text"].as_str().expect("text").to_owned(),
            }
        })
        .collect();
    let bookmarks = meeting["bookmarks"]
        .as_array()
        .expect("bookmarks")
        .iter()
        .map(|b| Bookmark {
            id: b["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| "fixture-bookmark".to_owned()),
            at_ms: b["atMs"].as_u64().expect("at"),
            label: b["label"].as_str().map(str::to_owned),
            created_after: b["createdAfter"].as_bool(),
            screenshot: b["screenshot"].as_str().map(str::to_owned),
        })
        .collect();
    let input = SummaryInput {
        meeting: MeetingMetadata {
            id: None,
            title: meeting["title"].as_str().expect("title").to_owned(),
            started_at: DateTime::parse_from_rfc3339(meeting["startedAt"].as_str().expect("date"))
                .expect("RFC3339")
                .with_timezone(&Utc),
            duration_ms: meeting["durationMs"].as_u64().expect("duration"),
        },
        turns,
        bookmarks,
        language: None,
        template: SummaryTemplate::Standard,
        meeting_language: "ru".to_owned(),
    };
    let started = Instant::now();
    let summary = summarize(&provider, &input, &SummaryOptions::default(), |_| {})
        .await
        .expect("summary");
    eprintln!(
        "summary in {:.2}s:\n{}",
        started.elapsed().as_secs_f64(),
        serde_json::to_string_pretty(&summary).expect("json")
    );
    assert!(!summary.about.is_empty());
}
