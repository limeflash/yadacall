//! Backend boundary of the desktop shell: DTOs (camelCase, mirrored by `ui/src/lib/api/types.ts`) and the
//! [`Backend`] trait that covers everything the UI can ask of the engine.
//!
//! `demo.rs` implements it in memory with the design's demo data; the real engine (`yc-engine`) will implement
//! the same trait. Commands in `commands.rs` are one-liners into it. Domain types (`Meeting`, `Turn`, `Summary`,
//! `EngineEvent`, …) come straight from `yc-core` so UI, engine and CLI share one vocabulary.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use yc_core::Ms;
use yc_core::events::{DownloadState, EngineEvent};
use yc_core::meeting::{Meeting, MeetingStatus};
use yc_core::summary::Summary;
use yc_core::transcript::Turn;
pub use yc_engine::RenameOutcome;

/// Pushes engine events to the UI (global `engine` event, a Tauri `Channel`, …).
pub type EventSink = Arc<dyn Fn(EngineEvent) + Send + Sync>;
/// Streams an answer to the UI token by token.
pub type AskSink = Arc<dyn Fn(AskEvent) + Send + Sync>;

pub type Result<T, E = yc_core::Error> = std::result::Result<T, E>;

// ---- meetings -----------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingListItem {
    pub id: String,
    pub title: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub duration_ms: Ms,
    pub status: MeetingStatus,
    /// One-line gist shown under the title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// e.g. "audio deleted after 7 days, text and summary kept".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingDetail {
    pub meeting: Meeting,
    pub turns: Vec<Turn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
    /// `asset://` URL of the mixed audio when it is still on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_url: Option<String>,
    pub has_audio: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    Md,
    Json,
}

// ---- recorder -----------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecorderStartOptions {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub expected_speakers: Option<u8>,
    #[serde(default)]
    pub mic_device: Option<String>,
    /// People invited to the calendar event behind the recording (names and emails).
    #[serde(default)]
    pub participant_hints: Vec<yc_core::meeting::ParticipantHint>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RecorderPhase {
    #[default]
    Idle,
    Recording,
    Paused,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecorderSnapshot {
    pub phase: RecorderPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub elapsed_ms: Ms,
    pub bookmarks: Vec<Ms>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mic_device: Option<String>,
    /// App whose audio is captured ("Zoom" / "System").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_source: Option<String>,
}

// ---- search / ask / tasks / import -------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub meeting_id: String,
    pub title: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub at_ms: Ms,
    pub speaker: String,
    pub text: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    pub n: u32,
    pub meeting_id: String,
    pub meeting_title: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub at_ms: Ms,
    pub speaker: String,
    pub quote: String,
}

