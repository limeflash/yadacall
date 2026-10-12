//! Raw HTTP client for Anthropic's Messages API.

use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use reqwest::{Client, Response, StatusCode, Url};
use serde_json::{Value, json};
use yc_core::{Error, Result};

use crate::{sse::SseParser, types::*};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// Client for Anthropic's Messages API.
#[derive(Clone)]
pub struct AnthropicProvider {
    client: Client,
    base_url: Url,
    api_key: String,
    model: String,
}

impl std::fmt::Debug for AnthropicProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicProvider")
            .field("base_url", &self.base_url)
            .field("provider_id", &"anthropic")
            .field("model", &self.model)
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

impl AnthropicProvider {
    /// Construct a Messages API provider.
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Result<Self> {
        Self::with_base_url(api_key, model, DEFAULT_BASE_URL)
    }

    /// Construct against a custom base URL, primarily useful for compatible gateways and tests.
    pub fn with_base_url(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl AsRef<str>,
    ) -> Result<Self> {
        let base_url = Url::parse(base_url.as_ref())
            .map_err(|error| Error::Invalid(format!("invalid Anthropic base URL: {error}")))?;
        Ok(Self {
            client: Client::builder().build().map_err(|error| {
                Error::Network(format!("could not create HTTP client: {error}"))
            })?,
            base_url,
            api_key: api_key.into(),
            model: model.into(),
        })
    }

    fn endpoint(&self) -> Url {
        let mut endpoint = self.base_url.clone();
        endpoint.set_path(&format!(
            "{}/v1/messages",
            endpoint.path().trim_end_matches('/')
        ));
        endpoint
    }

    fn payload(&self, request: &ChatRequest, streaming: bool) -> Value {
        let messages = request
            .messages
            .iter()
            .map(|message| {
                let role = match message.role {
                    ChatRole::User => "user",
                    ChatRole::Assistant => "assistant",
                };
                json!({ "role": role, "content": message.content })
            })
            .collect::<Vec<_>>();
        let mut payload = json!({
            "model": self.model,
            "max_tokens": request.max_tokens,
            "system": request.system,
            "messages": messages,
            "stream": streaming,
        });
        if !request.stop.is_empty() {
            payload["stop_sequences"] = json!(request.stop);
        }
        if let Some(schema) = &request.json_schema {
            payload["output_config"] = json!({
                "format": { "type": "json_schema", "schema": strict_schema(schema.schema.clone()) },
                "effort": "medium"
            });
        } else {
            payload["output_config"] = json!({ "effort": "low" });
        }
        if self.supports_server_fallback() {
            payload["fallbacks"] = json!("default");
        }
        payload
    }

    fn supports_server_fallback(&self) -> bool {
        self.model == "claude-sonnet-5-5" || self.model == "claude-opus-5-5"
    }

    fn request_builder(&self, payload: &Value) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .post(self.endpoint())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(payload);
        if self.supports_server_fallback() {
            request = request.header("anthropic-beta", "server-side-fallback-2026-07-01");
        }
        request
    }

    async fn send_with_retries(&self, payload: &Value) -> Result<Response> {
        let mut attempt = 0_u32;
        loop {
            let response = self
                .request_builder(payload)
                .send()
                .await
                .map_err(network_error)?;
            if !should_retry_response(&response) || attempt >= 3 {
                if response.status().is_success() {
                    return Ok(response);
                }
                return Err(response_error(response).await);
            }
            let retry_after = retry_after(&response);
            attempt += 1;
            tokio::time::sleep(retry_after.unwrap_or_else(|| {
                Duration::from_millis(250_u64.saturating_mul(2_u64.pow(attempt - 1)))
            }))
            .await;
        }
    }
}

#[async_trait::async_trait]
impl ChatProvider for AnthropicProvider {
    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse> {
        let response = self
            .send_with_retries(&self.payload(&request, false))
            .await?;
        let value: Value = response
            .json()
            .await
            .map_err(|error| Error::Llm(format!("invalid Anthropic response: {error}")))?;
        parse_message(value)
    }

