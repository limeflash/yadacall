//! Shared chat request and provider response types.

use std::pin::Pin;

use futures_core::Stream;
use serde::{Deserialize, Serialize};
use yc_core::Result;

/// Role and text for one item in a chat conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    User,
    Assistant,
}

/// One user or assistant message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Message role.
    pub role: ChatRole,
    /// Plain text content.
    pub content: String,
}

impl ChatMessage {
    /// Create a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::User,
            content: content.into(),
        }
    }

    /// Create an assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::Assistant,
            content: content.into(),
        }
    }
}

/// A JSON Schema requested as a constrained model response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonSchema {
    /// Human-readable schema name.
    pub name: String,
    /// JSON Schema document.
    pub schema: serde_json::Value,
}

/// A provider-independent chat completion request.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    /// System instruction, separated because some providers encode it specially.
    pub system: String,
    /// User and assistant messages.
    pub messages: Vec<ChatMessage>,
    /// Maximum generated tokens.
    pub max_tokens: u32,
    /// Optional sampling temperature. Providers that reject it omit it.
    pub temperature: Option<f32>,
    /// Optional nucleus sampling probability.
    pub top_p: Option<f32>,
    /// Optional top-k sampling limit.
    pub top_k: Option<u32>,
    /// Optional minimum probability cutoff.
    pub min_p: Option<f32>,
    /// Optional presence penalty.
    pub presence_penalty: Option<f32>,
    /// Optional constrained JSON output.
    pub json_schema: Option<JsonSchema>,
    /// Stop sequences.
    pub stop: Vec<String>,
}

impl ChatRequest {
    /// Construct a request with no optional sampling, schema, or stop settings.
    pub fn new(system: impl Into<String>, messages: Vec<ChatMessage>, max_tokens: u32) -> Self {
        Self {
            system: system.into(),
            messages,
            max_tokens,
            temperature: None,
            top_p: None,
            top_k: None,
            min_p: None,
            presence_penalty: None,
            json_schema: None,
            stop: Vec::new(),
        }
    }
}

/// Token usage reported by a provider, when available.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Prompt tokens.
    pub input_tokens: Option<u64>,
    /// Generated tokens.
    pub output_tokens: Option<u64>,
}

/// Token rates reported by a provider, when available.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Timings {
    /// Prompt processing tokens per second.
    pub prompt_tokens_per_second: Option<f64>,
    /// Decode tokens per second.
    pub decode_tokens_per_second: Option<f64>,
}

/// One completed chat response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatResponse {
    /// Generated text.
    pub text: String,
    /// Token usage, if reported.
    pub usage: TokenUsage,
    /// Server timing data, if reported.
    pub timings: Timings,
    /// Provider stop reason, if reported.
    pub stop_reason: Option<String>,
}

/// Incremental text emitted while a completion is streaming.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatDelta {
    /// Newly generated text; empty deltas can carry final metadata.
    pub text: String,
    /// Stop reason, normally present only on the final event.
    pub stop_reason: Option<String>,
    /// Usage, when the provider includes it in the stream.
    pub usage: Option<TokenUsage>,
}

/// Stream returned from a chat provider.
pub type ChatStream = Pin<Box<dyn Stream<Item = Result<ChatDelta>> + Send>>;

/// Provider abstraction shared by network APIs and the local server.
#[async_trait::async_trait]
pub trait ChatProvider: Send + Sync {
    /// Complete one chat request.
    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse>;

    /// Stream one chat request.
    async fn stream(&self, request: ChatRequest) -> Result<ChatStream>;

    /// Stable provider id used for privacy notes and summary metadata.
    fn provider_id(&self) -> &str;

    /// Configured model id.
    fn model(&self) -> &str;

    /// Estimate tokens with a provider-specific tokenizer, when available.
    async fn count_tokens(&self, _text: &str) -> Result<Option<usize>> {
        Ok(None)
    }
}
