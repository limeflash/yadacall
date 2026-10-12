//! Mapping between the UI's [`Settings`] and the engine's [`yc_engine::Settings`], plus the few
//! app-level preferences the engine has no place for (`app-prefs.json`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use yc_engine::settings as es;

use crate::adapters::SummaryConfig;
use crate::api::{KeepAudio, Settings, SettingsPatch, SummarySettings, ThemeChoice, UiLanguage};
use crate::paths;

/// What we learned about the system-audio permission (macOS has no query API).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SystemAudioKnown {
    #[default]
    Unknown,
    Granted,
    Denied,
}

/// App-level preferences stored next to the index in `app-prefs.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppPrefs {
    pub onboarded: bool,
    pub mini_player: bool,
    pub system_audio: SystemAudioKnown,
}

impl Default for AppPrefs {
    fn default() -> Self {
        Self {
            onboarded: false,
            mini_player: true,
            system_audio: SystemAudioKnown::Unknown,
        }
    }
}

impl AppPrefs {
    /// Reads the file; defaults when missing or damaged.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Writes the file atomically (errors are only logged: these are conveniences).
    pub fn save(&self, path: &Path) {
        let tmp = path.with_extension("json.tmp");
        let ok = serde_json::to_vec_pretty(self)
            .map_err(|e| e.to_string())
            .and_then(|b| std::fs::write(&tmp, b).map_err(|e| e.to_string()))
            .and_then(|()| std::fs::rename(&tmp, path).map_err(|e| e.to_string()));
        if let Err(e) = ok {
            tracing::warn!("could not save {}: {e}", path.display());
        }
    }
}

/// Settings written the first time the app runs (the engine's own defaults suit tests, not the UI).
pub fn first_run(s: &mut es::Settings, library: &Path, russian: bool) {
    s.ui_language = if russian { "ru" } else { "en" }.to_owned();
    s.meeting_language = if russian {
        es::MeetingLanguage::Ru
    } else {
        es::MeetingLanguage::Auto
    };
    s.theme = es::Theme::Dark;
    s.accent = "#7dd3a0".to_owned();
    s.hotkeys.toggle_recording = "Alt+Shift+R".to_owned();
    s.hotkeys.bookmark = "Alt+Shift+M".to_owned();
    s.library_dir = Some(library.to_path_buf());
    s.summary_provider =
        serde_json::to_value(SummaryConfig::default()).unwrap_or(serde_json::Value::Null);
}

/// Engine settings -> UI settings.
pub fn to_api(
    s: &es::Settings,
    effective_library: &Path,
    app: &AppPrefs,
    cfg: &SummaryConfig,
    api_key_set: bool,
) -> Settings {
    let entry = cfg.current();
    Settings {
        theme: match s.theme {
            es::Theme::System => ThemeChoice::System,
            es::Theme::Light => ThemeChoice::Light,
            es::Theme::Dark => ThemeChoice::Dark,
        },
        accent: s.accent.clone(),
        ui_language: if s.russian() {
            UiLanguage::Ru
        } else {
            UiLanguage::En
        },
        meeting_language: s.meeting_language.code().to_owned(),
        keep_audio: match s.keep_audio {
            es::KeepAudio::None => KeepAudio::None,
            es::KeepAudio::Days7 => KeepAudio::Days7,
            es::KeepAudio::Days30 => KeepAudio::Days30,
            es::KeepAudio::Forever => KeepAudio::Forever,
        },
        library_dir: paths::collapse_home(s.library_dir.as_deref().unwrap_or(effective_library)),
        hotkey_record: s.hotkeys.toggle_recording.clone(),
        hotkey_bookmark: s.hotkeys.bookmark.clone(),
        launch_at_login: s.launch_at_login,
        recording_reminder: s.recording_reminder,
        recognize_voices: s.recognise_voices,
        mini_player: app.mini_player,
        summaries: SummarySettings {
            provider: cfg.provider,
            model: entry.model,
            base_url: entry.base_url,
            api_key_set,
        },
        onboarded: app.onboarded,
    }
}

