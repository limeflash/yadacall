//! Test helpers.

use chrono::{TimeZone, Utc};

use crate::types::AgendaEvent;

/// Event on 2026-10-12 starting `start_min` minutes after 09:00 UTC, with a Meet link whose room
/// is the event id.
pub fn ev(id: &str, account: &str, start_min: i64, len_min: i64) -> AgendaEvent {
    let base = Utc.with_ymd_and_hms(2026, 10, 12, 9, 0, 0).unwrap();
    AgendaEvent {
        id: id.into(),
        account: account.into(),
        calendar: "c".into(),
        calendar_title: None,
        title: id.into(),
        start: base + chrono::Duration::minutes(start_min),
        end: base + chrono::Duration::minutes(start_min + len_min),
        meet_url: Some(format!("https://meet.google.com/{id}")),
        other_links: vec![],
        attendees: vec![],
        organizer: None,
        ical_uid: None,
        overlaps: vec![],
        also_in: vec![],
    }
}
