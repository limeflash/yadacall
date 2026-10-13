//! Parsing browser window titles and call-tab URLs into detected call details.

use std::sync::LazyLock;

use regex::Regex;
use url::Url;

use crate::links::{self, meet_code_of};
use crate::types::{DetectedApp, VideoProvider};

/// A call page found in the browser that currently holds the microphone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallTab {
    /// Provider represented by the call page.
    pub app: DetectedApp,
    /// Provider-specific room code or identifier, when available.
    pub code: Option<String>,
    /// Google Meet room code used for calendar matching.
    pub meet_code: Option<String>,
    /// Full URL read from the tab, when available.
    pub url: Option<String>,
    /// Browser display name.
    pub browser: String,
    /// Tab title, when supplied by the browser.
    pub title: Option<String>,
}

/// A Meet room seen in a browser window title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetWindow {
    /// Room code, absent when the title only says `Meet – <meeting name>`.
    pub code: Option<String>,
    /// Browser the window belongs to, when the title names it.
    pub browser: Option<String>,
}

static MEET_TITLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*meet\s*[-\u{2013}\u{2014}]\s*(?:([a-z]{3}-[a-z]{4}-[a-z]{3})\b|(\S.*))")
        .expect("static regex")
});

const BROWSERS: [(&str, &str); 10] = [
    ("google chrome", "Chrome"),
    ("microsoft\u{200b} edge", "Edge"),
    ("microsoft edge", "Edge"),
    ("brave", "Brave"),
    ("firefox", "Firefox"),
    ("opera", "Opera"),
    ("vivaldi", "Vivaldi"),
    ("yandex", "Yandex Browser"),
    ("arc", "Arc"),
    ("safari", "Safari"),
];

/// Recognises `Meet – xxx-xxxx-xxx - Google Chrome` style titles. A title with a code is accepted
/// anywhere (PWA windows have no browser suffix); a title without one must name a browser, so
/// pages such as `Meet – our team` in an unrelated app do not count.
pub fn parse_meet_title(title: &str) -> Option<MeetWindow> {
    let caps = MEET_TITLE.captures(title)?;
    let lower = title.to_lowercase();
    let browser = BROWSERS
        .iter()
        .find(|(needle, _)| lower.contains(needle))
        .map(|(_, name)| (*name).to_string());
    match caps.get(1) {
        Some(code) => Some(MeetWindow {
            code: Some(code.as_str().to_lowercase()),
            browser,
        }),
        None => browser.map(|b| MeetWindow {
            code: None,
            browser: Some(b),
        }),
    }
}

/// Finds supported call URLs among newline-separated `URL<TAB>title` rows.
///
/// `URL`-only rows are accepted for callers that cannot read tab titles.
pub fn call_tabs_from_output(output: &str, browser: &str) -> Vec<CallTab> {
    let mut out: Vec<CallTab> = Vec::new();
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let (url, title) = line.split_once('\t').unwrap_or((line, ""));
        let url = url.trim();
        let Some(provider) = links::classify(url) else {
            continue;
        };
        if out.iter().any(|tab| tab.url.as_deref() == Some(url)) {
            continue;
        }
        let meet_code = meet_code_of(url);
        out.push(CallTab {
            app: app_for_provider(provider),
            code: call_code(url, provider).or_else(|| meet_code.clone()),
            meet_code,
            url: Some(url.to_owned()),
            browser: browser.to_owned(),
            title: (!title.trim().is_empty()).then(|| title.trim().to_owned()),
        });
    }
    out
}

/// Meet room URLs among newline-separated tab URLs.
pub fn meet_rooms_from_urls(output: &str) -> Vec<(String, String)> {
    let mut rooms = Vec::new();
    for tab in call_tabs_from_output(output, "") {
        let (Some(code), Some(url)) = (tab.meet_code, tab.url) else {
            continue;
        };
        if !rooms.iter().any(|(seen, _)| seen == &code) {
            rooms.push((code, url));
        }
    }
    rooms
}

