//! Ollama's native API (`/api/chat`, `/api/tags`), for the local daemon and for Ollama Cloud.
//!
//! The daemon needs no key and also serves the user's cloud models (`name:cloud`) under the account
//! it is signed in to. Ollama Cloud proper (`https://ollama.com`) takes a bearer key.

use futures_util::StreamExt;
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use yc_core::{Error, Result};

use crate::types::*;

/// Address of the local Ollama daemon.
pub const LOCAL_URL: &str = "http://127.0.0.1:11434";
/// Address of the hosted Ollama Cloud API.
pub const CLOUD_URL: &str = "https://ollama.com";
/// Context window requested for chat calls (the daemon's default is far too small for transcripts).
const NUM_CTX: u32 = 32_768;

/// One model from `GET /api/tags`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OllamaModel {
    /// Tag, e.g. `glm-5.2:cloud` or `gemma4:e4b`.
    pub name: String,
    /// Size on disk in bytes (a few hundred for cloud stubs).
    pub size_bytes: u64,
    /// The model runs on Ollama's servers, not on this computer.
    pub cloud: bool,
}

/// What a probe of the local daemon found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OllamaProbe {
    /// The daemon answered.
    pub running: bool,
    /// Daemon version.
    pub version: Option<String>,
    /// The daemon is signed in to an Ollama account (needed for cloud models).
    pub signed_in: bool,
    /// Installed and cloud models.
    pub models: Vec<OllamaModel>,
}

/// Client for Ollama's native chat API.
#[derive(Clone)]
pub struct OllamaProvider {
    client: Client,
    base_url: Url,
    api_key: Option<String>,
    model: String,
}

impl std::fmt::Debug for OllamaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OllamaProvider")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

impl OllamaProvider {
    /// Create a client. `api_key` is `None` for the local daemon and the bearer key for Ollama Cloud.
    pub fn new(
        base_url: impl AsRef<str>,
        api_key: Option<String>,
        model: impl Into<String>,
    ) -> Result<Self> {
        let base_url = Url::parse(base_url.as_ref())
            .map_err(|error| Error::Invalid(format!("invalid Ollama URL: {error}")))?;
        if !matches!(base_url.scheme(), "http" | "https") {
            return Err(Error::Invalid(
                "Ollama URL must use HTTP or HTTPS".to_owned(),
            ));
        }
        Ok(Self {
            client: Client::builder().build().map_err(|error| {
                Error::Network(format!("could not create HTTP client: {error}"))
            })?,
            base_url,
            api_key,
            model: model.into(),
        })
    }

    /// Whether requests go to the hosted Ollama Cloud API (a key is attached).
    pub fn is_hosted(&self) -> bool {
        self.api_key.is_some()
    }

    /// Fetch the model list (`GET /api/tags`).
    pub async fn list_models(&self) -> Result<Vec<OllamaModel>> {
        let response = self
            .client
            .get(self.endpoint("api/tags"))
            .headers(self.auth_headers()?)
            .send()
            .await
            .map_err(network_error)?;
        let value: Value = checked_response(response)
            .await?
            .json()
            .await
            .map_err(|error| Error::Llm(format!("invalid /api/tags response: {error}")))?;
        Ok(parse_tags(&value))
    }

    /// Look at a daemon: reachable, version, signed in, models. Never fails; an unreachable daemon is
    /// reported as `running: false`.
    pub async fn probe(base_url: &str) -> OllamaProbe {
        let down = OllamaProbe {
            running: false,
            version: None,
            signed_in: false,
            models: Vec::new(),
        };
        let Ok(client) = Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()
        else {
            return down;
        };
        let base = base_url.trim_end_matches('/');
        let Ok(response) = client.get(format!("{base}/api/version")).send().await else {
            return down;
        };
        let version = response
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| v["version"].as_str().map(str::to_owned));
        let models = match client.get(format!("{base}/api/tags")).send().await {
            Ok(r) => r
                .json::<Value>()
                .await
                .map(|v| parse_tags(&v))
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        // `/api/me` answers 200 with the account when signed in, 401 otherwise.
        let signed_in = matches!(
            client.post(format!("{base}/api/me")).send().await,
            Ok(r) if r.status().is_success()
        );
        OllamaProbe {
            running: true,
            version,
            signed_in,
            models,
        }
    }

    fn endpoint(&self, path: &str) -> Url {
        let mut endpoint = self.base_url.clone();
        let base = endpoint.path().trim_end_matches('/');
        endpoint.set_path(&format!("{base}/{}", path.trim_start_matches('/')));
        endpoint
    }

    fn auth_headers(&self) -> Result<reqwest::header::HeaderMap> {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(key) = self.api_key.as_deref() {
            let value =
                reqwest::header::HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| {
                    Error::Invalid("API key contains invalid header characters".to_owned())
                })?;
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
        Ok(headers)
    }

