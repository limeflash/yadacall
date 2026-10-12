//! `yc-llm` behind the engine's [`Summarizer`] and [`Answerer`] ports, per the summary-provider setting:
//! on-device Gemma through a supervised `llama-server`, Anthropic (key from the OS keychain), or any
//! OpenAI-compatible / Ollama endpoint.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use yc_core::summary::Summary;
use yc_core::{Error, Result};
use yc_engine::ports::{
    AnswerEvent, AnswerRequest, Answerer, Summarizer, SummaryInput as EngineSummaryInput,
};
use yc_hub::catalog::{ModelKind, ModelSpec};
use yc_hub::predictor::Verdict;
use yc_llm::{
    ChatProvider, LocalServer, LocalServerConfig, MeetingMetadata, ProviderConfig, SummaryInput,
    SummaryOptions, SummaryTemplate, SummaryTurn, answer_question,
};

use super::hub::{Insights, ModelHub};
use super::{SharedPrefs, read_prefs};
use crate::api::SummaryProvider;

/// Keychain account of the Anthropic key.
pub const KEY_ANTHROPIC: &str = "anthropic";
/// Keychain account of the OpenAI-compatible key.
pub const KEY_OPENAI: &str = "openai";
/// A local server unused for this long is stopped (it restarts on the next use).
const SERVER_IDLE: Duration = Duration::from_secs(15 * 60);

struct ServerSlot {
    server: LocalServer,
    model_id: String,
}

/// The summary/answer provider selected in the settings.
pub struct Llm {
    hub: Arc<ModelHub>,
    insights: Arc<Insights>,
    prefs: SharedPrefs,
    server: tokio::sync::Mutex<Option<ServerSlot>>,
    keys: Mutex<HashMap<String, bool>>,
}

impl Llm {
    pub fn new(hub: Arc<ModelHub>, insights: Arc<Insights>, prefs: SharedPrefs) -> Arc<Self> {
        Arc::new(Self {
            hub,
            insights,
            prefs,
            server: tokio::sync::Mutex::new(None),
            keys: Mutex::new(HashMap::new()),
        })
    }

    // ---- keychain ---------------------------------------------------------------------------------

    /// Whether a key is stored for `account` (cached; the keychain is read once per session).
    pub fn key_present(&self, account: &str) -> bool {
        let mut cache = self.keys.lock().unwrap_or_else(|e| e.into_inner());
        *cache
            .entry(account.to_owned())
            .or_insert_with(|| yc_llm::get_secret(account).ok().flatten().is_some())
    }

    /// Stores a key in the OS keychain.
    pub fn set_key(&self, account: &str, secret: &str) -> Result<()> {
        yc_llm::set_secret(account, secret)?;
        self.keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(account.to_owned(), true);
        Ok(())
    }

