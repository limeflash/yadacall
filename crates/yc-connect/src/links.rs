//! Finding video-call links in free text (location, description) and classifying them.

use std::sync::LazyLock;

use regex::Regex;
use url::Url;

use crate::types::{VideoLink, VideoProvider};

static URL_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Stops at whitespace, quotes and HTML delimiters; trailing punctuation is trimmed below.
    Regex::new(r#"https?://[^\s"'<>]+"#).expect("static regex")
});

/// Classifies a URL by host, returning `None` for non-call links.
pub fn classify(url: &str) -> Option<VideoProvider> {
    let parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let path = parsed.path();
    let host_is = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    if host == "meet.google.com" {
        // Only real rooms; `/landing` and bare `/` are not calls.
        let first = path.trim_matches('/');
        let is_room = crate::links::is_meet_code(first) || first.starts_with("lookup/");
        return is_room.then_some(VideoProvider::Meet);
    }
    if host_is("zoom.us") || host_is("zoomgov.com") {
        return (path.starts_with("/j/")
            || path.starts_with("/my/")
            || path.starts_with("/wc/")
            || path.starts_with("/s/"))
        .then_some(VideoProvider::Zoom);
    }
    if host == "teams.microsoft.com" || host == "teams.live.com" || host == "teams.microsoft.us" {
        return (path.contains("meetup-join") || path.starts_with("/meet/"))
            .then_some(VideoProvider::Teams);
    }
    if host == "telemost.yandex.ru" || host == "telemost.360.yandex.ru" {
        return (path.trim_matches('/').len() > 3).then_some(VideoProvider::Telemost);
    }
    if host == "meet.jit.si" {
        return (!path.trim_matches('/').is_empty()).then_some(VideoProvider::Jitsi);
    }
    None
}

/// `abc-defg-hij`
pub fn is_meet_code(s: &str) -> bool {
    static CODE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[a-z]{3}-[a-z]{4}-[a-z]{3}$").expect("static regex"));
    CODE.is_match(s)
}

/// Extracts the Meet room code from a Meet URL.
pub fn meet_code_of(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    if parsed.host_str()? != "meet.google.com" {
        return None;
    }
    let first = parsed.path().trim_matches('/').split('/').next()?;
    is_meet_code(first).then(|| first.to_string())
}

/// Finds all call links in `text` (plain text or HTML), de-duplicated, in order of appearance.
pub fn find_video_links(text: &str) -> Vec<VideoLink> {
    let decoded = text.replace("&amp;", "&");
    let mut found: Vec<VideoLink> = Vec::new();
    for m in URL_RE.find_iter(&decoded) {
        let raw = m
            .as_str()
            .trim_end_matches(['.', ',', ';', ')', ']', '>', '\\']);
        let Some(provider) = classify(raw) else {
            continue;
        };
        let url = normalize(raw, provider);
        if !found.iter().any(|l| l.url == url) {
            found.push(VideoLink { provider, url });
        }
    }
    found
}

/// Drops tracking noise from Meet URLs so the same room compares equal.
fn normalize(url: &str, provider: VideoProvider) -> String {
    if provider != VideoProvider::Meet {
        return url.to_string();
    }
    match meet_code_of(url) {
        Some(code) => format!("https://meet.google.com/{code}"),
        None => url.to_string(),
    }
}

/// Classifies one explicit URL (conference data), keeping `Other` for unknown hosts.
pub fn link_from_url(url: &str) -> VideoLink {
    let provider = classify(url).unwrap_or(VideoProvider::Other);
    VideoLink {
        provider,
        url: normalize(url, provider),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_links_in_html_and_text() {
        let text = r#"Join: <a href="https://meet.google.com/abc-defg-hij?authuser=0&amp;hs=122">Meet</a>
            or https://us02web.zoom.us/j/123456789?pwd=abc. Teams: https://teams.microsoft.com/l/meetup-join/19%3a.. 
            https://telemost.yandex.ru/j/12345678901234 and https://example.com/x"#;
        let links = find_video_links(text);
        let kinds: Vec<_> = links.iter().map(|l| l.provider).collect();
        assert_eq!(
            kinds,
            [
                VideoProvider::Meet,
                VideoProvider::Zoom,
                VideoProvider::Teams,
                VideoProvider::Telemost
            ]
        );
        assert_eq!(links[0].url, "https://meet.google.com/abc-defg-hij");
        assert!(
            links[1]
                .url
                .starts_with("https://us02web.zoom.us/j/123456789")
        );
    }

    #[test]
    fn ignores_non_room_meet_urls() {
        assert!(find_video_links("https://meet.google.com/landing").is_empty());
        assert!(find_video_links("https://zoom.us/pricing").is_empty());
    }

    #[test]
    fn meet_code_parsing() {
        assert_eq!(
            meet_code_of("https://meet.google.com/abc-defg-hij?authuser=2"),
            Some("abc-defg-hij".into())
        );
        assert_eq!(meet_code_of("https://meet.google.com/xyz"), None);
        assert_eq!(meet_code_of("https://example.com/abc-defg-hij"), None);
    }
}
