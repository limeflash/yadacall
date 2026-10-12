//! Telling call apps and browsers apart by process / executable name.

use crate::types::DetectedApp;

/// Maps a process name (macOS process, Windows exe, or a Windows consent-store key such as
/// `C:#Program Files#Zoom#bin#Zoom.exe`) to a call app.
pub fn classify_process(name: &str) -> Option<DetectedApp> {
    let base = name
        .rsplit(['#', '/', '\\'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    if base.contains("zoomit") {
        return None;
    }
    if base == "zoom.us" || base.starts_with("zoom.") || base == "zoom" || base.contains("zoom.us")
    {
        return Some(DetectedApp::Zoom);
    }
    if base.contains("msteams")
        || base.contains("ms-teams")
        || base.starts_with("microsoft teams")
        || base == "teams.exe"
        || base == "teams"
    {
        return Some(DetectedApp::Teams);
    }
    if base.starts_with("telegram") || base == "telegram.exe" {
        return Some(DetectedApp::Telegram);
    }
    None
}

/// Windows consent-store records: `(key name, LastUsedTimeStart, LastUsedTimeStop)`.
/// An app holds the microphone right now when it started and has not stopped (`stop == 0`).
/// Returns the app names (last path component), de-duplicated.
pub fn mic_users_from_consent(entries: &[(String, u64, u64)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (name, start, stop) in entries {
        if *start > 0 && *stop == 0 {
            let base = name.rsplit('#').next().unwrap_or(name).to_string();
            if !out.contains(&base) {
                out.push(base);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_call_apps() {
        for (n, app) in [
            ("zoom.us", DetectedApp::Zoom),
            ("Zoom.exe", DetectedApp::Zoom),
            ("C:#Program Files#Zoom#bin#Zoom.exe", DetectedApp::Zoom),
            ("MSTeams_8wekyb3d8bbwe", DetectedApp::Teams),
            ("ms-teams.exe", DetectedApp::Teams),
            ("Microsoft Teams", DetectedApp::Teams),
            ("Telegram", DetectedApp::Telegram),
            ("C:#Users#me#AppData#Roaming#Telegram Desktop#Telegram.exe", DetectedApp::Telegram),
        ] {
            assert_eq!(classify_process(n), Some(app), "{n}");
        }
        for n in ["ZoomIt.exe", "Finder", "chrome.exe", "Slack"] {
            assert_eq!(classify_process(n), None, "{n}");
        }
    }

    #[test]
    fn consent_store_in_use_means_started_not_stopped() {
        let entries = vec![
            ("Microsoft.WindowsCamera_8wekyb3d8bbwe".to_string(), 10, 20),
            ("C:#Program Files#Zoom#bin#Zoom.exe".to_string(), 133_000_000, 0),
            ("C:#Program Files#Zoom#bin#Zoom.exe".to_string(), 5, 0),
            ("never_used".to_string(), 0, 0),
        ];
        assert_eq!(mic_users_from_consent(&entries), ["Zoom.exe"]);
    }
}
