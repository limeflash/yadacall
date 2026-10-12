//! LLM adapters and meeting intelligence for YadaCall.

mod anthropic;
mod config;
mod local_server;
mod openai;
mod rag;
mod secrets;
mod sse;
mod summary;
mod types;

pub use anthropic::AnthropicProvider;
pub use config::ProviderConfig;
pub use local_server::{LocalServer, LocalServerConfig, LocalServerProgress};
pub use openai::OpenAiCompatibleProvider;
pub use rag::{Answer, AnswerCitation, ContextHit, EgressNote, answer_question, context_hit};
pub use secrets::{delete as delete_secret, get as get_secret, set as set_secret};
pub use summary::{
    MeetingMetadata, SummaryInput, SummaryOptions, SummaryProgress, SummaryTemplate, SummaryTurn,
    summarize,
};
pub use types::{
    ChatDelta, ChatMessage, ChatProvider, ChatRequest, ChatResponse, ChatRole, ChatStream,
    JsonSchema, Timings, TokenUsage,
};
