//! Serialized provider selection and construction.

use serde::{Deserialize, Serialize};
use yc_core::{Error, Result};

use crate::{
    anthropic::AnthropicProvider,
    ollama::{self, OllamaProvider},
    openai::OpenAiCompatibleProvider,
    secrets,
};

/// Keychain account of the Ollama Cloud API key.
pub const OLLAMA_CLOUD_ACCOUNT: &str = "ollama-cloud";

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
    /// The local Ollama daemon through its native API. Cloud models (`name:cloud`) it serves run
    /// under the account the daemon is signed in to; no key is involved.
    Ollama {
        /// Model name.
        model: String,
        /// Daemon address; defaults to `http://127.0.0.1:11434`.
        #[serde(default = "default_ollama_url")]
        base_url: String,
    },
    /// Ollama Cloud's hosted API; the key is read from the keychain account `ollama-cloud`.
    OllamaCloud {
        /// Model name, for example `gpt-oss:120b`.
        model: String,
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
            Self::Ollama { model, base_url } => {
                let base = base_url.trim_end_matches('/');
                let base = base.strip_suffix("/v1").unwrap_or(base);
                Ok(Box::new(OllamaProvider::new(base, None, model)?))
            }
            Self::OllamaCloud { model } => {
                let key = secrets::get(OLLAMA_CLOUD_ACCOUNT)?.ok_or_else(|| {
                    Error::Llm("Ollama Cloud API key is missing from the OS keychain".to_owned())
                })?;
                Ok(Box::new(OllamaProvider::new(
                    ollama::CLOUD_URL,
                    Some(key),
                    model,
                )?))
            }
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
    ollama::LOCAL_URL.to_owned()
}
fn default_anthropic_model() -> String {
    "claude-sonnet-5-5".to_owned()
}
