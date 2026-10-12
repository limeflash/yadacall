//! Adapters that implement the `yc-engine` ports with the real crates (`yc-ml`, `yc-hub`, `yc-llm`).
//!
//! They read the user's model/provider choices from [`Prefs`], which the backend updates whenever the
//! settings change, so a change applies to the next recording or summary without rebuilding the engine.

pub mod bench;
pub mod hub;
pub mod llm;
pub mod ml;
pub mod runtime_keeper;

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::api::SummaryProvider;

/// Per-provider summary settings (model, optional base URL).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderEntry {
    pub model: String,
    pub base_url: Option<String>,
}

/// Which provider summarises and answers, and each provider's own settings.
/// Stored as the opaque `summary_provider` value of the engine settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SummaryConfig {
    pub provider: SummaryProvider,
    pub entries: BTreeMap<String, ProviderEntry>,
}

impl Default for SummaryConfig {
    fn default() -> Self {
        Self {
            provider: SummaryProvider::Local,
            entries: BTreeMap::new(),
        }
    }
}

impl SummaryConfig {
    /// Key under which a provider's entry is stored.
    pub fn key(p: SummaryProvider) -> &'static str {
        match p {
            SummaryProvider::Local => "local",
            SummaryProvider::Claude => "claude",
            SummaryProvider::Openai => "openai",
            SummaryProvider::Ollama => "ollama",
        }
    }

    /// Default model of a provider (empty = chosen automatically).
    pub fn default_model(p: SummaryProvider) -> &'static str {
        match p {
            SummaryProvider::Claude => "claude-sonnet-5-5",
            SummaryProvider::Ollama => "gemma3",
            SummaryProvider::Local | SummaryProvider::Openai => "",
        }
    }

    /// The active provider's entry with defaults filled in.
    pub fn current(&self) -> ProviderEntry {
        self.entry(self.provider)
    }

    /// A provider's entry with defaults filled in.
    pub fn entry(&self, p: SummaryProvider) -> ProviderEntry {
        let mut e = self.entries.get(Self::key(p)).cloned().unwrap_or_default();
        if e.model.is_empty() {
            e.model = Self::default_model(p).to_owned();
        }
        e
    }

    /// Parses the engine's opaque value; `None` for the never-initialised `null`.
    pub fn from_value(v: &serde_json::Value) -> Option<Self> {
        if v.is_null() {
            return None;
        }
        serde_json::from_value(v.clone()).ok()
    }
}

/// Choices the adapters consult on every use.
#[derive(Debug, Clone, Default)]
pub struct Prefs {
    pub asr: Option<String>,
    pub speaker: Option<String>,
    pub text_embedding: Option<String>,
    pub llm: Option<String>,
    pub summary: SummaryConfig,
    pub russian: bool,
}

/// Shared handle to [`Prefs`].
pub type SharedPrefs = Arc<RwLock<Prefs>>;

/// Snapshot of the prefs (poison-tolerant).
pub fn read_prefs(p: &SharedPrefs) -> Prefs {
    p.read().map(|g| g.clone()).unwrap_or_default()
}
