//! `yc-llm` behind the engine's [`Summarizer`] and [`Answerer`] ports, per the summary mode setting:
//! off, on-device Gemma through a supervised `llama-server`, Ollama (the local daemon or Ollama Cloud),
//! or a chat API (Anthropic, OpenRouter, LM Studio, any OpenAI-compatible endpoint) with a key from the
//! OS keychain.

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
    ChatMessage, ChatProvider, ChatRequest, LocalServer, LocalServerConfig, MeetingMetadata,
    OLLAMA_CLOUD_ACCOUNT, OLLAMA_CLOUD_URL, OLLAMA_LOCAL_URL, OllamaProvider, ProviderConfig,
    SummaryInput, SummaryOptions, SummaryTemplate, SummaryTurn, answer_question,
};

use super::hub::{Insights, ModelHub};
use super::runtime_keeper::RuntimeKeeper;
use super::{SharedPrefs, read_prefs};
use crate::api::{
    ApiProvider, KeyAccount, KeysSet, OllamaBase, OllamaStatus, ProviderTest, SummaryMode,
};

/// Keychain account of the Anthropic key.
pub const KEY_ANTHROPIC: &str = "anthropic";
/// Keychain account of the OpenAI-compatible key.
pub const KEY_OPENAI: &str = "openai";
/// Keychain account of the OpenRouter key.
pub const KEY_OPENROUTER: &str = "openrouter";
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
    keeper: Arc<RuntimeKeeper>,
    prefs: SharedPrefs,
    server: tokio::sync::Mutex<Option<ServerSlot>>,
    keys: Mutex<HashMap<String, bool>>,
    /// False in tests: the OS keychain is never touched.
    keychain: bool,
}