/// Applies the plain (non-summary) fields of a patch to the engine settings and the app prefs.
pub fn apply_patch(s: &mut es::Settings, app: &mut AppPrefs, p: &SettingsPatch) {
    if let Some(v) = p.theme {
        s.theme = match v {
            ThemeChoice::System => es::Theme::System,
            ThemeChoice::Light => es::Theme::Light,
            ThemeChoice::Dark => es::Theme::Dark,
        };
    }
    if let Some(v) = &p.accent {
        s.accent = v.clone();
    }
    if let Some(v) = p.ui_language {
        s.ui_language = match v {
            UiLanguage::Ru => "ru",
            UiLanguage::En => "en",
        }
        .to_owned();
    }
    if let Some(v) = &p.meeting_language {
        s.meeting_language = match v.as_str() {
            "ru" => es::MeetingLanguage::Ru,
            "en" => es::MeetingLanguage::En,
            _ => es::MeetingLanguage::Auto,
        };
    }
    if let Some(v) = p.keep_audio {
        s.keep_audio = match v {
            KeepAudio::None => es::KeepAudio::None,
            KeepAudio::Days7 => es::KeepAudio::Days7,
            KeepAudio::Days30 => es::KeepAudio::Days30,
            KeepAudio::Forever => es::KeepAudio::Forever,
        };
    }
    if let Some(v) = &p.library_dir
        && !v.trim().is_empty()
    {
        s.library_dir = Some(paths::expand_tilde(v));
    }
    if let Some(v) = &p.hotkey_record {
        s.hotkeys.toggle_recording = v.clone();
    }
    if let Some(v) = &p.hotkey_bookmark {
        s.hotkeys.bookmark = v.clone();
    }
    if let Some(v) = p.launch_at_login {
        s.launch_at_login = v;
    }
    if let Some(v) = p.recording_reminder {
        s.recording_reminder = v;
    }
    if let Some(v) = p.recognize_voices {
        s.recognise_voices = v;
    }
    if let Some(v) = p.mini_player {
        app.mini_player = v;
    }
    if let Some(v) = p.onboarded {
        app.onboarded = v;
    }
}

/// Applies the summary part of a patch. Returns the new config; the key (if any) is for the caller to
/// store in the keychain (it is never kept here).
pub fn apply_summary_patch(cfg: &mut SummaryConfig, p: &crate::api::SummaryPatch) {
    if let Some(provider) = p.provider {
        cfg.provider = provider;
    }
    let provider = cfg.provider;
    let entry = cfg
        .entries
        .entry(SummaryConfig::key(provider).to_owned())
        .or_default();
    if let Some(m) = &p.model {
        entry.model = m.trim().to_owned();
    }
    if let Some(u) = &p.base_url {
        let u = u.trim();
        entry.base_url = (!u.is_empty()).then(|| u.to_owned());
    }
}

/// The catalog id an on-device model choice of the UI ("gemma-4-e4b") stands for: an exact id, else the
/// first catalog id that continues it ("gemma-4-e4b-q4-0").
pub fn local_model_choice(ids: &[&str], value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let prefix = format!("{value}-");
    ids.iter()
        .find(|id| **id == value)
        .or_else(|| ids.iter().find(|id| id.starts_with(&prefix)))
        .map(|id| (*id).to_owned())
}

/// Location of the app prefs file.
pub fn prefs_path(data_dir: &Path) -> PathBuf {
    data_dir.join("app-prefs.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::SummaryProvider;

    #[test]
    fn local_choice_matches_catalog_ids() {
        let ids = ["gemma-4-e2b-q4-0", "gemma-4-e4b-q4-0", "gemma-4-e4b-q8-0"];
        assert_eq!(
            local_model_choice(&ids, "gemma-4-e4b").as_deref(),
            Some("gemma-4-e4b-q4-0")
        );
        assert_eq!(
            local_model_choice(&ids, "gemma-4-e4b-q8-0").as_deref(),
            Some("gemma-4-e4b-q8-0")
        );
        assert_eq!(local_model_choice(&ids, "llama").as_deref(), None);
    }

    #[test]
    fn first_run_and_back() {
        let mut s = es::Settings::default();
        first_run(&mut s, Path::new("/tmp/lib"), false);
        let cfg = SummaryConfig::from_value(&s.summary_provider).unwrap();
        let api = to_api(&s, Path::new("/tmp/lib"), &AppPrefs::default(), &cfg, false);
        assert_eq!(api.ui_language, UiLanguage::En);
        assert_eq!(api.meeting_language, "auto");
        assert_eq!(api.summaries.provider, SummaryProvider::Local);
        assert!(!api.onboarded);
    }
}