    /// Keychain account used by a provider, if it takes a key.
    pub fn account_of(p: SummaryProvider) -> Option<&'static str> {
        match p {
            SummaryProvider::Claude => Some(KEY_ANTHROPIC),
            SummaryProvider::Openai => Some(KEY_OPENAI),
            SummaryProvider::Local | SummaryProvider::Ollama => None,
        }
    }

    // ---- provider state -----------------------------------------------------------------------------

    /// Whether the selected provider can produce a summary right now.
    pub fn configured(&self) -> bool {
        let cfg = read_prefs(&self.prefs).summary;
        match cfg.provider {
            SummaryProvider::Local => {
                self.hub.installed_runtime(None).is_some()
                    && self
                        .hub
                        .installed_of(ModelKind::Llm)
                        .first()
                        .is_some()
            }
            SummaryProvider::Claude => self.key_present(KEY_ANTHROPIC),
            SummaryProvider::Openai => {
                cfg.current().base_url.is_some_and(|u| !u.trim().is_empty())
            }
            SummaryProvider::Ollama => true,
        }
    }

    /// Label for the "what left the device" note. `"local"` means nothing left the computer.
    pub fn egress_label(&self) -> String {
        let cfg = read_prefs(&self.prefs).summary;
        match cfg.provider {
            SummaryProvider::Local => "local".to_owned(),
            SummaryProvider::Claude => "Claude".to_owned(),
            SummaryProvider::Ollama => {
                let url = cfg.current().base_url.unwrap_or_default();
                if url.is_empty() || is_loopback(&url) {
                    "local".to_owned()
                } else {
                    "Ollama".to_owned()
                }
            }
            SummaryProvider::Openai => {
                let url = cfg.current().base_url.unwrap_or_default();
                if is_loopback(&url) {
                    "local".to_owned()
                } else {
                    host_of(&url).unwrap_or_else(|| "OpenAI".to_owned())
                }
            }
        }
    }

    /// Picks the installed LLM: the user's choice, else the best the predictor rates (verdict first,
    /// then the larger model).
    async fn pick_llm(&self) -> Result<ModelSpec> {
        let chosen = read_prefs(&self.prefs).llm;
        if let Some(id) = chosen
            && let Ok(spec) = self.hub.spec(&id)
            && self.hub.is_installed(spec)
        {
            return Ok(spec.clone());
        }
        let installed = self.hub.installed_of(ModelKind::Llm);
        let mut best: Option<(u8, f64, &ModelSpec)> = None;
        for spec in installed {
            let rank = match self.insights.predict(spec).await.map(|p| p.verdict) {
                Ok(Verdict::Great) => 0,
                Ok(Verdict::Ok) => 1,
                Ok(Verdict::Slow) => 2,
                Ok(Verdict::WontRun) => 3,
                Err(_) => 2,
            };
            let size = spec.cost.total_params;
            let better = match &best {
                None => true,
                Some((r, s, _)) => rank < *r || (rank == *r && size > *s),
            };
            if better {
                best = Some((rank, size, spec));
            }
        }
        best.map(|(_, _, s)| s.clone())
            .ok_or_else(|| Error::ModelMissing("gemma-4".to_owned()))
    }

    async fn local_provider(&self) -> Result<Box<dyn ChatProvider>> {
        let spec = self.pick_llm().await?;
        let hardware = self.insights.hardware().await.ok();
        let (_, runtime_dir, _) = self
            .hub
            .installed_runtime(hardware)
            .ok_or_else(|| Error::ModelMissing("llama.cpp runtime".to_owned()))?;
        let gguf = self.hub.gguf_path(&spec)?;
        let mut slot = self.server.lock().await;
        if let Some(s) = slot.as_ref()
            && (s.model_id != spec.id || !server_alive(&s.server).await)
        {
            *slot = None;
        }
        if slot.is_none() {
            let mut cfg = LocalServerConfig::new(runtime_dir, gguf);
            cfg.idle_timeout = SERVER_IDLE;
            let server = LocalServer::start(cfg, |_| {}).await?;
            *slot = Some(ServerSlot {
                server,
                model_id: spec.id.clone(),
            });
        }
        slot.as_ref()
            .map(|s| s.server.provider())
            .transpose()?
            .ok_or_else(|| Error::Llm("local server did not start".to_owned()))
    }

    /// A chat provider for the selected setting.
    pub async fn provider(&self) -> Result<Box<dyn ChatProvider>> {
        let cfg = read_prefs(&self.prefs).summary;
        let entry = cfg.current();
        match cfg.provider {
            SummaryProvider::Local => self.local_provider().await,
            SummaryProvider::Claude => ProviderConfig::Anthropic { model: entry.model }.build(),
            SummaryProvider::Openai => {
                let base_url = entry
                    .base_url
                    .filter(|u| !u.trim().is_empty())
                    .unwrap_or_else(|| "https://api.openai.com/v1".to_owned());
                ProviderConfig::OpenAiCompatible {
                    provider_id: KEY_OPENAI.to_owned(),
                    base_url,
                    model: entry.model,
                }
                .build()
            }
            SummaryProvider::Ollama => {
                let base = entry
                    .base_url
                    .filter(|u| !u.trim().is_empty())
                    .unwrap_or_else(|| "http://localhost:11434".to_owned());
                ProviderConfig::Ollama {
                    model: entry.model,
                    base_url: with_v1(&base),
                }
                .build()
            }
        }
    }

    /// Stops the local server (app exit / provider switch).
    pub async fn stop_server(&self) {
        self.server.lock().await.take();
    }

    /// Catalog id of the installed LLM that would be used now.
    pub async fn chosen_installed(&self) -> Option<String> {
        self.pick_llm().await.ok().map(|s| s.id)
    }
}

#[async_trait]
impl Summarizer for Llm {
    fn is_configured(&self) -> bool {
        self.configured()
    }

    async fn summarize(
        &self,
        input: EngineSummaryInput,
        progress: &(dyn Fn(f32) + Send + Sync),
    ) -> Result<Summary> {
        let provider = self.provider().await?;
        let converted = to_llm_input(&input);
        let report = |p: yc_llm::SummaryProgress| {
            progress(if p.total_windows == 0 {
                0.0
            } else {
                (p.completed_windows as f32 / p.total_windows as f32).min(0.95)
            });
        };
        let summary =
            yc_llm::summarize(&*provider, &converted, &SummaryOptions::default(), report).await?;
        progress(1.0);
        Ok(summary)
    }
}