impl Llm {
    pub fn new(
        hub: Arc<ModelHub>,
        insights: Arc<Insights>,
        keeper: Arc<RuntimeKeeper>,
        prefs: SharedPrefs,
        keychain: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            hub,
            insights,
            keeper,
            prefs,
            server: tokio::sync::Mutex::new(None),
            keys: Mutex::new(HashMap::new()),
            keychain,
        })
    }

    // ---- keychain ---------------------------------------------------------------------------------

    /// Whether a key is stored for `account` (cached; the keychain is read once per session).
    pub fn key_present(&self, account: &str) -> bool {
        if !self.keychain {
            return false;
        }
        let mut cache = self.keys.lock().unwrap_or_else(|e| e.into_inner());
        *cache
            .entry(account.to_owned())
            .or_insert_with(|| yc_llm::get_secret(account).ok().flatten().is_some())
    }

    /// Stores a key in the OS keychain.
    pub fn set_key(&self, account: &str, secret: &str) -> Result<()> {
        if !self.keychain {
            return Err(Error::Invalid("the keychain is disabled".to_owned()));
        }
        yc_llm::set_secret(account, secret)?;
        self.keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(account.to_owned(), true);
        Ok(())
    }

    /// Keychain account of a key the UI can write.
    pub fn account_of(k: KeyAccount) -> &'static str {
        match k {
            KeyAccount::Anthropic => KEY_ANTHROPIC,
            KeyAccount::OpenAi => KEY_OPENAI,
            KeyAccount::OpenRouter => KEY_OPENROUTER,
            KeyAccount::OllamaCloud => OLLAMA_CLOUD_ACCOUNT,
        }
    }

    /// Stores a key, or removes it when `value` is empty.
    pub fn apply_key(&self, k: KeyAccount, value: &str) -> Result<()> {
        let account = Self::account_of(k);
        if !self.keychain {
            return Err(Error::Invalid("the keychain is disabled".to_owned()));
        }
        if value.trim().is_empty() {
            yc_llm::delete_secret(account)?;
            self.keys
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(account.to_owned(), false);
            Ok(())
        } else {
            self.set_key(account, value.trim())
        }
    }

    /// Which keys are in the keychain.
    pub fn keys_set(&self) -> KeysSet {
        KeysSet {
            anthropic: self.key_present(KEY_ANTHROPIC),
            open_ai: self.key_present(KEY_OPENAI),
            open_router: self.key_present(KEY_OPENROUTER),
            ollama_cloud: self.key_present(OLLAMA_CLOUD_ACCOUNT),
        }
    }

    // ---- provider state -----------------------------------------------------------------------------

    /// The saved mode.
    pub fn mode(&self) -> SummaryMode {
        read_prefs(&self.prefs).summary
    }

    /// Whether the selected mode can produce a summary right now. False when summaries are off, so no
    /// summarize job is queued.
    pub fn configured(&self) -> bool {
        match self.mode() {
            SummaryMode::Off => false,
            // The runtime is installed on first use, so only the model has to be there.
            SummaryMode::Local { .. } => !self.hub.installed_of(ModelKind::Llm).is_empty(),
            SummaryMode::Ollama { base, model } => {
                !model.trim().is_empty()
                    && (base == OllamaBase::Local || self.key_present(OLLAMA_CLOUD_ACCOUNT))
            }
            SummaryMode::Api {
                provider,
                model,
                base_url,
            } => match provider {
                ApiProvider::Anthropic => self.key_present(KEY_ANTHROPIC),
                ApiProvider::OpenRouter => {
                    !model.trim().is_empty() && self.key_present(KEY_OPENROUTER)
                }
                ApiProvider::OpenAiCompatible => {
                    !model.trim().is_empty() && base_url.is_some_and(|u| !u.trim().is_empty())
                }
                ApiProvider::LmStudio => !model.trim().is_empty(),
            },
        }
    }

    /// Label for the "what left the device" note. `"local"` means nothing left the computer.
    pub async fn egress_label(&self) -> String {
        match self.mode() {
            SummaryMode::Off | SummaryMode::Local { .. } => "local".to_owned(),
            SummaryMode::Ollama { base, model } => {
                if base == OllamaBase::Cloud || self.daemon_runs_in_cloud(&model).await {
                    "Ollama Cloud".to_owned()
                } else {
                    "local".to_owned()
                }
            }
            SummaryMode::Api {
                provider, base_url, ..
            } => match provider {
                ApiProvider::Anthropic => "Claude".to_owned(),
                ApiProvider::OpenRouter => "OpenRouter".to_owned(),
                ApiProvider::LmStudio | ApiProvider::OpenAiCompatible => {
                    let default_name = if provider == ApiProvider::LmStudio {
                        "LM Studio"
                    } else {
                        "OpenAI"
                    };
                    match base_url {
                        Some(u) if is_loopback(&u) => "local".to_owned(),
                        Some(u) => host_of(&u).unwrap_or_else(|| default_name.to_owned()),
                        None if provider == ApiProvider::LmStudio => "local".to_owned(),
                        None => default_name.to_owned(),
                    }
                }
            },
        }
    }

    /// Whether the local daemon forwards `model` to Ollama's servers (a `:cloud` model of the account).
    async fn daemon_runs_in_cloud(&self, model: &str) -> bool {
        if is_cloud_name(model) {
            return true;
        }
        match OllamaProvider::new(OLLAMA_LOCAL_URL, None, model) {
            Ok(p) => p
                .list_models()
                .await
                .is_ok_and(|ms| ms.iter().any(|m| m.name == model && m.cloud)),
            Err(_) => false,
        }
    }

    /// Probes the local daemon, or lists Ollama Cloud with the stored key.
    pub async fn ollama_status(&self, base: OllamaBase) -> OllamaStatus {
        match base {
            OllamaBase::Local => {
                let p = OllamaProvider::probe(OLLAMA_LOCAL_URL).await;
                OllamaStatus {
                    running: p.running,
                    version: p.version,
                    signed_in: p.signed_in,
                    models: p.models,
                    error: None,
                }
            }
            OllamaBase::Cloud => {
                let down = |error: String| OllamaStatus {
                    running: false,
                    version: None,
                    signed_in: false,
                    models: Vec::new(),
                    error: Some(error),
                };
                let key = match self
                    .keychain
                    .then(|| yc_llm::get_secret(OLLAMA_CLOUD_ACCOUNT))
                    .transpose()
                {
                    Ok(Some(Some(k))) => k,
                    Ok(_) => return down("no key".to_owned()),
                    Err(e) => return down(e.to_string()),
                };
                let listed = match OllamaProvider::new(OLLAMA_CLOUD_URL, Some(key), "") {
                    Ok(p) => p.list_models().await,
                    Err(e) => Err(e),
                };
                match listed {
                    Ok(models) => OllamaStatus {
                        running: true,
                        version: None,
                        signed_in: true,
                        models: models
                            .into_iter()
                            .map(|m| yc_llm::OllamaModel { cloud: true, ..m })
                            .collect(),
                        error: None,
                    },
                    Err(e) => down(e.to_string()),
                }
            }
        }
    }

    /// One short completion with `mode`, timed.
    pub async fn test_provider(&self, mode: &SummaryMode) -> ProviderTest {
        let started = std::time::Instant::now();
        let result = async {
            let provider = self.provider_for(mode).await?;
            let request = ChatRequest::new(
                "",
                vec![ChatMessage::user(
                    "Ответь одним коротким предложением: ты на связи?",
                )],
                200,
            );
            tokio::time::timeout(Duration::from_secs(90), provider.complete(request))
                .await
                .map_err(|_| Error::Network("the provider did not answer in 90 s".to_owned()))?
        }
        .await;
        let latency_ms = started.elapsed().as_millis() as u64;
        match result {
            Ok(r) => ProviderTest {
                ok: !r.text.trim().is_empty(),
                latency_ms,
                reply: Some(r.text.trim().chars().take(160).collect()),
                error: r.text.trim().is_empty().then(|| "empty reply".to_owned()),
            },
            Err(e) => ProviderTest {
                ok: false,
                latency_ms,
                reply: None,
                error: Some(e.to_string()),
            },
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
        // Installs or repairs the llama.cpp runtime first when it is missing or broken.
        let ready = self.keeper.ensure().await?;
        let runtime_dir = self.hub.runtime_dir(&ready.pack);
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

    /// A chat provider for the saved mode.
    pub async fn provider(&self) -> Result<Box<dyn ChatProvider>> {
        self.provider_for(&self.mode()).await
    }

    /// A chat provider for `mode`.
    pub async fn provider_for(&self, mode: &SummaryMode) -> Result<Box<dyn ChatProvider>> {
        match mode {
            SummaryMode::Off => Err(Error::Invalid("summaries are turned off".to_owned())),
            SummaryMode::Local { .. } => self.local_provider().await,
            SummaryMode::Ollama { base, model } => match base {
                OllamaBase::Local => ProviderConfig::Ollama {
                    model: model.clone(),
                    base_url: OLLAMA_LOCAL_URL.to_owned(),
                }
                .build(),
                OllamaBase::Cloud => ProviderConfig::OllamaCloud {
                    model: model.clone(),
                }
                .build(),
            },
            SummaryMode::Api {
                provider,
                model,
                base_url,
            } => {
                let url = base_url.clone().filter(|u| !u.trim().is_empty());
                match provider {
                    ApiProvider::Anthropic => ProviderConfig::Anthropic {
                        model: if model.trim().is_empty() {
                            "claude-sonnet-5-5".to_owned()
                        } else {
                            model.clone()
                        },
                    }
                    .build(),
                    ApiProvider::OpenRouter => ProviderConfig::OpenRouter {
                        model: model.clone(),
                    }
                    .build(),
                    ApiProvider::OpenAiCompatible => ProviderConfig::OpenAiCompatible {
                        provider_id: KEY_OPENAI.to_owned(),
                        base_url: url.unwrap_or_else(|| "https://api.openai.com/v1".to_owned()),
                        model: model.clone(),
                    }
                    .build(),
                    ApiProvider::LmStudio => ProviderConfig::OpenAiCompatible {
                        provider_id: "lmstudio".to_owned(),
                        base_url: url.unwrap_or_else(|| "http://127.0.0.1:1234/v1".to_owned()),
                        model: model.clone(),
                    }
                    .build(),
                }
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
        let n = request.excerpts.len();
        if self.mode() == SummaryMode::Off {
            // No generated answer: hand back the ranked passages as sources.
            let note = if request.language.to_ascii_lowercase().starts_with("en") {
                "Summaries are off: nothing left this computer"
            } else {
                "Итоги выключены: ничего не покинуло компьютер"
            };
            let _ = out.send(AnswerEvent::Egress(note.to_owned())).await;
            let _ = out.send(AnswerEvent::Citations((0..n).collect())).await;
            let _ = out.send(AnswerEvent::Done).await;
            return Ok(());
        }
        let provider = self.provider().await?;
        let label = self.egress_label().await;
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

/// Ollama's naming for models that run on its servers.
fn is_cloud_name(model: &str) -> bool {
    model.ends_with(":cloud") || model.ends_with("-cloud")
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
    fn summary_input_uses_participant_names_and_fallbacks() {
        use yc_core::meeting::{MeetingId, Participant};
        use yc_core::transcript::Turn;
        let input = EngineSummaryInput {
            meeting_id: MeetingId::new(),
            title: "Синк".into(),
            started_at: chrono::Utc::now(),
            language: "ru".into(),
            duration_ms: 60_000,
            turns: vec![
                Turn {
                    corrections: Vec::new(),
                    speaker: "me".into(),
                    start_ms: 0,
                    end_ms: 1000,
                    text: "Привет".into(),
                },
                Turn {
                    corrections: Vec::new(),
                    speaker: "s1".into(),
                    start_ms: 1000,
                    end_ms: 2000,
                    text: "Здравствуй".into(),
                },
                Turn {
                    corrections: Vec::new(),
                    speaker: "them".into(),
                    start_ms: 2000,
                    end_ms: 3000,
                    text: "Ок".into(),
                },
            ],
            participants: vec![Participant {
                key: "s1".into(),
                name: "Иван".into(),
                voice_id: None,
                suggestions: Vec::new(),
            }],
            bookmarks: vec![],
        };
        let out = to_llm_input(&input);
        let names: Vec<_> = out.turns.iter().map(|t| t.speaker.as_str()).collect();
        assert_eq!(names, ["Я", "Иван", "Собеседник"]);
        assert_eq!(out.meeting_language, "ru");
    }

    #[test]
    fn urls_and_model_names_are_classified() {
        assert!(is_cloud_name("glm-5.2:cloud") && is_cloud_name("qwen3:480b-cloud"));
        assert!(!is_cloud_name("gemma4:e4b"));
        assert!(is_loopback("http://localhost:1234/v1"));
        assert!(!is_loopback("https://api.example.com/v1"));
        assert_eq!(
            host_of("https://api.example.com/v1").as_deref(),
            Some("api.example.com")
        );
    }
}