    async fn stream(&self, request: ChatRequest) -> Result<ChatStream> {
        let response = self
            .send_with_retries(&self.payload(&request, true))
            .await?;
        let mut body = response.bytes_stream();
        let output = async_stream::stream! {
            let mut parser = SseParser::default();
            while let Some(chunk) = body.next().await {
                match chunk {
                    Ok(chunk) => {
                        for event in parser.push(&chunk) {
                            match parse_anthropic_event(&event.event, &event.data) {
                                Ok(Some(delta)) => yield Ok(delta),
                                Ok(None) => {},
                                Err(error) => { yield Err(error); return; }
                            }
                        }
                    }
                    Err(error) => { yield Err(network_error(error)); return; }
                }
            }
            for event in parser.finish() {
                match parse_anthropic_event(&event.event, &event.data) {
                    Ok(Some(delta)) => yield Ok(delta),
                    Ok(None) => {},
                    Err(error) => { yield Err(error); return; }
                }
            }
        };
        Ok(Box::pin(output))
    }

    fn provider_id(&self) -> &str {
        "anthropic"
    }
    fn model(&self) -> &str {
        &self.model
    }
}

fn strict_schema(mut schema: Value) -> Value {
    match &mut schema {
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("object") {
                object.insert("additionalProperties".to_owned(), Value::Bool(false));
                if let Some(properties) = object.get("properties").and_then(Value::as_object) {
                    object.insert(
                        "required".to_owned(),
                        Value::Array(properties.keys().cloned().map(Value::String).collect()),
                    );
                }
            }
            if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
                for child in properties.values_mut() {
                    *child = strict_schema(child.clone());
                }
            }
            if let Some(items) = object.get_mut("items") {
                *items = strict_schema(items.clone());
            }
            if let Some(any_of) = object.get_mut("anyOf").and_then(Value::as_array_mut) {
                for child in any_of {
                    *child = strict_schema(child.clone());
                }
            }
        }
        Value::Array(items) => {
            for child in items {
                *child = strict_schema(child.clone());
            }
        }
        _ => {}
    }
    schema
}

fn parse_message(value: Value) -> Result<ChatResponse> {
    let stop_reason = value["stop_reason"].as_str().map(str::to_owned);
    if stop_reason.as_deref() == Some("refusal") {
        let category = value["stop_details"]["category"]
            .as_str()
            .unwrap_or("unspecified");
        return Err(Error::Llm(format!(
            "Anthropic refused the request ({category})"
        )));
    }
    let content = value["content"]
        .as_array()
        .ok_or_else(|| Error::Llm("Anthropic response has no content".to_owned()))?;
    let text = content
        .iter()
        .filter(|block| block["type"].as_str() == Some("text"))
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("");
    Ok(ChatResponse {
        text,
        usage: TokenUsage {
            input_tokens: value["usage"]["input_tokens"].as_u64(),
            output_tokens: value["usage"]["output_tokens"].as_u64(),
        },
        timings: Timings::default(),
        stop_reason,
    })
}

fn parse_anthropic_event(event_name: &Option<String>, data: &str) -> Result<Option<ChatDelta>> {
    if data.is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(data)
        .map_err(|error| Error::Llm(format!("invalid Anthropic stream event: {error}")))?;
    let kind = event_name
        .as_deref()
        .or_else(|| value["type"].as_str())
        .unwrap_or_default();
    match kind {
        "content_block_delta" => {
            let delta = &value["delta"];
            if delta["type"].as_str() == Some("text_delta") {
                Ok(Some(ChatDelta {
                    text: delta["text"].as_str().unwrap_or_default().to_owned(),
                    stop_reason: None,
                    usage: None,
                }))
            } else {
                Ok(None)
            }
        }
        "message_delta" => {
            let stop_reason = value["delta"]["stop_reason"].as_str().map(str::to_owned);
            if stop_reason.as_deref() == Some("refusal") {
                let category = value["delta"]["stop_details"]["category"]
                    .as_str()
                    .unwrap_or("unspecified");
                return Err(Error::Llm(format!(
                    "Anthropic refused the request ({category})"
                )));
            }
            let output_tokens = value["usage"]["output_tokens"].as_u64();
            Ok(Some(ChatDelta {
                text: String::new(),
                stop_reason,
                usage: output_tokens.map(|output_tokens| TokenUsage {
                    input_tokens: None,
                    output_tokens: Some(output_tokens),
                }),
            }))
        }
        "error" => {
            let kind = value["error"]["type"].as_str().unwrap_or("unknown error");
            let message = value["error"]["message"]
                .as_str()
                .unwrap_or("provider stream failed");
            if kind == "overloaded_error" {
                Err(Error::Network(format!(
                    "Anthropic is overloaded: {message}"
                )))
            } else {
                Err(Error::Llm(format!(
                    "Anthropic stream error ({kind}): {message}"
                )))
            }
        }
        "message_start"
        | "content_block_start"
        | "content_block_stop"
        | "message_stop"
        | "ping"
        | "" => Ok(None),
        _ => Ok(None),
    }
}

