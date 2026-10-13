//! Telling call apps and browsers apart by process / executable name.

use crate::types::DetectedApp;

/// One browser that can contain a supported call page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowserApp {
    /// Display name used in detection messages.
    pub name: &'static str,
    /// macOS application bundle identifier.
    pub bundle_id: &'static str,
}

const BROWSERS: [BrowserApp; 8] = [
    BrowserApp {
        name: "Chrome",
        bundle_id: "com.google.Chrome",
    },
    BrowserApp {
        name: "Brave",
        bundle_id: "com.brave.Browser",
    },
    BrowserApp {
        name: "Arc",
        bundle_id: "company.thebrowser.Browser",
    },
    BrowserApp {
        name: "Edge",
        bundle_id: "com.microsoft.edgemac",
    },
    BrowserApp {
        name: "Safari",
        bundle_id: "com.apple.Safari",
    },
    BrowserApp {
        name: "Yandex Browser",
        bundle_id: "ru.yandex.desktop.yandex-browser",
    },
    BrowserApp {
        name: "Opera",
        bundle_id: "com.operasoftware.Opera",
    },
    BrowserApp {
        name: "Vivaldi",
        bundle_id: "com.vivaldi.Vivaldi",
    },
];

/// Returns the supported browser for a macOS bundle identifier.
pub fn browser_for_bundle_id(bundle_id: &str) -> Option<BrowserApp> {
    BROWSERS
        .iter()
        .copied()
        .find(|browser| is_bundle_or_helper(bundle_id, browser.bundle_id))
}

/// Returns the supported browser for an executable or ConsentStore process name.
pub fn browser_for_process(name: &str) -> Option<BrowserApp> {
    let base = name
        .rsplit(['#', '/', '\\'])
        .next()
        .unwrap_or(name)
        .trim_end_matches(".exe")
        .to_ascii_lowercase();
    match base.as_str() {
        "chrome" | "google chrome" => browser_for_bundle_id("com.google.Chrome"),
        "brave" | "brave browser" => browser_for_bundle_id("com.brave.Browser"),
        "arc" => browser_for_bundle_id("company.thebrowser.Browser"),
        "msedge" | "microsoft edge" => browser_for_bundle_id("com.microsoft.edgemac"),
        "safari" => browser_for_bundle_id("com.apple.Safari"),
        "yandex" | "browser" | "yandex browser" => {
            browser_for_bundle_id("ru.yandex.desktop.yandex-browser")
        }
        "opera" => browser_for_bundle_id("com.operasoftware.Opera"),
        "vivaldi" => browser_for_bundle_id("com.vivaldi.Vivaldi"),
        _ => None,
    }
}

/// Maps a macOS application bundle identifier to a supported call app.
pub fn classify_bundle_id(bundle_id: &str) -> Option<DetectedApp> {
    [
        ("us.zoom.xos", DetectedApp::Zoom),
        ("com.microsoft.teams2", DetectedApp::Teams),
        ("ru.keepcoder.telegram", DetectedApp::Telegram),
        ("org.telegram.desktop", DetectedApp::Telegram),
        ("com.tinyspeck.slackmacgap", DetectedApp::Slack),
        ("com.hnc.discord", DetectedApp::Discord),
        ("ru.yandex.telemost", DetectedApp::Telemost),
    ]
    .into_iter()
    .find_map(|(known, app)| is_bundle_or_helper(bundle_id, known).then_some(app))
}

fn is_bundle_or_helper(bundle_id: &str, known: &str) -> bool {
    bundle_id.eq_ignore_ascii_case(known)
        || bundle_id
            .get(..known.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(known))
            && bundle_id
                .get(known.len()..)
                .is_some_and(|suffix| suffix.starts_with(".helper"))
}

#[cfg(test)]
mod mapping_tests {
    use super::*;

    #[test]
    fn recognizes_helper_bundle_identifiers() {
        assert_eq!(
            browser_for_bundle_id("com.brave.Browser.helper"),
            browser_for_bundle_id("com.brave.Browser")
        );
        assert_eq!(
            classify_bundle_id("us.zoom.xos.helper"),
            Some(DetectedApp::Zoom)
        );
    }
}

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
    if base.contains("slack") {
        return Some(DetectedApp::Slack);
    }
    if base.contains("discord") {
        return Some(DetectedApp::Discord);
    }
    if base.contains("telemost") {
        return Some(DetectedApp::Telemost);
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
            (
                "C:#Users#me#AppData#Roaming#Telegram Desktop#Telegram.exe",
                DetectedApp::Telegram,
            ),
        ] {
            assert_eq!(classify_process(n), Some(app), "{n}");
        }
        for n in ["ZoomIt.exe", "Finder", "chrome.exe"] {
            assert_eq!(classify_process(n), None, "{n}");
        }
        assert_eq!(classify_process("Slack"), Some(DetectedApp::Slack));
        assert_eq!(classify_process("Discord"), Some(DetectedApp::Discord));
        assert_eq!(classify_process("Yandex Telemost"), Some(DetectedApp::Telemost));
    }

    #[test]
    fn maps_macos_bundle_identifiers() {
        for (bundle_id, expected) in [
            ("us.zoom.xos", DetectedApp::Zoom),
            ("com.microsoft.teams2", DetectedApp::Teams),
            ("ru.keepcoder.Telegram", DetectedApp::Telegram),
            ("org.telegram.desktop", DetectedApp::Telegram),
            ("com.tinyspeck.slackmacgap", DetectedApp::Slack),
            ("com.hnc.Discord", DetectedApp::Discord),
            ("ru.yandex.telemost", DetectedApp::Telemost),
        ] {
            assert_eq!(classify_bundle_id(bundle_id), Some(expected), "{bundle_id}");
        }
        assert_eq!(classify_bundle_id("com.brave.Browser"), None);
    }

    #[test]
    fn maps_supported_browsers_by_bundle_id() {
        for (bundle_id, name) in [
            ("com.google.Chrome", "Chrome"),
            ("com.brave.Browser", "Brave"),
            ("company.thebrowser.Browser", "Arc"),
            ("com.microsoft.edgemac", "Edge"),
            ("com.apple.Safari", "Safari"),
            ("ru.yandex.desktop.yandex-browser", "Yandex Browser"),
            ("com.operasoftware.Opera", "Opera"),
            ("com.vivaldi.Vivaldi", "Vivaldi"),
        ] {
            assert_eq!(browser_for_bundle_id(bundle_id).map(|b| b.name), Some(name));
        }
        assert_eq!(browser_for_bundle_id("com.example.App"), None);
    }

    #[test]
    fn consent_store_in_use_means_started_not_stopped() {
        let entries = vec![
            ("Microsoft.WindowsCamera_8wekyb3d8bbwe".to_string(), 10, 20),
            (
                "C:#Program Files#Zoom#bin#Zoom.exe".to_string(),
                133_000_000,
                0,
            ),
            ("C:#Program Files#Zoom#bin#Zoom.exe".to_string(), 5, 0),
            ("never_used".to_string(), 0, 0),
        ];
        assert_eq!(mic_users_from_consent(&entries), ["Zoom.exe"]);
    }
}
