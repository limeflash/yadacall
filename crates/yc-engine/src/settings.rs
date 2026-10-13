//! User settings: typed, serde with defaults, persisted as one JSON value in the `settings` table.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Key under which [`Settings`] is stored in the yc-store settings table.
pub const SETTINGS_KEY: &str = "settings";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// Language of the speech in meetings (not of the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeetingLanguage {
    Auto,
    #[default]
    Ru,
    En,
}

impl MeetingLanguage {
    /// The code handed to the recogniser and stored in `meeting.json`.
    pub fn code(self) -> &'static str {
        match self {
            MeetingLanguage::Auto => "auto",
            MeetingLanguage::Ru => "ru",
            MeetingLanguage::En => "en",
        }
    }
}

/// How long call audio stays on disk after the transcript is ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeepAudio {
    /// Delete as soon as the transcript is safely written.
    None,
    Days7,
    #[default]
    Days30,
    Forever,
}

impl KeepAudio {
    /// `Some(days)` for time-limited retention, `None` for forever. [`KeepAudio::None`] is `Some(0)`.
    pub fn days(self) -> Option<u32> {
        match self {
            KeepAudio::None => Some(0),
            KeepAudio::Days7 => Some(7),
            KeepAudio::Days30 => Some(30),
            KeepAudio::Forever => Option::None,
        }
    }
}

/// Global shortcut accelerators (Tauri syntax); empty = unset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Hotkeys {
    pub toggle_recording: String,
    pub bookmark: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            toggle_recording: "CommandOrControl+Shift+R".to_owned(),
            bookmark: "CommandOrControl+Shift+B".to_owned(),
        }
    }
}

/// Catalog ids of the models in use, per kind. `None` = let the app pick the best installed one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActiveModels {
    pub asr: Option<String>,
    pub vad: Option<String>,
    pub speaker: Option<String>,
    pub text_embedding: Option<String>,
    pub llm: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub theme: Theme,
    /// Accent colour key chosen in the UI.
    pub accent: String,
    /// UI language (`ru`, `en`); also picks the language of engine notices.
    pub ui_language: String,
    pub meeting_language: MeetingLanguage,
    pub keep_audio: KeepAudio,
    /// Library folder; applied the next time the engine opens.
    pub library_dir: Option<PathBuf>,
    pub hotkeys: Hotkeys,
    pub launch_at_login: bool,
    /// Remind the user to record when a call app starts using the microphone.
    pub recording_reminder: bool,
    /// Match speakers against saved voices and learn from renamed speakers.
    pub recognise_voices: bool,
    pub models: ActiveModels,
    /// Opaque summary-provider configuration owned by the LLM layer.
    pub summary_provider: serde_json::Value,
    /// Microphone device id/name; `None` = system default.
    pub mic_device: Option<String>,
    /// Use the built-in microphone instead of a Bluetooth headset's (keeps its audio quality high).
    pub prefer_builtin_mic: bool,
    /// Minimum cosine similarity of a speaker centroid to a saved voice to adopt its name.
    pub voice_match_threshold: f32,
    /// Diarisation merges clusters while their average cosine distance is below this.
    pub speaker_cluster_threshold: f32,
    /// After diarisation, speaker clusters whose voices are at least this cosine-similar are
    /// merged into one person.
    pub speaker_merge_similarity: f32,
    /// The user's own terms (product names, jargon) that the recogniser should spell correctly.
    pub terms: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            accent: "indigo".to_owned(),
            ui_language: "ru".to_owned(),
            meeting_language: MeetingLanguage::Ru,
            keep_audio: KeepAudio::Days30,
            library_dir: None,
            hotkeys: Hotkeys::default(),
            launch_at_login: false,
            recording_reminder: true,
            recognise_voices: true,
            models: ActiveModels::default(),
            summary_provider: serde_json::Value::Null,
            mic_device: None,
            prefer_builtin_mic: true,
            voice_match_threshold: 0.6,
            speaker_cluster_threshold: 0.55,
            speaker_merge_similarity: crate::diarize::DEFAULT_MERGE_SIMILARITY,
            terms: Vec::new(),
        }
    }
}

impl Settings {
    /// Whether engine notices should be written in Russian.
    pub fn russian(&self) -> bool {
        !self.ui_language.to_ascii_lowercase().starts_with("en")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_json_fills_defaults() {
        let s: Settings =
            serde_json::from_str(r#"{"keepAudio":"forever","hotkeys":{"bookmark":"F9"}}"#).unwrap();
        assert_eq!(s.keep_audio, KeepAudio::Forever);
        assert_eq!(s.hotkeys.bookmark, "F9");
        assert_eq!(
            s.hotkeys.toggle_recording,
            Hotkeys::default().toggle_recording
        );
        assert_eq!(s.meeting_language, MeetingLanguage::Ru);
        assert_eq!(s.keep_audio.days(), None);
        assert_eq!(KeepAudio::None.days(), Some(0));
    }

    #[test]
    fn round_trips() {
        let mut s = Settings::default();
        s.models.asr = Some("gigaam-v3".into());
        s.summary_provider = serde_json::json!({"kind": "local"});
        let back: Settings = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}
