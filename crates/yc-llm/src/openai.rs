//! OpenAI-compatible chat completions, including llama-server and Ollama endpoints.

use futures_util::StreamExt;
use reqwest::{Client, StatusCode, Url};
use serde_json::{Value, json};
use yc_core::{Error, Result};

use crate::{sse::SseParser, types::*};

/// Client for an OpenAI-compatible `/chat/completions` API.
#[derive(Clone)]
pub struct OpenAiCompatibleProvider {
    client: Client,
    base_url: Url,
    api_key: Option<String>,
    provider_id: String,
    model: String,
}

impl std::fmt::Debug for OpenAiCompatibleProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatibleProvider")
            .field("base_url", &self.base_url)
            .field("provider_id", &self.provider_id)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

impl OpenAiCompatibleProvider {
    /// Create a provider for any endpoint implementing the OpenAI chat-completions API.
    pub fn new(
        base_url: impl AsRef<str>,
        api_key: Option<String>,
        provider_id: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self> {
        let base_url = Url::parse(base_url.as_ref())
            .map_err(|error| Error::Invalid(format!("invalid LLM base URL: {error}")))?;
        if !matches!(base_url.scheme(), "http" | "https") {
            return Err(Error::Invalid(
                "LLM base URL must use HTTP or HTTPS".to_owned(),
            ));
        }
        Ok(Self {
            client: Client::builder().build().map_err(|error| {
                Error::Network(format!("could not create HTTP client: {error}"))
            })?,
            base_url,
            api_key,
            provider_id: provider_id.into(),
            model: model.into(),
        })
    }

    /// Fetch model ids from `GET /models`.
    pub async fn list_models(&self) -> Result<Vec<String>> {
        let response = self
            .client
            .get(self.endpoint("models"))
            .headers(self.auth_headers()?)
            .send()
            .await
            .map_err(network_error)?;
        let response = checked_response(response).await?;
        let value: Value = response
            .json()
            .await
            .map_err(|error| Error::Llm(format!("invalid /models response: {error}")))?;
        Ok(value["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|model| model["id"].as_str().map(str::to_owned))
            .collect())
    }

    /// Ask llama-server's `/tokenize` endpoint. Returns `None` when the endpoint is unavailable.
    pub async fn tokenize(&self, text: &str) -> Result<Option<usize>> {
        let response = self
            .client
            .post(self.endpoint("tokenize"))
            .headers(self.auth_headers()?)
            .json(&json!({ "content": text }))
            .send()
            .await
            .map_err(network_error)?;
        if response.status() == StatusCode::NOT_FOUND
            || response.status() == StatusCode::METHOD_NOT_ALLOWED
        {
            return Ok(None);
        }
        let response = checked_response(response).await?;
        let value: Value = response
            .json()
            .await
            .map_err(|error| Error::Llm(format!("invalid /tokenize response: {error}")))?;
        Ok(value["tokens"]
            .as_array()
            .map(Vec::len)
            .or_else(|| value["token_count"].as_u64().map(|n| n as usize)))
    }

    fn endpoint(&self, path: &str) -> Url {
        let mut endpoint = self.base_url.clone();
        let base = endpoint.path().trim_end_matches('/');
        let suffix = path.trim_start_matches('/');
        endpoint.set_path(&format!("{base}/{suffix}"));
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

    fn payload(&self, request: &ChatRequest, streaming: bool) -> Value {
        let mut messages = Vec::with_capacity(request.messages.len() + 1);
        if !request.system.is_empty() {
            messages.push(json!({ "role": "system", "content": request.system }));
        }
        messages.extend(request.messages.iter().map(|message| {
            let role = match message.role {
                ChatRole::User => "user",
                ChatRole::Assistant => "assistant",
            };
            json!({ "role": role, "content": message.content })
        }));
        let mut payload = json!({
            "model": self.model,
            "messages": messages,
            "max_tokens": request.max_tokens,
            "stream": streaming,
        });
        if let Some(temperature) = request.temperature {
            payload["temperature"] = json!(temperature);
        }
        if !request.stop.is_empty() {
            payload["stop"] = json!(request.stop);
        }
        if let Some(schema) = &request.json_schema {
            payload["response_format"] = json!({
                "type": "json_schema",
                "json_schema": { "name": schema.name, "schema": schema.schema, "strict": true }
            });
        }
        payload
    }
}

#[async_trait::async_trait]
impl ChatProvider for OpenAiCompatibleProvider {
    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse> {
        let response = self
            .client
            .post(self.endpoint("chat/completions"))
            .headers(self.auth_headers()?)
            .json(&self.payload(&request, false))
            .send()
            .await
            .map_err(network_error)?;
        let response = checked_response(response).await?;
        let value: Value = response
            .json()
            .await
            .map_err(|error| Error::Llm(format!("invalid chat completion response: {error}")))?;
        parse_completion(value)
    }

    async fn stream(&self, request: ChatRequest) -> Result<ChatStream> {
        let response = self
            .client
            .post(self.endpoint("chat/completions"))
            .headers(self.auth_headers()?)
            .json(&self.payload(&request, true))
            .send()
            .await
            .map_err(network_error)?;
        let response = checked_response(response).await?;
        let mut body = response.bytes_stream();
        let output = async_stream::stream! {
            let mut parser = SseParser::default();
            while let Some(chunk) = body.next().await {
                match chunk {
                    Ok(chunk) => {
                        for event in parser.push(&chunk) {
                            if event.data == "[DONE]" { return; }
                            match parse_openai_delta(&event.data) {
                                Ok(Some(delta)) => yield Ok(delta),
                                Ok(None) => {},
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
            for event in parser.finish() {
                if event.data == "[DONE]" { return; }
                match parse_openai_delta(&event.data) {
                    Ok(Some(delta)) => yield Ok(delta),
                    Ok(None) => {},
                    Err(error) => yield Err(error),
                }
            }
        };
        Ok(Box::pin(output))
    }

    fn provider_id(&self) -> &str {
        &self.provider_id
    }
    fn model(&self) -> &str {
        &self.model
    }

    async fn count_tokens(&self, text: &str) -> Result<Option<usize>> {
        if self.provider_id == "local" {
            self.tokenize(text).await
        } else {
            Ok(None)
        }
    }
}

fn parse_completion(value: Value) -> Result<ChatResponse> {
    let choice = value["choices"]
        .as_array()
        .and_then(|choices| choices.first())
        .ok_or_else(|| Error::Llm("chat completion has no choices".to_owned()))?;
    let text = choice["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let usage = TokenUsage {
        input_tokens: value["usage"]["prompt_tokens"].as_u64(),
        output_tokens: value["usage"]["completion_tokens"].as_u64(),
    };
    let timings = Timings {
        prompt_tokens_per_second: value["timings"]["prompt_per_second"].as_f64(),
        decode_tokens_per_second: value["timings"]["predicted_per_second"].as_f64(),
    };
    Ok(ChatResponse {
        text,
        usage,
        timings,
        stop_reason: choice["finish_reason"].as_str().map(str::to_owned),
    })
}

fn parse_openai_delta(data: &str) -> Result<Option<ChatDelta>> {
    let value: Value = serde_json::from_str(data)
        .map_err(|error| Error::Llm(format!("invalid OpenAI stream event: {error}")))?;
    let Some(choice) = value["choices"]
        .as_array()
        .and_then(|choices| choices.first())
    else {
        return Ok(None);
    };
    let text = choice["delta"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let stop_reason = choice["finish_reason"].as_str().map(str::to_owned);
    let usage = value.get("usage").map(|usage| TokenUsage {
        input_tokens: usage["prompt_tokens"].as_u64(),
        output_tokens: usage["completion_tokens"].as_u64(),
    });
    Ok(Some(ChatDelta {
        text,
        stop_reason,
        usage,
    }))
}

async fn checked_response(response: reqwest::Response) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let message = format!(
        "OpenAI-compatible endpoint returned {status}: {}",
        truncate(&body, 1_000)
    );
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        Err(Error::Network(message))
    } else {
        Err(Error::Llm(message))
    }
}

fn network_error(error: reqwest::Error) -> Error {
    if error.is_timeout() || error.is_connect() || error.is_request() || error.is_body() {
        Error::Network(format!("LLM request failed: {error}"))
    } else {
        Error::Llm(format!("LLM response failed: {error}"))
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
    use super::{parse_completion, parse_openai_delta};
    use serde_json::json;

    #[test]
    fn reads_llama_server_timings() {
        let response = parse_completion(json!({
            "choices": [{ "message": { "content": "ok" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 },
            "timings": { "prompt_per_second": 123.0, "predicted_per_second": 42.5 }
        }))
        .expect("valid completion");
        assert_eq!(response.text, "ok");
        assert_eq!(response.timings.prompt_tokens_per_second, Some(123.0));
        assert_eq!(response.timings.decode_tokens_per_second, Some(42.5));
    }

    #[test]
    fn reads_openai_delta_and_finish_reason() {
        let delta =
            parse_openai_delta(r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#)
                .expect("valid event")
                .expect("choice");
        assert_eq!(delta.text, "hi");
    }
}
