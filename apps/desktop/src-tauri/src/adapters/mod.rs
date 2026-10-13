//! Adapters that implement the `yc-engine` ports with the real crates (`yc-ml`, `yc-hub`, `yc-llm`).
//!
//! They read the user's model/provider choices from [`Prefs`], which the backend updates whenever the
//! settings change, so a change applies to the next recording or summary without rebuilding the engine.

pub mod bench;
pub mod hub;
pub mod llm;
pub mod ml;
pub mod runtime_keeper;

use std::sync::{Arc, RwLock};

use crate::api::{ApiProvider, OllamaBase, SummaryMode};

/// Reads the engine's opaque `summary_provider` value. Understands the first release's shape
/// (`{provider: "claude" | "openai" | "ollama" | "local", entries: {..}}`) too. `None` for the
/// never-initialised `null`.
pub fn parse_summary_mode(v: &serde_json::Value) -> Option<SummaryMode> {
    if v.is_null() {
        return None;
    }
    if v.get("mode").is_some() {
        return serde_json::from_value(v.clone()).ok();
    }
    let provider = v.get("provider")?.as_str()?;
    let entry = v.get("entries").and_then(|e| e.get(provider));
    let text = |k: &str| {
        entry
            .and_then(|e| e.get(k))
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(str::to_owned)
    };
    Some(match provider {
        "claude" => SummaryMode::Api {
            provider: ApiProvider::Anthropic,
            model: text("model").unwrap_or_default(),
            base_url: None,
        },
        "openai" => SummaryMode::Api {
            provider: ApiProvider::OpenAiCompatible,
            model: text("model").unwrap_or_default(),
            base_url: text("baseUrl"),
        },
        "ollama" => SummaryMode::Ollama {
            base: OllamaBase::Local,
            model: text("model").unwrap_or_default(),
        },
        _ => SummaryMode::default(),
    })
}

/// Choices the adapters consult on every use.
#[derive(Debug, Clone, Default)]
pub struct Prefs {
    pub asr: Option<String>,
    pub speaker: Option<String>,
    pub text_embedding: Option<String>,
    pub llm: Option<String>,
    pub summary: SummaryMode,
    pub russian: bool,
}

/// Shared handle to [`Prefs`].
pub type SharedPrefs = Arc<RwLock<Prefs>>;

/// Snapshot of the prefs (poison-tolerant).
pub fn read_prefs(p: &SharedPrefs) -> Prefs {
    p.read().map(|g| g.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_and_current_shapes_parse() {
        assert_eq!(parse_summary_mode(&serde_json::Value::Null), None);
        assert_eq!(
            parse_summary_mode(
                &json!({"provider":"claude","entries":{"claude":{"model":"claude-haiku-5-5"}}})
            ),
            Some(SummaryMode::Api {
                provider: ApiProvider::Anthropic,
                model: "claude-haiku-5-5".into(),
                base_url: None
            })
        );
        assert_eq!(
            parse_summary_mode(&json!({"provider":"ollama","entries":{}})),
            Some(SummaryMode::Ollama {
                base: OllamaBase::Local,
                model: String::new()
            })
        );
        assert_eq!(
            parse_summary_mode(&json!({"provider":"local"})),
            Some(SummaryMode::default())
        );
        let current = SummaryMode::Ollama {
            base: OllamaBase::Cloud,
            model: "gpt-oss:120b".into(),
        };
        let value = serde_json::to_value(&current).unwrap();
        assert_eq!(value["mode"], "ollama");
        assert_eq!(value["base"], "cloud");
        assert_eq!(parse_summary_mode(&value), Some(current));
        assert_eq!(
            parse_summary_mode(&json!({"mode":"off"})),
            Some(SummaryMode::Off)
        );
    }
}