fn should_retry_response(response: &Response) -> bool {
    response.status() == StatusCode::TOO_MANY_REQUESTS || response.status().is_server_error()
}

fn retry_after(response: &Response) -> Option<Duration> {
    let value = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?;
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds.min(60)));
    }
    let at = DateTime::parse_from_rfc2822(value)
        .ok()?
        .with_timezone(&Utc);
    Some(
        at.signed_duration_since(Utc::now())
            .to_std()
            .unwrap_or_default()
            .min(Duration::from_secs(60)),
    )
}

async fn response_error(response: Response) -> Error {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let message = format!("Anthropic returned {status}: {}", truncate(&body, 1_000));
    if status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
        || body.contains("overloaded_error")
    {
        Error::Network(message)
    } else {
        Error::Llm(message)
    }
}

fn network_error(error: reqwest::Error) -> Error {
    Error::Network(format!("Anthropic request failed: {error}"))
}

fn truncate(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::{AnthropicProvider, parse_anthropic_event, parse_message, strict_schema};
    use serde_json::json;
    use yc_core::Error;

    #[test]
    fn refusal_is_returned_before_reading_content() {
        let error = parse_message(json!({
            "stop_reason": "refusal",
            "stop_details": { "category": "safety" },
            "content": [{ "type": "text", "text": "do not use this" }]
        }))
        .expect_err("refusal must not become content");
        assert!(
            matches!(error, Error::Llm(message) if message.contains("refused") && message.contains("safety"))
        );
    }

    #[test]
    fn ignores_thinking_blocks_and_keeps_text_blocks() {
        let response = parse_message(json!({
            "stop_reason": "end_turn",
            "content": [
                { "type": "thinking", "thinking": "private reasoning" },
                { "type": "text", "text": "visible" }
            ]
        }))
        .expect("valid response");
        assert_eq!(response.text, "visible");
    }

    #[test]
    fn reads_text_delta_and_ignores_thinking_delta() {
        let thinking = parse_anthropic_event(
            &Some("content_block_delta".to_owned()),
            r#"{"delta":{"type":"thinking_delta","thinking":"hidden"}}"#,
        )
        .expect("valid event");
        assert!(thinking.is_none());
        let text = parse_anthropic_event(
            &Some("content_block_delta".to_owned()),
            r#"{"delta":{"type":"text_delta","text":"Привет"}}"#,
        )
        .expect("valid event")
        .expect("text delta");
        assert_eq!(text.text, "Привет");
    }

    #[test]
    fn forces_strict_object_requirements() {
        let schema =
            strict_schema(json!({ "type": "object", "properties": { "x": { "type": "string" } } }));
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], json!(["x"]));
    }

    #[test]
    fn short_answer_payload_uses_low_effort() {
        let provider = AnthropicProvider::with_base_url(
            "redacted",
            "claude-haiku-5-5",
            "http://localhost",
        )
        .expect("provider");
        let payload = provider.payload(&ChatRequest::new("system", Vec::new(), 1_200), true);
        assert_eq!(payload["output_config"]["effort"], "low");
        assert!(payload.get("fallbacks").is_none());
    }
}
