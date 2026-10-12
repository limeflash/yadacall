use serde::{Deserialize, Serialize};

use crate::Ms;

/// Contents of `summary.json`. Produced by an LLM under a JSON schema; every claim that can be
/// anchored carries a timestamp so the UI can jump to the moment it was said.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// Short title suggestion, used when the user did not name the meeting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// "О чём встреча" — 1–3 short paragraphs.
    pub about: Vec<String>,
    pub decisions: Vec<Decision>,
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub open_questions: Vec<String>,
    /// Language the summary is written in.
    pub language: String,
    /// Provider/model that produced it, e.g. `local:gemma-4-e4b` or `anthropic:claude-sonnet-5-5`.
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<Ms>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub text: String,
    /// Speaker key or free-form name of the owner, if it was said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Due date as said or resolved (`2026-10-15`), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<Ms>,
    #[serde(default)]
    pub done: bool,
}