    fn payload(&self, request: &ChatRequest, streaming: bool, think: bool) -> Value {
        let mut messages = Vec::with_capacity(request.messages.len() + 1);
        // Cloud models ignore `format`, so the schema is also spelled out in the instruction.
        let mut system = request.system.clone();
        if let Some(schema) = &request.json_schema {
            system.push_str(&format!(
                "\n\nReply with exactly one JSON object and nothing else (no Markdown fence). It must \
                 match this JSON Schema, with no extra fields:\n{}",
                schema.schema
            ));
        }
        if !system.is_empty() {
            messages.push(json!({ "role": "system", "content": system }));
        }
        messages.extend(request.messages.iter().map(|message| {
            let role = match message.role {
                ChatRole::User => "user",
                ChatRole::Assistant => "assistant",
            };
            json!({ "role": role, "content": message.content })
        }));
        let mut options = json!({ "num_predict": request.max_tokens, "num_ctx": NUM_CTX });
        if let Some(temperature) = request.temperature {
            options["temperature"] = json!(temperature);
        }
        if !request.stop.is_empty() {
            options["stop"] = json!(request.stop);
        }
        let mut payload = json!({
            "model": self.model,
            "messages": messages,
            "stream": streaming,
            "options": options,
        });
        if !think {
            // Reasoning tokens only slow a summary down and eat the output budget.
            payload["think"] = json!(false);
        }
        if let Some(schema) = &request.json_schema {
            payload["format"] = schema.schema.clone();
        }
        payload
    }

    /// POST `/api/chat`; retries once without `think:false` for models that reject the flag.
    async fn post_chat(&self, request: &ChatRequest, streaming: bool) -> Result<reqwest::Response> {
        let send = |think: bool| {
            let client = self.client.clone();
            let url = self.endpoint("api/chat");
            let headers = self.auth_headers();
            let body = self.payload(request, streaming, think);
            async move {
                client
                    .post(url)
                    .headers(headers?)
                    .json(&body)
                    .send()
                    .await
                    .map_err(network_error)
            }
        };
        let response = send(false).await?;
        if response.status() == StatusCode::BAD_REQUEST {
            let body = response.text().await.unwrap_or_default();
            if body.contains("think") {
                return checked_response(send(true).await?).await;
            }
            return Err(Error::Llm(format!(
                "Ollama returned 400: {}",
                truncate(&body, 1_000)
            )));
        }
        checked_response(response).await
    }
}

#[async_trait::async_trait]
impl ChatProvider for OllamaProvider {
    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse> {
        let value: Value = self
            .post_chat(&request, false)
            .await?
            .json()
            .await
            .map_err(|error| Error::Llm(format!("invalid Ollama chat response: {error}")))?;
        Ok(parse_final(&value))
    }

    async fn stream(&self, request: ChatRequest) -> Result<ChatStream> {
        let response = self.post_chat(&request, true).await?;
        let mut body = response.bytes_stream();
        let output = async_stream::stream! {
            // NDJSON: one JSON object per line; a line can be split across network chunks.
            let mut pending: Vec<u8> = Vec::new();
            while let Some(chunk) = body.next().await {
                match chunk {
                    Ok(chunk) => {
                        pending.extend_from_slice(&chunk);
                        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                            let line: Vec<u8> = pending.drain(..=end).collect();
                            match parse_line(&line) {
                                Ok(Some(delta)) => yield Ok(delta),
                                Ok(None) => {}
                                Err(error) => yield Err(error),
                            }
                        }
                    }
                    Err(error) => {
                        yield Err(network_error(error));
                        return;
                    }
                }
            }
            match parse_line(&pending) {
                Ok(Some(delta)) => yield Ok(delta),
                Ok(None) => {}
                Err(error) => yield Err(error),
            }
        };
        Ok(Box::pin(output))
    }

    fn provider_id(&self) -> &str {
        if self.is_hosted() {
            "ollama-cloud"
        } else {
            "ollama"
        }
    }

    fn model(&self) -> &str {
        &self.model
    }
}

fn parse_tags(value: &Value) -> Vec<OllamaModel> {
    value["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let name = m["name"].as_str().or_else(|| m["model"].as_str())?;
            let remote = m["remote_host"].as_str().is_some_and(|h| !h.is_empty())
                || name.ends_with(":cloud")
                || name.ends_with("-cloud");
            Some(OllamaModel {
                name: name.to_owned(),
                size_bytes: m["size"].as_u64().unwrap_or(0),
                cloud: remote,
            })
        })
        .collect()
}

/// Final (non-streaming) response, or the last streamed object, to our response type.
fn parse_final(value: &Value) -> ChatResponse {
    ChatResponse {
        text: value["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        usage: TokenUsage {
            input_tokens: value["prompt_eval_count"].as_u64(),
            output_tokens: value["eval_count"].as_u64(),
        },
        timings: Timings {
            prompt_tokens_per_second: rate(value, "prompt_eval_count", "prompt_eval_duration"),
            decode_tokens_per_second: rate(value, "eval_count", "eval_duration"),
        },
        stop_reason: value["done_reason"].as_str().map(str::to_owned),
    }
}