fn app_for_provider(provider: VideoProvider) -> DetectedApp {
    match provider {
        VideoProvider::Meet => DetectedApp::GoogleMeet,
        VideoProvider::Zoom => DetectedApp::Zoom,
        VideoProvider::Teams => DetectedApp::Teams,
        VideoProvider::Telemost => DetectedApp::Telemost,
        VideoProvider::Jitsi => DetectedApp::Jitsi,
        VideoProvider::Other => DetectedApp::Other("Browser call".into()),
    }
}

fn call_code(url: &str, provider: VideoProvider) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    match provider {
        VideoProvider::Meet => meet_code_of(url),
        VideoProvider::Zoom => parsed
            .path_segments()?
            .find(|segment| !segment.is_empty() && segment.chars().all(|c| c.is_ascii_digit()))
            .map(str::to_owned),
        VideoProvider::Telemost | VideoProvider::Jitsi => parsed
            .path_segments()?
            .rfind(|segment| !segment.is_empty())
            .map(str::to_owned),
        VideoProvider::Teams | VideoProvider::Other => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_with_codes() {
        let w = parse_meet_title("Meet \u{2013} abc-defg-hij - Google Chrome").unwrap();
        assert_eq!(w.code.as_deref(), Some("abc-defg-hij"));
        assert_eq!(w.browser.as_deref(), Some("Chrome"));
        let w =
            parse_meet_title("Meet - abc-defg-hij - Personal - Microsoft\u{200b} Edge").unwrap();
        assert_eq!(w.browser.as_deref(), Some("Edge"));
        let pwa = parse_meet_title("Meet \u{2014} xyz-abcd-efg").unwrap();
        assert_eq!(pwa.code.as_deref(), Some("xyz-abcd-efg"));
        assert_eq!(pwa.browser, None);
    }

    #[test]
    fn codeless_titles_need_a_browser_name() {
        let w = parse_meet_title("Meet - Daily standup - Brave").unwrap();
        assert_eq!(w.code, None);
        assert_eq!(w.browser.as_deref(), Some("Brave"));
        assert_eq!(parse_meet_title("Meet - Daily standup - Notepad"), None);
    }

    #[test]
    fn unrelated_titles_do_not_match() {
        for t in [
            "Meet the team - Company - Google Chrome",
            "Google Meet - Google Chrome",
            "Documentation - Google Chrome",
            "",
        ] {
            assert_eq!(parse_meet_title(t), None, "{t}");
        }
    }

    #[test]
    fn matches_supported_call_urls_and_titles() {
        let output = "https://meet.google.com/abc-defg-hij?authuser=1\tWeekly sync\n\
https://teams.microsoft.com/l/meetup-join/19%3ameeting\tTeams call\n\
https://us02web.zoom.us/wc/123456789/join\tZoom room\n\
https://telemost.yandex.ru/j/room-token\tTelemost\n\
https://meet.jit.si/yada-room\tJitsi\n\
https://example.com/not-a-call\tExample";
        let tabs = call_tabs_from_output(output, "Brave");
        assert_eq!(tabs.len(), 5);
        assert_eq!(tabs[0].app, DetectedApp::GoogleMeet);
        assert_eq!(tabs[0].code.as_deref(), Some("abc-defg-hij"));
        assert_eq!(tabs[0].title.as_deref(), Some("Weekly sync"));
        assert_eq!(tabs[0].browser, "Brave");
        assert_eq!(tabs[1].app, DetectedApp::Teams);
        assert_eq!(tabs[2].app, DetectedApp::Zoom);
        assert_eq!(tabs[2].code.as_deref(), Some("123456789"));
        assert_eq!(tabs[3].app, DetectedApp::Telemost);
        assert_eq!(tabs[4].app, DetectedApp::Jitsi);
    }

    #[test]
    fn meet_rooms_from_urls_deduplicates_tracking_variants() {
        let out = "https://mail.google.com/\nhttps://meet.google.com/abc-defg-hij?authuser=1\n\
https://meet.google.com/landing\nhttps://meet.google.com/abc-defg-hij\n";
        let rooms = meet_rooms_from_urls(out);
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].0, "abc-defg-hij");
        assert!(rooms[0].1.contains("authuser=1"));
    }
}