/// What left the device for an answer: the UI always states it ("6 text excerpts sent to Claude, no audio").
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SentNote {
    pub excerpts: u32,
    pub provider: String,
    pub audio: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum AskEvent {
    Token {
        text: String,
    },
    Citations {
        items: Vec<Citation>,
    },
    /// `generated` is false when summaries are off: only the matching passages were found.
    Done {
        sent: SentNote,
        #[serde(default = "yes")]
        generated: bool,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskItem {
    pub meeting_id: String,
    pub meeting_title: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub index: usize,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<Ms>,
    pub done: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ImportState {
    Queued,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportJob {
    pub id: String,
    pub file_name: String,
    pub state: ImportState,
    pub progress: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---- models -------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ModelKind {
    Asr,
    Voice,
    Embed,
    Llm,
}

/// Predictor verdict (`yc-hub`): great / ok / slow / won't run.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Great,
    Ok,
    Slow,
    WontRun,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDownload {
    pub state: DownloadState,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub bytes_per_sec: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_secs: Option<u64>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    pub id: String,
    pub kind: ModelKind,
    pub name: String,
    pub size_bytes: u64,
    /// Short note: "accurate, RU and EN".
    pub blurb: String,
    pub installed: bool,
    pub active: bool,
    pub verdict: Verdict,
    /// One-line estimate: "≈ 40× faster than real time", "≈ 25 tok/s · 6.1 of 16 GB".
    pub estimate: String,
    /// Whether "measure" makes sense for this model (not for VAD or runtime bundles).
    #[serde(default = "yes")]
    pub benchmarkable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download: Option<ModelDownload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareProfile {
    pub label: String,
    pub chip: String,
    pub ram_gb: u32,
    pub backend: String,
    pub disk_free_bytes: u64,
    pub models_bytes: u64,
}

/// The llama.cpp runtime the local LLM runs on. It has no card of its own: it is installed on demand
/// and shown discreetly under the LLM section. `id` goes to `model_download` to reinstall it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    pub id: String,
    /// "llama.cpp b8639".
    pub name: String,
    pub size_bytes: u64,
    pub installed: bool,
    /// An install or repair is running; its progress arrives as `download` events with `item == id`.
    pub busy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsOverview {
    pub hardware: HardwareProfile,
    pub models: Vec<ModelEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkResult {
    pub model_id: String,
    pub realtime_factor: f32,
}

// ---- settings / permissions ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ThemeChoice {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum UiLanguage {
    Ru,
    En,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum KeepAudio {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "7d")]
    Days7,
    #[serde(rename = "30d")]
    Days30,
    #[serde(rename = "forever")]
    Forever,
}

/// Where an Ollama request goes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OllamaBase {
    /// The daemon on this computer (also serves the user's `:cloud` models under their account).
    Local,
    /// Ollama Cloud's own API with a key.
    Cloud,
}

/// Hosted or self-run API behind the "API" summary mode.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ApiProvider {
    Anthropic,
    OpenAiCompatible,
    OpenRouter,
    LmStudio,
}

/// How meeting summaries (and answers in Ask) are produced. Also the persisted form.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "mode",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SummaryMode {
    /// No summaries; Ask lists the matching passages without a generated answer.
    Off,
    /// Gemma on this computer; `model` is a catalog id prefix such as `gemma-4-e4b` (empty = automatic).
    Local {
        #[serde(default)]
        model: String,
    },
    /// Ollama (the local daemon, or Ollama Cloud with a key).
    Ollama { base: OllamaBase, model: String },
    /// A chat API with the user's own key.
    Api {
        provider: ApiProvider,
        #[serde(default)]
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_url: Option<String>,
    },
}

impl Default for SummaryMode {
    fn default() -> Self {
        Self::Local {
            model: String::new(),
        }
    }
}

/// Keychain accounts the UI can write.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum KeyAccount {
    Anthropic,
    OpenAi,
    OpenRouter,
    OllamaCloud,
}

/// Which keys are stored (the values never leave the keychain).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KeysSet {
    pub anthropic: bool,
    pub open_ai: bool,
    pub open_router: bool,
    pub ollama_cloud: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummarySettings {
    #[serde(flatten)]
    pub mode: SummaryMode,
    pub keys_set: KeysSet,
}

/// What a probe of Ollama found (local daemon, or Ollama Cloud with the stored key).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OllamaStatus {
    pub running: bool,
    pub version: Option<String>,
    /// The daemon is signed in to an Ollama account (cloud models need it). Always true for the cloud API.
    pub signed_in: bool,
    pub models: Vec<yc_llm::OllamaModel>,
    /// Why Ollama Cloud could not be listed (no key, rejected key, no network).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Result of the "Проверить" button: one short completion with the selected provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTest {
    pub ok: bool,
    pub latency_ms: u64,
    pub reply: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub theme: ThemeChoice,
    pub accent: String,
    pub ui_language: UiLanguage,
    /// `auto`, `ru`, `en`.
    pub meeting_language: String,
    pub keep_audio: KeepAudio,
    pub library_dir: String,
    pub hotkey_record: String,
    pub hotkey_bookmark: String,
    pub launch_at_login: bool,
    pub recording_reminder: bool,
    pub recognize_voices: bool,
    pub mini_player: bool,
    /// The user's own terms (product names, jargon) spelled correctly in transcripts.
    pub terms: Vec<String>,
    pub summaries: SummarySettings,
    pub onboarded: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::Dark,
            accent: "#7dd3a0".into(),
            ui_language: UiLanguage::Ru,
            meeting_language: "auto".into(),
            keep_audio: KeepAudio::Days30,
            library_dir: "~/YadaCall".into(),
            hotkey_record: "Alt+Shift+R".into(),
            hotkey_bookmark: "Alt+Shift+M".into(),
            launch_at_login: false,
            recording_reminder: true,
            recognize_voices: true,
            mini_player: true,
            terms: Vec::new(),
            summaries: SummarySettings {
                mode: SummaryMode::Local {
                    model: String::new(),
                },
                keys_set: KeysSet::default(),
            },
            onboarded: true,
        }
    }
}

/// Partial update (`undefined` fields are left alone). `summaries.api_key` goes to the OS keychain in the real
/// backend and is never echoed back.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SettingsPatch {
    pub theme: Option<ThemeChoice>,
    pub accent: Option<String>,
    pub ui_language: Option<UiLanguage>,
    pub meeting_language: Option<String>,
    pub keep_audio: Option<KeepAudio>,
    pub library_dir: Option<String>,
    pub hotkey_record: Option<String>,
    pub hotkey_bookmark: Option<String>,
    pub launch_at_login: Option<bool>,
    pub recording_reminder: Option<bool>,
    pub recognize_voices: Option<bool>,
    pub mini_player: Option<bool>,
    pub terms: Option<Vec<String>>,
    pub onboarded: Option<bool>,
    pub summaries: Option<SummaryPatch>,
}

/// A key to store in (or, when `value` is empty, remove from) the OS keychain; never echoed back.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyPatch {
    pub account: KeyAccount,
    pub value: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SummaryPatch {
    pub mode: Option<SummaryMode>,
    pub key: Option<KeyPatch>,
}

