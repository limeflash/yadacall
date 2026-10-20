use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

use chrono::{DateTime, Utc};
use serde_json::Value;
use yc_core::{Result, meeting::Bookmark};
use yc_llm::{
    ChatProvider, ChatRequest, ChatResponse, ChatStream, LocalServer, LocalServerConfig,
    LocalServerProgress, SummaryInput, SummaryOptions, SummaryTemplate, SummaryTurn, Timings,
    summarize,
};

#[derive(Default)]
struct TimingCollector(Mutex<Vec<Timings>>);

struct RecordingProvider {
    inner: Box<dyn ChatProvider>,
    timings: Arc<TimingCollector>,
}

#[async_trait::async_trait]
impl ChatProvider for RecordingProvider {
    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse> {
        let response = self.inner.complete(request).await?;
        if let Ok(mut timings) = self.timings.0.lock() {
            timings.push(response.timings);
        }
        Ok(response)
    }
    async fn stream(&self, request: ChatRequest) -> Result<ChatStream> {
        self.inner.stream(request).await
    }
    fn provider_id(&self) -> &str {
        self.inner.provider_id()
    }
    fn model(&self) -> &str {
        self.inner.model()
    }
    async fn count_tokens(&self, text: &str) -> Result<Option<usize>> {
        self.inner.count_tokens(text).await
    }
}

#[tokio::test]
#[ignore = "requires a downloaded llama-server runtime and Gemma 4 GGUF under YC_MODELS_DIR"]
async fn local_gemma4_summarizes_two_minute_russian_meeting() {
    let root = std::env::var_os("YC_MODELS_DIR")
        .map(PathBuf::from)
        .expect("set YC_MODELS_DIR to the developer model cache root");
    let model =
        find_named(&root, "gemma-4-E2B-it-Q4_0.gguf").expect("Gemma 4 Q4 GGUF is not installed");
    let server_binary = find_named(
        &root,
        if cfg!(windows) {
            "llama-server.exe"
        } else {
            "llama-server"
        },
    )
    .expect("llama-server runtime is not installed");
    let mut config = LocalServerConfig::new(server_binary.parent().expect("binary parent"), &model);
    config.startup_timeout = std::time::Duration::from_secs(300);
    let server = LocalServer::start(config, |progress| match progress {
        LocalServerProgress::Starting => eprintln!("starting llama-server"),
        LocalServerProgress::Loading => eprintln!("loading Gemma 4"),
        LocalServerProgress::Ready { port } => eprintln!("llama-server ready on {port}"),
    })
    .await
    .expect("start llama-server");
    let provider = server.provider().expect("local provider");
    let timings = Arc::new(TimingCollector::default());
    let recording = RecordingProvider {
        inner: provider,
        timings: timings.clone(),
    };
    let value: Value =
        serde_json::from_str(include_str!("fixtures/ru_demo.json")).expect("fixture JSON");
    let meeting = &value["meeting"];
    let started_at: DateTime<Utc> =
        DateTime::parse_from_rfc3339(meeting["startedAt"].as_str().expect("meeting date"))
            .expect("RFC3339 date")
            .with_timezone(&Utc);
    let participants = meeting["participants"]
        .as_array()
        .expect("participants")
        .iter()
        .filter_map(|participant| {
            Some((
                participant["key"].as_str()?.to_owned(),
                participant["name"].as_str()?.to_owned(),
            ))
        })
        .collect::<std::collections::HashMap<_, _>>();
    let turns = value["transcript"]["segments"]
        .as_array()
        .expect("segments")
        .iter()
        .map(|segment| {
            let speaker = segment["speaker"].as_str().expect("speaker");
            SummaryTurn {
                speaker: participants
                    .get(speaker)
                    .cloned()
                    .unwrap_or_else(|| speaker.to_owned()),
                start_ms: segment["startMs"].as_u64().expect("start ms"),
                text: segment["text"].as_str().expect("text").to_owned(),
            }
        })
        .collect::<Vec<_>>();
    let bookmarks = meeting["bookmarks"]
        .as_array()
        .expect("bookmarks")
        .iter()
        .map(|bookmark| Bookmark {
            id: bookmark["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| "fixture-bookmark".to_owned()),
            at_ms: bookmark["atMs"].as_u64().expect("bookmark time"),
            label: bookmark["label"].as_str().map(str::to_owned),
            created_after: bookmark["createdAfter"].as_bool(),
            screenshot: bookmark["screenshot"].as_str().map(str::to_owned),
        })
        .collect();
    let input = SummaryInput {
        meeting: yc_llm::MeetingMetadata {
            id: None,
            title: meeting["title"].as_str().expect("meeting title").to_owned(),
            started_at,
            duration_ms: meeting["durationMs"].as_u64().expect("duration"),
        },
        turns,
        bookmarks,
        language: None,
        template: SummaryTemplate::Standard,
        meeting_language: "ru".to_owned(),
    };
    let started = Instant::now();
    let summary = summarize(&recording, &input, &SummaryOptions::default(), |_| {})
        .await
        .expect("Gemma 4 summary");
    let elapsed = started.elapsed();
    let timings = timings.0.lock().expect("timings lock").clone();
    let prompt_rate = timings
        .iter()
        .filter_map(|timing| timing.prompt_tokens_per_second)
        .next_back();
    let decode_rate = timings
        .iter()
        .filter_map(|timing| timing.decode_tokens_per_second)
        .next_back();
    eprintln!(
        "wall time: {:.2}s; prompt tok/s: {:?}; decode tok/s: {:?}",
        elapsed.as_secs_f64(),
        prompt_rate,
        decode_rate
    );
    eprintln!(
        "produced summary JSON:\n{}",
        serde_json::to_string_pretty(&summary).expect("serialize summary")
    );
    assert!(!summary.about.is_empty());
}

fn find_named(root: &Path, name: &str) -> Option<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(directory).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|candidate| candidate == name) {
                return Some(path);
            }
            if path.is_dir() {
                pending.push(path);
            }
        }
    }
    None
}
