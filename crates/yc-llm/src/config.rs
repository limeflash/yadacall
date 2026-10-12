//! Serialized provider selection and construction.

use serde::{Deserialize, Serialize};
use yc_core::{Error, Result};

use crate::{anthropic::AnthropicProvider, openai::OpenAiCompatibleProvider, secrets};

/// Provider settings. Credentials are looked up by provider id in the OS keychain.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ProviderConfig {
    /// Local llama-server or another custom OpenAI-compatible service.
    OpenAiCompatible {
        /// Credential account id; use `local` for a local server.
        provider_id: String,
        /// API root, usually ending in `/v1`.
        base_url: String,
        /// Model id sent to the service.
        model: String,
    },
    /// Ollama's OpenAI-compatible endpoint.
    Ollama {
        /// Model name.
        model: String,
        /// API root; defaults to `http://localhost:11434/v1`.
        #[serde(default = "default_ollama_url")]
        base_url: String,
    },
    /// OpenRouter.
    OpenRouter {
        /// Model id, for example `anthropic/claude-sonnet-5-5`.
        model: String,
    },
    /// Anthropic Messages API.
    Anthropic {
        /// Exact API model id; defaults to `claude-sonnet-5-5`.
        #[serde(default = "default_anthropic_model")]
        model: String,
    },
}

impl ProviderConfig {
    /// Construct the configured provider. API keys never appear in this serialized config.
    pub fn build(&self) -> Result<Box<dyn crate::ChatProvider>> {
        match self {
            Self::OpenAiCompatible {
                provider_id,
                base_url,
                model,
            } => {
                let key = secrets::get(provider_id)?;
                Ok(Box::new(OpenAiCompatibleProvider::new(
                    base_url,
                    key,
                    provider_id,
                    model,
                )?))
            }
            Self::Ollama { model, base_url } => Ok(Box::new(OpenAiCompatibleProvider::new(
                base_url, None, "ollama", model,
            )?)),
            Self::OpenRouter { model } => {
                let key = secrets::get("openrouter")?.ok_or_else(|| {
                    Error::Llm("OpenRouter API key is missing from the OS keychain".to_owned())
                })?;
                Ok(Box::new(OpenAiCompatibleProvider::new(
                    "https://openrouter.ai/api/v1",
                    Some(key),
                    "openrouter",
                    model,
                )?))
            }
            Self::Anthropic { model } => {
                let key = secrets::get("anthropic")?.ok_or_else(|| {
                    Error::Llm("Anthropic API key is missing from the OS keychain".to_owned())
                })?;
                Ok(Box::new(AnthropicProvider::new(key, model)?))
            }
        }
    }
}

fn default_ollama_url() -> String {
    "http://localhost:11434/v1".to_owned()
}
fn default_anthropic_model() -> String {
    "claude-sonnet-5-5".to_owned()
}
