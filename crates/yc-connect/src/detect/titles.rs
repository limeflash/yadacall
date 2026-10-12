//! Parsing browser window titles (Windows) and tab URLs (macOS) into Google Meet rooms.

use std::sync::LazyLock;

use regex::Regex;

use crate::links::meet_code_of;

/// A Meet room seen in a browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetWindow {
    /// Room code, absent when the title only says "Meet – <meeting name>".
    pub code: Option<String>,
    /// Browser the window belongs to, when the title names it.
    pub browser: Option<String>,
}

static MEET_TITLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*meet\s*[-\u{2013}\u{2014}]\s*(?:([a-z]{3}-[a-z]{4}-[a-z]{3})\b|(\S.*))")
        .expect("static regex")
});

const BROWSERS: [(&str, &str); 7] = [
    ("google chrome", "Chrome"),
    ("microsoft\u{200b} edge", "Edge"),
    ("microsoft edge", "Edge"),
    ("brave", "Brave"),
    ("firefox", "Firefox"),
    ("opera", "Opera"),
    ("vivaldi", "Vivaldi"),
];

/// Recognises `Meet – xxx-xxxx-xxx - Google Chrome` style titles. A title with a code is accepted
/// anywhere (PWA windows have no browser suffix); a title without one must name a browser, so
/// pages such as "Meet – our team" in an unrelated app do not count.
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

/// Meet rooms among newline-separated tab URLs (the `osascript` output on macOS).
pub fn meet_rooms_from_urls(output: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in output.lines().map(str::trim) {
        if let Some(code) = meet_code_of(line)
            && !out.iter().any(|(c, _)| *c == code)
        {
            out.push((code, line.to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_with_codes() {
        let w = parse_meet_title("Meet \u{2013} abc-defg-hij - Google Chrome").unwrap();
        assert_eq!(w.code.as_deref(), Some("abc-defg-hij"));
        assert_eq!(w.browser.as_deref(), Some("Chrome"));
        let w = parse_meet_title("Meet - abc-defg-hij - Personal - Microsoft\u{200b} Edge").unwrap();
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
    fn urls_to_rooms() {
        let out = "https://mail.google.com/\nhttps://meet.google.com/abc-defg-hij?authuser=1\n\
https://meet.google.com/landing\nhttps://meet.google.com/abc-defg-hij\n";
        let rooms = meet_rooms_from_urls(out);
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].0, "abc-defg-hij");
        assert!(rooms[0].1.contains("authuser=1"));
    }
}