/// Tokens per second from a count and a duration in nanoseconds (absent on cloud models).
fn rate(value: &Value, count: &str, duration: &str) -> Option<f64> {
    let n = value[count].as_f64()?;
    let ns = value[duration].as_f64().filter(|d| *d > 0.0)?;
    Some(n / (ns / 1e9))
}

fn parse_line(line: &[u8]) -> Result<Option<ChatDelta>> {
    let line = std::str::from_utf8(line)
        .map_err(|error| Error::Llm(format!("invalid UTF-8 in Ollama stream: {error}")))?
        .trim();
    if line.is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(line)
        .map_err(|error| Error::Llm(format!("invalid Ollama stream event: {error}")))?;
    if let Some(message) = value["error"].as_str() {
        return Err(Error::Llm(format!("Ollama error: {message}")));
    }
    let done = value["done"].as_bool().unwrap_or(false);
    let text = value["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if !done && text.is_empty() {
        // Reasoning ("thinking") chunks carry no answer text.
        return Ok(None);
    }
    Ok(Some(ChatDelta {
        text,
        stop_reason: done.then(|| value["done_reason"].as_str().unwrap_or("stop").to_owned()),
        usage: done.then(|| TokenUsage {
            input_tokens: value["prompt_eval_count"].as_u64(),
            output_tokens: value["eval_count"].as_u64(),
        }),
    }))
}

async fn checked_response(response: reqwest::Response) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let message = format!("Ollama returned {status}: {}", truncate(&body, 1_000));
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        Err(Error::Network(message))
    } else {
        Err(Error::Llm(message))
    }
}

fn network_error(error: reqwest::Error) -> Error {
    if error.is_timeout() || error.is_connect() || error.is_request() || error.is_body() {
        Error::Network(format!("Ollama request failed: {error}"))
    } else {
        Error::Llm(format!("Ollama response failed: {error}"))
    }
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
    use super::*;

    #[test]
    fn tags_mark_cloud_models_by_remote_host_or_suffix() {
        let models = parse_tags(&json!({ "models": [
            { "name": "gemma4:e4b", "size": 5_000_000_000_u64 },
            { "name": "glm-5.2:cloud", "size": 338, "remote_host": "https://ollama.com:443" },
            { "name": "qwen3:480b-cloud", "size": 1 },
            { "name": "someone/model:latest", "size": 9, "remote_host": "https://ollama.com:443" }
        ]}));
        let cloud: Vec<_> = models.iter().map(|m| m.cloud).collect();
        assert_eq!(cloud, [false, true, true, true]);
    }

    #[test]
    fn stream_lines_skip_thinking_and_report_the_final_stats() {
        assert_eq!(
            parse_line(br#"{"message":{"content":"","thinking":"hm"},"done":false}"#).expect("ok"),
            None
        );
        let delta = parse_line(br#"{"message":{"content":"hi"},"done":false}"#)
            .expect("ok")
            .expect("delta");
        assert_eq!(delta.text, "hi");
        let last = parse_line(
            br#"{"message":{"content":""},"done":true,"done_reason":"stop","eval_count":10,"eval_duration":500000000}"#,
        )
        .expect("ok")
        .expect("delta");
        assert_eq!(last.stop_reason.as_deref(), Some("stop"));
        assert_eq!(last.usage.and_then(|u| u.output_tokens), Some(10));
        assert!(parse_line(br#"{"error":"boom"}"#).is_err());
    }

    #[test]
    fn final_response_computes_decode_speed() {
        let r = parse_final(
            &json!({"message":{"content":"x"},"eval_count":20,"eval_duration":1_000_000_000_u64}),
        );
        assert_eq!(r.timings.decode_tokens_per_second, Some(20.0));
        assert_eq!(r.timings.prompt_tokens_per_second, None);
    }

    #[test]
    fn payload_carries_schema_options_and_think_flag() {
        let provider = OllamaProvider::new(LOCAL_URL, None, "m").expect("provider");
        let mut request = ChatRequest::new("sys", vec![ChatMessage::user("hi")], 123);
        request.json_schema = Some(JsonSchema {
            name: "s".into(),
            schema: json!({"type":"object"}),
        });
        let body = provider.payload(&request, true, false);
        assert_eq!(body["format"], json!({"type":"object"}));
        assert_eq!(body["options"]["num_predict"], 123);
        assert_eq!(body["think"], false);
        assert_eq!(body["messages"][0]["role"], "system");
        assert!(
            provider
                .payload(&request, false, true)
                .get("think")
                .is_none()
        );
    }
}
