//! Merging per-account events into one agenda, marking overlaps, building join URLs.

use std::collections::HashMap;

use url::Url;

use crate::types::AgendaEvent;

/// Merges events from all sources: sorted by start, the same meeting seen through several
/// accounts collapsed into one (the first keeps `also_in`), overlaps marked on both sides.
pub fn merge_agenda(sources: Vec<Vec<AgendaEvent>>) -> Vec<AgendaEvent> {
    let mut all: Vec<AgendaEvent> = sources.into_iter().flatten().collect();
    all.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.id.cmp(&b.id)));

    let mut merged: Vec<AgendaEvent> = Vec::with_capacity(all.len());
    let mut seen: HashMap<(String, i64), usize> = HashMap::new();
    for event in all {
        let key = event
            .ical_uid
            .clone()
            .or_else(|| event.meet_url.clone())
            .map(|k| (k, event.start.timestamp()));
        if let Some(index) = key.as_ref().and_then(|k| seen.get(k)).copied() {
            let first = &mut merged[index];
            if first.account != event.account && !first.also_in.contains(&event.account) {
                first.also_in.push(event.account);
            }
            continue;
        }
        if let Some(key) = key {
            seen.insert(key, merged.len());
        }
        merged.push(event);
    }

    for e in &mut merged {
        e.overlaps.clear();
    }
    for i in 0..merged.len() {
        for j in (i + 1)..merged.len() {
            if merged[j].start >= merged[i].end {
                break; // sorted by start: nothing later can overlap `i`
            }
            let (a, b) = (merged[i].id.clone(), merged[j].id.clone());
            merged[i].overlaps.push(b);
            merged[j].overlaps.push(a);
        }
    }
    merged
}

/// The URL that opens the call. Google Meet links get `authuser=<email>` so the call opens in
/// the right Google account; other providers' links are returned unchanged.
pub fn join_url(event: &AgendaEvent, account_email: Option<&str>) -> Option<String> {
    if let Some(meet) = &event.meet_url {
        let Some(email) = account_email.filter(|e| e.contains('@')) else {
            return Some(meet.clone());
        };
        return Some(match Url::parse(meet) {
            Ok(mut url) => {
                let kept: Vec<(String, String)> = url
                    .query_pairs()
                    .filter(|(k, _)| k != "authuser")
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
                url.query_pairs_mut()
                    .clear()
                    .extend_pairs(kept)
                    .append_pair("authuser", email);
                url.to_string()
            }
            Err(_) => meet.clone(),
        });
    }
    event.other_links.first().map(|l| l.url.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::ev;
    use crate::types::{VideoLink, VideoProvider};

    #[test]
    fn marks_overlaps_but_not_back_to_back() {
        let merged = merge_agenda(vec![vec![
            ev("a", "x", 0, 60),
            ev("b", "y", 30, 60),
            ev("c", "x", 90, 30),
        ]]);
        assert_eq!(merged[0].overlaps, ["b"]);
        assert_eq!(merged[1].overlaps, ["a"]);
        assert!(merged[2].overlaps.is_empty());
    }

    #[test]
    fn same_meeting_through_two_accounts_collapses() {
        let mut a = ev("a", "work", 0, 30);
        let mut b = ev("b", "home", 0, 30);
        a.ical_uid = Some("u1".into());
        b.ical_uid = Some("u1".into());
        let merged = merge_agenda(vec![vec![a], vec![b]]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].also_in, ["home"]);
    }

    #[test]
    fn join_url_adds_authuser() {
        let mut e = ev("abc-defg-hij", "a@x.com", 0, 30);
        e.meet_url = Some("https://meet.google.com/abc-defg-hij?authuser=0&hs=1".into());
        let url = join_url(&e, Some("a@x.com")).unwrap();
        assert_eq!(
            url,
            "https://meet.google.com/abc-defg-hij?hs=1&authuser=a%40x.com"
        );
        assert_eq!(
            join_url(&e, None).unwrap(),
            "https://meet.google.com/abc-defg-hij?authuser=0&hs=1"
        );
        e.meet_url = None;
        e.other_links = vec![VideoLink {
            provider: VideoProvider::Zoom,
            url: "https://zoom.us/j/1".into(),
        }];
        assert_eq!(
            join_url(&e, Some("a@x.com")).unwrap(),
            "https://zoom.us/j/1"
        );
    }
}
