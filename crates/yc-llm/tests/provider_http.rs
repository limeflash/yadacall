use std::{io, time::Duration};

use chrono::{TimeZone, Utc};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use yc_core::{
    Error,
    meeting::{Bookmark, MeetingId},
};
use yc_llm::{
    AnthropicProvider, ChatMessage, ChatProvider, ChatRequest, ContextHit, MeetingMetadata,
    OpenAiCompatibleProvider, SummaryInput, SummaryOptions, SummaryTemplate, SummaryTurn,
    answer_question, summarize,
};

#[derive(Clone)]
struct Reply {
    status: u16,
    content_type: &'static str,
    chunks: Vec<Vec<u8>>,
}

#[derive(Debug)]
struct CapturedRequest {
    path: String,
    headers: String,
    body: Value,
}

async fn mock_server_with_replies(
    replies: Vec<Reply>,
) -> (String, JoinHandle<Vec<CapturedRequest>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server");
    let address = listener.local_addr().expect("mock address");
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for reply in replies {
            let (mut stream, _) = listener.accept().await.expect("accept HTTP request");
            requests.push(
                read_request_with_writer(&mut stream)
                    .await
                    .expect("read HTTP request"),
            );
            send_reply(&mut stream, reply)
                .await
                .expect("write HTTP response");
        }
        requests
    });
    (format!("http://{address}"), task)
}

async fn read_request_with_writer(stream: &mut TcpStream) -> io::Result<CapturedRequest> {
    let mut request_line = String::new();
    let mut reader = BufReader::new(stream);
    reader.read_line(&mut request_line).await?;
    let mut headers = String::new();
    let mut content_length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse::<usize>().unwrap_or_default();
        }
        headers.push_str(&line);
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).await?;
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    let body = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    Ok(CapturedRequest {
        path,
        headers,
        body,
    })
}

async fn send_reply(stream: &mut TcpStream, reply: Reply) -> io::Result<()> {
    let reason = match reply.status {
        200 => "OK",
        400 => "Bad Request",
        429 => "Too Many Requests",
        _ => "Mock",
    };
    stream.write_all(format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        reply.status, reason, reply.content_type
    ).as_bytes()).await?;
    for chunk in reply.chunks {
        stream
            .write_all(format!("{:X}\r\n", chunk.len()).as_bytes())
            .await?;
        stream.write_all(&chunk).await?;
        stream.write_all(b"\r\n").await?;
        stream.flush().await?;
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    stream.write_all(b"0\r\n\r\n").await?;
    stream.shutdown().await
}

fn json_reply(body: Value) -> Reply {
    Reply {
        status: 200,
        content_type: "application/json",
        chunks: vec![serde_json::to_vec(&body).expect("serialize body")],
    }
}

fn event_stream(chunks: Vec<Vec<u8>>) -> Reply {
    Reply {
        status: 200,
        content_type: "text/event-stream",
        chunks,
    }
}

#[tokio::test]
async fn anthropic_refusal_is_an_error_and_fallback_settings_are_sent() {
    let (base, server) = mock_server_with_replies(vec![json_reply(json!({
        "stop_reason": "refusal",
        "stop_details": { "category": "safety" },
        "content": [{ "type": "text", "text": "not an answer" }]
    }))])
    .await;
    let provider = AnthropicProvider::with_base_url("secret-test", "claude-sonnet-5-5", &base)
        .expect("provider");
    let request = ChatRequest {
        system: "system".into(),
        messages: vec![ChatMessage::user("question")],
        max_tokens: 1_000,
        temperature: Some(0.1),
        json_schema: None,
        stop: Vec::new(),
    };
    let error = provider.complete(request).await.expect_err("refusal");
    assert!(matches!(error, Error::Llm(message) if message.contains("safety")));
    let requests = server.await.expect("mock server task");
    assert_eq!(requests[0].path, "/v1/messages");
    assert!(
        requests[0]
            .headers
            .to_ascii_lowercase()
            .contains("x-api-key: secret-test")
    );
    assert!(
        requests[0]
            .headers
            .to_ascii_lowercase()
            .contains("anthropic-beta: server-side-fallback-2026-07-01")
    );
    assert_eq!(requests[0].body["fallbacks"], "default");
    assert!(requests[0].body.get("temperature").is_none());
}

#[tokio::test]
async fn anthropic_structured_output_uses_strict_schema_and_medium_effort() {
    let response = json_reply(json!({
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 11, "output_tokens": 4 },
        "content": [
            { "type": "thinking", "thinking": "must not be returned" },
            { "type": "text", "text": "{}" }
        ]
    }));
    let (base, server) = mock_server_with_replies(vec![response]).await;
    let provider = AnthropicProvider::with_base_url("secret-test", "claude-opus-5-5", &base).expect("provider");
    let request = ChatRequest {
        system: "system".into(),
        messages: vec![ChatMessage::user("summarize")],
        max_tokens: 8_000,
        temperature: Some(0.4),
        json_schema: Some(yc_llm::JsonSchema {
            name: "sample".into(),
            schema: json!({ "type": "object", "properties": { "answer": { "type": "string" } } }),
        }),
        stop: Vec::new(),
    };
    let response = provider.complete(request).await.expect("completion");
    assert_eq!(response.text, "{}");
    assert_eq!(response.usage.input_tokens, Some(11));
    let requests = server.await.expect("mock server task");
    let body = &requests[0].body;
    assert_eq!(body["output_config"]["effort"], "medium");
    assert_eq!(body["output_config"]["format"]["type"], "json_schema");
    assert_eq!(body["output_config"]["format"]["schema"]["additionalProperties"], false);
    assert_eq!(body["output_config"]["format"]["schema"]["required"], json!(["answer"]));
    for forbidden in ["temperature", "top_p", "thinking", "budget_tokens"] {
        assert!(body.get(forbidden).is_none(), "unexpected body field `{forbidden}`");
    }
    assert!(requests[0].headers.to_ascii_lowercase().contains("anthropic-beta: server-side-fallback-2026-07-01"));
    assert_eq!(body["fallbacks"], "default");
}