#[async_trait]
impl Answerer for Llm {
    async fn answer(&self, request: AnswerRequest, out: mpsc::Sender<AnswerEvent>) -> Result<()> {
        let provider = self.provider().await?;
        let label = self.egress_label();
        let n = request.excerpts.len();
        let note = if request.language.to_ascii_lowercase().starts_with("en") {
            if label == "local" {
                "Nothing left this computer".to_owned()
            } else {
                format!("{n} text excerpts sent to {label}, no audio")
            }
        } else if label == "local" {
            "Ничего не покинуло компьютер".to_owned()
        } else {
            format!("{n} фрагментов текста отправлено в {label}, аудио — нет")
        };
        if out.send(AnswerEvent::Egress(note)).await.is_err() {
            return Ok(());
        }
        let hits = request
            .excerpts
            .iter()
            .map(|e| yc_llm::ContextHit {
                meeting_id: e.meeting_id,
                meeting_title: e.meeting_title.clone(),
                date: e.started_at.chars().take(10).collect(),
                speaker: e.speakers.join(", "),
                start_ms: e.start_ms,
                text: e.text.clone(),
            })
            .collect::<Vec<_>>();
        let (delta_tx, mut delta_rx) = mpsc::unbounded_channel::<String>();
        let ask = answer_question(&*provider, &request.question, hits, move |d| {
            let _ = delta_tx.send(d.to_owned());
        });
        let forward = async {
            while let Some(d) = delta_rx.recv().await {
                if out.send(AnswerEvent::Token(d)).await.is_err() {
                    break;
                }
            }
        };
        let (answer, ()) = tokio::join!(ask, forward);
        let answer = answer?;
        let cited = answer
            .citations
            .iter()
            .filter_map(|c| c.n.checked_sub(1))
            .collect::<Vec<_>>();
        let _ = out.send(AnswerEvent::Citations(cited)).await;
        let _ = out.send(AnswerEvent::Done).await;
        Ok(())
    }
}

/// Engine summary input -> `yc-llm` summary input (speaker keys become display names).
pub fn to_llm_input(input: &EngineSummaryInput) -> SummaryInput {
    let english = input.language.to_ascii_lowercase().starts_with("en");
    let name_of = |key: &str| -> String {
        input
            .participants
            .iter()
            .find(|p| p.key == key)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| match key {
                "me" if english => "Me".to_owned(),
                "me" => "Я".to_owned(),
                "them" if english => "Them".to_owned(),
                "them" => "Собеседник".to_owned(),
                other => other.to_owned(),
            })
    };
    SummaryInput {
        meeting: MeetingMetadata {
            id: Some(input.meeting_id),
            title: input.title.clone(),
            started_at: input.started_at,
            duration_ms: input.duration_ms,
        },
        turns: input
            .turns
            .iter()
            .map(|t| SummaryTurn {
                speaker: name_of(&t.speaker),
                start_ms: t.start_ms,
                text: t.text.clone(),
            })
            .collect(),
        bookmarks: input.bookmarks.clone(),
        language: None,
        template: SummaryTemplate::Standard,
        meeting_language: input.language.clone(),
    }
}

async fn server_alive(server: &LocalServer) -> bool {
    let url = server.base_url();
    let Some(addr) = url.strip_prefix("http://") else {
        return false;
    };
    let addr = addr.to_owned();
    tokio::task::spawn_blocking(move || {
        use std::net::ToSocketAddrs;
        addr.to_socket_addrs()
            .ok()
            .and_then(|mut a| a.next())
            .is_some_and(|a| {
                std::net::TcpStream::connect_timeout(&a, Duration::from_millis(500)).is_ok()
            })
    })
    .await
    .unwrap_or(false)
}

fn with_v1(base: &str) -> String {
    let b = base.trim_end_matches('/');
    if b.ends_with("/v1") {
        b.to_owned()
    } else {
        format!("{b}/v1")
    }
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1)?;
    let host = rest.split(['/', ':']).next()?;
    (!host.is_empty()).then(|| host.to_owned())
}

fn is_loopback(url: &str) -> bool {
    host_of(url).is_some_and(|h| matches!(h.as_str(), "localhost" | "127.0.0.1" | "::1" | "[::1]"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_normalised() {
        assert_eq!(with_v1("http://localhost:11434"), "http://localhost:11434/v1");
        assert_eq!(with_v1("http://x/v1/"), "http://x/v1");
        assert!(is_loopback("http://localhost:1234/v1"));
        assert!(!is_loopback("https://api.example.com/v1"));
        assert_eq!(host_of("https://api.example.com/v1").as_deref(), Some("api.example.com"));
    }
}