impl Settings {
    /// Applies a patch; returns the new settings.
    pub fn apply(mut self, p: SettingsPatch) -> Self {
        macro_rules! set { ($($f:ident),*) => { $( if let Some(v) = p.$f { self.$f = v; } )* }; }
        set!(
            theme,
            accent,
            ui_language,
            meeting_language,
            keep_audio,
            library_dir,
            hotkey_record,
            hotkey_bookmark,
            launch_at_login,
            recording_reminder,
            recognize_voices,
            mini_player,
            terms,
            onboarded
        );
        if let Some(s) = p.summaries {
            if let Some(mode) = s.mode {
                self.summaries.mode = mode;
            }
            if let Some(k) = s.key {
                let set = !k.value.is_empty();
                let keys = &mut self.summaries.keys_set;
                match k.account {
                    KeyAccount::Anthropic => keys.anthropic = set,
                    KeyAccount::OpenAi => keys.open_ai = set,
                    KeyAccount::OpenRouter => keys.open_router = set,
                    KeyAccount::OllamaCloud => keys.ollama_cloud = set,
                }
            }
        }
        self
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PermissionState {
    Granted,
    Denied,
    NotDetermined,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    pub microphone: PermissionState,
    pub system_audio: PermissionState,
}

pub use yc_core::error::PermissionKind;

// ---- the trait ----------------------------------------------------------------------------------------

/// Everything the UI needs from the engine. Implemented by `demo::DemoBackend` today.
#[async_trait]
pub trait Backend: Send + Sync + 'static {
    // meetings
    async fn list_meetings(&self) -> Result<Vec<MeetingListItem>>;
    async fn get_meeting(&self, id: &str) -> Result<MeetingDetail>;
    async fn rename_meeting(&self, id: &str, title: &str) -> Result<()>;
    async fn delete_meeting(&self, id: &str) -> Result<()>;
    async fn rename_speaker(&self, id: &str, key: &str, name: &str) -> Result<RenameOutcome>;
    async fn set_task_done(&self, id: &str, index: usize, done: bool) -> Result<()>;
    async fn regenerate_summary(&self, id: &str) -> Result<Summary>;
    /// Queues a rebuild of the transcript from stored meeting audio.
    async fn meeting_retranscribe(&self, id: &str) -> Result<()>;
    /// Writes `transcript.md` / `transcript.json` next to the meeting and returns the path.
    async fn export_meeting(&self, id: &str, format: ExportFormat) -> Result<String>;
    /// Folder of a meeting on disk, if it has one (used by "show in folder").
    async fn meeting_dir(&self, id: &str) -> Result<Option<PathBuf>>;
    /// Markdown with front matter, ready to paste into Claude.
    async fn copy_for_claude(&self, id: &str) -> Result<String>;

    // recorder
    async fn recorder_state(&self) -> Result<RecorderSnapshot>;
    async fn recorder_start(&self, opts: RecorderStartOptions) -> Result<RecorderSnapshot>;
    async fn recorder_pause(&self) -> Result<()>;
    async fn recorder_resume(&self) -> Result<()>;
    /// Stops and returns the meeting id; post-call jobs continue in the background (`Status` events).
    async fn recorder_stop(&self) -> Result<String>;
    async fn recorder_cancel(&self) -> Result<()>;
    async fn recorder_bookmark(&self, label: Option<String>) -> Result<()>;
    async fn list_input_devices(&self) -> Result<Vec<InputDevice>>;

    // search / ask / tasks / import
    async fn search(&self, query: &str) -> Result<Vec<SearchHit>>;
    async fn ask(&self, question: &str, sink: AskSink) -> Result<()>;
    async fn list_tasks(&self, include_done: bool) -> Result<Vec<TaskItem>>;
    async fn import_audio(&self, paths: Vec<String>) -> Result<Vec<ImportJob>>;
    async fn list_imports(&self) -> Result<Vec<ImportJob>>;

    // models
    async fn models_overview(&self) -> Result<ModelsOverview>;
    /// Starts or resumes a download; progress arrives as `EngineEvent::Download` on `progress`.
    async fn model_download(&self, id: &str, progress: EventSink) -> Result<()>;
    async fn model_pause(&self, id: &str) -> Result<()>;
    async fn model_cancel(&self, id: &str) -> Result<()>;
    async fn model_remove(&self, id: &str) -> Result<()>;
    async fn model_set_active(&self, id: &str) -> Result<()>;
    async fn model_benchmark(&self, id: &str) -> Result<BenchmarkResult>;

    // summary providers
    /// Probes the local Ollama daemon, or lists Ollama Cloud with the stored key.
    async fn ollama_status(&self, base: OllamaBase) -> Result<OllamaStatus>;
    /// Runs a one-sentence request with `mode` (the saved mode when `None`) and times it.
    async fn test_summary_provider(&self, mode: Option<SummaryMode>) -> Result<ProviderTest>;

    // settings / permissions
    async fn get_settings(&self) -> Result<Settings>;
    async fn update_settings(&self, patch: SettingsPatch) -> Result<Settings>;
    async fn permissions(&self) -> Result<Permissions>;
    async fn request_permission(&self, kind: PermissionKind) -> Result<Permissions>;
    async fn open_system_settings(&self, kind: PermissionKind) -> Result<()>;
}