#[tokio::test]
async fn anthropic_sse_stream_decodes_utf8_split_between_http_chunks() {
    let payload = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Привет 🌍\"}}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
    );
    let bytes = payload.as_bytes();
    let split = bytes
        .iter()
        .position(|byte| *byte == 0xd0)
        .expect("Cyrillic byte")
        + 1;
    let reply = event_stream(vec![bytes[..split].to_vec(), bytes[split..].to_vec()]);
    let (base, server) = mock_server_with_replies(vec![reply]).await;
    let provider = AnthropicProvider::with_base_url("secret-test", "claude-haiku-5-5", &base)
        .expect("provider");
    let mut stream = provider
        .stream(ChatRequest::new(
            "system",
            vec![ChatMessage::user("question")],
            1_000,
        ))
        .await
        .expect("stream");
    use futures_util::StreamExt;
    let mut text = String::new();
    let mut output_tokens = None;
    while let Some(delta) = stream.next().await {
        let delta = delta.expect("stream delta");
        text.push_str(&delta.text);
        output_tokens = delta
            .usage
            .and_then(|usage| usage.output_tokens)
            .or(output_tokens);
    }
    assert_eq!(text, "Привет 🌍");
    assert_eq!(output_tokens, Some(2));
    let _ = server.await.expect("mock server task");
}

#[tokio::test]
async fn structured_summary_retries_once_after_schema_violation() {
    let invalid = json!({"choices":[{"message":{"content":"not json"},"finish_reason":"stop"}]});
    let valid = json!({"choices":[{"message":{"content":r#"{"title":"План","about":["Обсудили план"],"decisions":[{"text":"Запустить пилот","at":"00:12"}],"tasks":[{"text":"Отправить план","owner":"Ирина","due":"2026-10-14","at":"00:18"}],"open_questions":[]}"#},"finish_reason":"stop"}]});
    let (base, server) = mock_server_with_replies(vec![
        json_reply(json!({ "tokens": [1] })),
        json_reply(invalid),
        json_reply(valid),
    ])
    .await;
    let provider =
        OpenAiCompatibleProvider::new(&base, None, "local", "test-model").expect("provider");
    let input = SummaryInput {
        meeting: MeetingMetadata {
            id: None,
            title: "Планирование".into(),
            started_at: Utc
                .with_ymd_and_hms(2026, 10, 12, 10, 0, 0)
                .single()
                .expect("date"),
            duration_ms: 30_000,
        },
        turns: vec![SummaryTurn {
            speaker: "Ирина".into(),
            start_ms: 12_000,
            text: "Давайте запускаем пилот".into(),
        }],
        bookmarks: vec![Bookmark {
            at_ms: 15_000,
            label: Some("план".into()),
        }],
        language: None,
        template: SummaryTemplate::Standard,
        meeting_language: "ru".into(),
    };
    let summary = summarize(&provider, &input, &SummaryOptions::default(), |_| {})
        .await
        .expect("summary");
    assert_eq!(summary.decisions[0].at_ms, Some(12_000));
    assert_eq!(summary.tasks[0].owner.as_deref(), Some("Ирина"));
    assert_eq!(summary.model, "local:test-model");
    let requests = server.await.expect("mock server task");
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].path, "/tokenize");
    assert_eq!(requests[1].body["response_format"]["type"], "json_schema");
    assert_eq!(
        requests[1].body["response_format"]["json_schema"]["strict"],
        true
    );
    assert!(
        requests[2].body["messages"][1]["content"]
            .as_str()
            .is_some_and(|text| text.contains("prior response did not satisfy"))
    );
    assert!(
        requests[1].body["messages"][1]["content"]
            .as_str()
            .is_some_and(|text| text.contains("BOOKMARK"))
    );
}

#[tokio::test]
async fn openai_sse_answer_returns_source_citation_mapping_and_egress_note() {
    let payload = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Договорились о пятнице [1].\"},\"finish_reason\":null}]}\n\n",
        "data: [DONE]\n\n"
    );
    let (base, server) =
        mock_server_with_replies(vec![event_stream(vec![payload.as_bytes().to_vec()])]).await;
    let provider =
        OpenAiCompatibleProvider::new(&base, None, "local", "test-model").expect("provider");
    let hit = ContextHit {
        meeting_id: MeetingId::new(),
        meeting_title: "План".into(),
        date: "2026-10-12".into(),
        speaker: "Ирина".into(),
        start_ms: 50_000,
        text: "Давайте созвонимся в пятницу".into(),
    };
    let answer = answer_question(&provider, "О чём договорились?", vec![hit.clone()], |_| {})
        .await
        .expect("answer");
    assert_eq!(answer.text, "Договорились о пятнице [1].");
    assert_eq!(answer.citations[0].meeting_id, hit.meeting_id);
    assert_eq!(answer.citations[0].start_ms, 50_000);
    assert_eq!(answer.egress.excerpts_sent, 1);
    assert_eq!(answer.egress.chars_sent, hit.text.chars().count());
    assert!(!answer.egress.audio_sent);
    let _ = server.await.expect("mock server task");
}
