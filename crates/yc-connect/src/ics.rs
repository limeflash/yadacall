//! Zero-OAuth fallback: subscribe to a calendar's secret ICS URL (Google, Outlook, Yandex,
//! iCloud). Parses VEVENTs, expands RRULE/EXDATE/RDATE for the agenda window, applies
//! RECURRENCE-ID overrides and extracts call links.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use icalendar::parser::{Component, Property, read_calendar, unfold};

use crate::error::{ConnectError, ConnectResult};
use crate::links::{find_video_links, link_from_url};
use crate::types::{AgendaEvent, Attendee, VideoLink, VideoProvider};

/// Upper bound on instances generated per recurring series.
const MAX_INSTANCES: u16 = 500;
/// Largest feed we are willing to download.
const MAX_FEED_BYTES: usize = 25 * 1024 * 1024;
/// Length assumed for events with neither DTEND nor DURATION.
const DEFAULT_LEN: chrono::Duration = chrono::Duration::minutes(30);

/// Downloads ICS feeds.
#[derive(Debug, Clone)]
pub struct IcsClient {
    http: reqwest::Client,
}

impl IcsClient {
    /// Creates a client with sane timeouts.
    pub fn new() -> ConnectResult<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent(concat!("YadaCall/", env!("CARGO_PKG_VERSION")))
                .build()?,
        })
    }

    /// Downloads the feed text.
    pub async fn download(&self, url: &str) -> ConnectResult<String> {
        let response = self.http.get(url).send().await?;
        let status = response.status();
        if !status.is_success() {
            return Err(ConnectError::Network(format!("ICS feed: HTTP {status}")));
        }
        if response
            .content_length()
            .is_some_and(|n| n as usize > MAX_FEED_BYTES)
        {
            return Err(ConnectError::Protocol("ICS feed is too large".into()));
        }
        let bytes = response.bytes().await?;
        if bytes.len() > MAX_FEED_BYTES {
            return Err(ConnectError::Protocol("ICS feed is too large".into()));
        }
        String::from_utf8(bytes.to_vec())
            .map_err(|_| ConnectError::Protocol("ICS feed is not UTF-8".into()))
    }

    /// Downloads and parses the feed into agenda events inside `window`.
    pub async fn fetch_agenda(
        &self,
        url: &str,
        account: &str,
        window: (DateTime<Utc>, DateTime<Utc>),
    ) -> ConnectResult<Vec<AgendaEvent>> {
        let text = self.download(url).await?;
        parse_ics(&text, account, window)
    }
}

/// Parses ICS text into events overlapping `window`, sorted by start.
pub fn parse_ics(
    text: &str,
    account: &str,
    window: (DateTime<Utc>, DateTime<Utc>),
) -> ConnectResult<Vec<AgendaEvent>> {
    let unfolded = unfold(text);
    let calendar = read_calendar(&unfolded)
        .map_err(|e| ConnectError::Protocol(format!("not an iCalendar feed: {e}")))?;
    let default_tz = calendar
        .properties
        .iter()
        .find(|p| p.name.as_str().eq_ignore_ascii_case("X-WR-TIMEZONE"))
        .and_then(|p| resolve_tz(p.val.as_str()));
    let title = calendar
        .properties
        .iter()
        .find(|p| p.name.as_str().eq_ignore_ascii_case("X-WR-CALNAME"))
        .map(|p| unescape(p.val.as_str()));

    let mut masters: Vec<&Component> = Vec::new();
    let mut overrides: HashMap<(String, i64), &Component> = HashMap::new();
    for c in calendar
        .components
        .iter()
        .filter(|c| c.name.as_str().eq_ignore_ascii_case("VEVENT"))
    {
        match (prop(c, "RECURRENCE-ID"), prop(c, "UID")) {
            (Some(rid), Some(uid)) => {
                if let Some(Dt::Instant(t)) = parse_dt(rid, default_tz) {
                    overrides.insert((uid.val.as_str().to_string(), t.timestamp()), c);
                }
            }
            _ => masters.push(c),
        }
    }

    let ctx = Ctx {
        account,
        title: title.as_deref(),
        default_tz,
    };
    let mut events = Vec::new();
    let mut used_overrides: Vec<(String, i64)> = Vec::new();
    for master in masters {
        let uid = prop(master, "UID").map(|p| p.val.as_str().to_string());
        let Some(base) = BaseEvent::read(master, default_tz) else {
            continue;
        };
        for start in base.instances(master, default_tz, window) {
            let key = uid.clone().map(|u| (u, start.timestamp()));
            if let Some(over) = key.as_ref().and_then(|k| overrides.get(k)) {
                used_overrides.extend(key);
                if let Some(e) = ctx.event_from(over, uid.as_deref()) {
                    events.push(e);
                }
                continue;
            }
            if base.cancelled {
                continue;
            }
            if let Some(e) = ctx.build(master, uid.as_deref(), start, base.len) {
                events.push(e);
            }
        }
    }
    // Overrides whose master is absent from the feed stand on their own.
    for (key, over) in &overrides {
        if !used_overrides.contains(key)
            && let Some(e) = ctx.event_from(over, Some(&key.0))
        {
            events.push(e);
        }
    }
    events.retain(|e| e.end > window.0 && e.start < window.1);
    events.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.id.cmp(&b.id)));
    events.dedup_by(|a, b| a.id == b.id);
    Ok(events)
}

struct Ctx<'a> {
    account: &'a str,
    title: Option<&'a str>,
    default_tz: Option<Tz>,
}

impl Ctx<'_> {
    /// Builds the event for a stand-alone VEVENT (an override or a non-recurring one).
    fn event_from(&self, c: &Component, uid: Option<&str>) -> Option<AgendaEvent> {
        let base = BaseEvent::read(c, self.default_tz)?;
        if base.cancelled {
            return None;
        }
        self.build(c, uid, base.start, base.len)
    }

    fn build(
        &self,
        c: &Component,
        uid: Option<&str>,
        start: DateTime<Utc>,
        len: chrono::Duration,
    ) -> Option<AgendaEvent> {
        let uid_text = uid.map(str::to_string).unwrap_or_else(|| {
            format!(
                "{}@{}",
                text_of(c, "SUMMARY").unwrap_or_default(),
                start.timestamp()
            )
        });
        let description = text_of(c, "DESCRIPTION").unwrap_or_default();
        let location = text_of(c, "LOCATION").unwrap_or_default();

        let mut meet_url: Option<String> = None;
        let mut other_links: Vec<VideoLink> = Vec::new();
        let mut add = |link: VideoLink| {
            if link.provider == VideoProvider::Meet {
                meet_url.get_or_insert(link.url);
            } else if !other_links.iter().any(|l| l.url == link.url) {
                other_links.push(link);
            }
        };
        for name in ["X-GOOGLE-CONFERENCE", "CONFERENCE", "URL"] {
            for p in c
                .properties
                .iter()
                .filter(|p| p.name.as_str().eq_ignore_ascii_case(name))
            {
                let value = p.val.as_str().trim();
                let link = link_from_url(value);
                if link.provider != VideoProvider::Other {
                    add(link);
                }
            }
        }
        for text in [&location, &description] {
            find_video_links(text).into_iter().for_each(&mut add);
        }

        let attendees = c
            .properties
            .iter()
            .filter(|p| p.name.as_str().eq_ignore_ascii_case("ATTENDEE"))
            .map(attendee_of)
            .collect();
        let organizer = prop(c, "ORGANIZER").map(attendee_of);

        Some(AgendaEvent {
            id: format!("{}|ics|{}@{}", self.account, uid_text, start.timestamp()),
            account: self.account.to_string(),
            calendar: "ics".into(),
            calendar_title: self.title.map(str::to_string),
            title: text_of(c, "SUMMARY").unwrap_or_else(|| "(без названия)".into()),
            start,
            end: start + len,
            meet_url,
            other_links,
            attendees,
            organizer,
            ical_uid: uid.map(str::to_string),
            overlaps: Vec::new(),
            also_in: Vec::new(),
        })
    }
}

/// Start, length and recurrence facts of a VEVENT.
struct BaseEvent {
    start: DateTime<Utc>,
    len: chrono::Duration,
    cancelled: bool,
    recurring: bool,
}

impl BaseEvent {
    fn read(c: &Component, default_tz: Option<Tz>) -> Option<Self> {
        let Dt::Instant(start) = parse_dt(prop(c, "DTSTART")?, default_tz)? else {
            return None; // all-day
        };
        let len = match prop(c, "DTEND").and_then(|p| parse_dt(p, default_tz)) {
            Some(Dt::Instant(end)) if end > start => end - start,
            Some(Dt::Instant(_)) => chrono::Duration::zero(),
            Some(Dt::Date) => return None,
            None => prop(c, "DURATION")
                .and_then(|p| parse_duration(p.val.as_str()))
                .unwrap_or(DEFAULT_LEN),
        };
        let cancelled = prop(c, "STATUS")
            .is_some_and(|p| p.val.as_str().trim().eq_ignore_ascii_case("CANCELLED"));
        let recurring = c.properties.iter().any(|p| {
            let n = p.name.as_str();
            n.eq_ignore_ascii_case("RRULE") || n.eq_ignore_ascii_case("RDATE")
        });
        Some(Self {
            start,
            len,
            cancelled,
            recurring,
        })
    }

    /// Start instants of the occurrences that may overlap the window.
    fn instances(
        &self,
        c: &Component,
        default_tz: Option<Tz>,
        window: (DateTime<Utc>, DateTime<Utc>),
    ) -> Vec<DateTime<Utc>> {
        if !self.recurring {
            return vec![self.start];
        }
        match expand(c, default_tz, self.len, window) {
            Ok(starts) => starts,
            Err(e) => {
                tracing::warn!("cannot expand recurrence, using first instance only: {e}");
                vec![self.start]
            }
        }
    }
}

/// Expands DTSTART/RRULE/RDATE/EXDATE with the `rrule` crate.
fn expand(
    c: &Component,
    default_tz: Option<Tz>,
    len: chrono::Duration,
    window: (DateTime<Utc>, DateTime<Utc>),
) -> Result<Vec<DateTime<Utc>>, String> {
    let mut lines = Vec::new();
    for p in &c.properties {
        let name = p.name.as_str().to_ascii_uppercase();
        if !matches!(name.as_str(), "DTSTART" | "RRULE" | "RDATE" | "EXDATE") {
            continue;
        }
        let mut line = name.clone();
        let mut has_tzid = false;
        for param in &p.params {
            let key = param.key.as_str().to_ascii_uppercase();
            let val = param
                .val
                .as_ref()
                .map(|v| v.as_str().trim_matches('"').to_string());
            if key == "TZID" {
                let tz = val.as_deref().and_then(resolve_tz).or(default_tz);
                if let Some(tz) = tz {
                    line.push_str(&format!(";TZID={}", tz.name()));
                    has_tzid = true;
                }
            } else if let Some(val) = val {
                line.push_str(&format!(";{key}={val}"));
            }
        }
        let value = p.val.as_str().trim();
        let floating = !value.ends_with('Z') && !value.ends_with('z') && !value.contains('Z');
        if matches!(name.as_str(), "DTSTART" | "RDATE" | "EXDATE")
            && floating
            && !has_tzid
            && !line.contains("VALUE=DATE")
        {
            let tz = default_tz.unwrap_or(Tz::UTC);
            line.push_str(&format!(";TZID={}", tz.name()));
        }
        line.push(':');
        line.push_str(value);
        lines.push(line);
    }
    let set: rrule::RRuleSet = lines
        .join("\n")
        .parse()
        .map_err(|e: rrule::RRuleError| e.to_string())?;
    let after = (window.0 - len).with_timezone(&rrule::Tz::UTC);
    let before = window.1.with_timezone(&rrule::Tz::UTC);
    let result = set.after(after).before(before).all(MAX_INSTANCES);
    Ok(result
        .dates
        .into_iter()
        .map(|d| d.with_timezone(&Utc))
        .collect())
}

/// A parsed DTSTART/DTEND/RECURRENCE-ID value.
enum Dt {
    Instant(DateTime<Utc>),
    Date,
}

fn parse_dt(p: &Property, default_tz: Option<Tz>) -> Option<Dt> {
    let value = p.val.as_str().trim();
    let param = |name: &str| {
        p.params
            .iter()
            .find(|q| q.key.as_str().eq_ignore_ascii_case(name))
            .and_then(|q| q.val.as_ref())
            .map(|v| v.as_str().trim_matches('"').to_string())
    };
    if param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE")) || value.len() == 8 {
        return NaiveDate::parse_from_str(value, "%Y%m%d")
            .ok()
            .map(|_| Dt::Date);
    }
    if let Some(utc) = value.strip_suffix(['Z', 'z']) {
        let naive = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        return Some(Dt::Instant(Utc.from_utc_datetime(&naive)));
    }
    let naive = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").ok()?;
    let tz = match param("TZID") {
        Some(name) => resolve_tz(&name).or_else(|| {
            tracing::warn!("unknown time zone `{name}`, using the feed default");
            default_tz
        }),
        None => default_tz,
    }
    .unwrap_or(Tz::UTC);
    // `earliest` picks the first of two ambiguous local times; a gap falls through to UTC.
    let local = tz.from_local_datetime(&naive).earliest();
    Some(Dt::Instant(
        local
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|| Utc.from_utc_datetime(&naive)),
    ))
}

/// IANA name or one of the common Windows (Outlook) names.
pub fn resolve_tz(name: &str) -> Option<Tz> {
    let name = name.trim().trim_matches('"');
    if let Ok(tz) = name.parse::<Tz>() {
        return Some(tz);
    }
    let iana = match name {
        "UTC" | "Coordinated Universal Time" => "UTC",
        "W. Europe Standard Time" => "Europe/Berlin",
        "Central Europe Standard Time" => "Europe/Budapest",
        "Central European Standard Time" => "Europe/Warsaw",
        "Romance Standard Time" => "Europe/Paris",
        "GMT Standard Time" => "Europe/London",
        "Russian Standard Time" => "Europe/Moscow",
        "E. Europe Standard Time" => "Europe/Chisinau",
        "FLE Standard Time" => "Europe/Kyiv",
        "GTB Standard Time" => "Europe/Bucharest",
        "Belarus Standard Time" => "Europe/Minsk",
        "Turkey Standard Time" => "Europe/Istanbul",
        "Israel Standard Time" => "Asia/Jerusalem",
        "Georgian Standard Time" => "Asia/Tbilisi",
        "Armenian Standard Time" => "Asia/Yerevan",
        "Azerbaijan Standard Time" => "Asia/Baku",
        "Arabian Standard Time" => "Asia/Dubai",
        "Arab Standard Time" => "Asia/Riyadh",
        "West Asia Standard Time" => "Asia/Tashkent",
        "Central Asia Standard Time" => "Asia/Almaty",
        "Ekaterinburg Standard Time" => "Asia/Yekaterinburg",
        "N. Central Asia Standard Time" => "Asia/Novosibirsk",
        "North Asia Standard Time" => "Asia/Krasnoyarsk",
        "India Standard Time" => "Asia/Kolkata",
        "SE Asia Standard Time" => "Asia/Bangkok",
        "Singapore Standard Time" => "Asia/Singapore",
        "China Standard Time" => "Asia/Shanghai",
        "Tokyo Standard Time" => "Asia/Tokyo",
        "Korea Standard Time" => "Asia/Seoul",
        "AUS Eastern Standard Time" => "Australia/Sydney",
        "Eastern Standard Time" => "America/New_York",
        "Central Standard Time" => "America/Chicago",
        "Mountain Standard Time" => "America/Denver",
        "Pacific Standard Time" => "America/Los_Angeles",
        _ => return None,
    };
    iana.parse().ok()
}

/// `PT1H30M`, `P1D`, `P1W`, `-PT15M` (sign ignored; a negative length makes no sense here).
pub fn parse_duration(s: &str) -> Option<chrono::Duration> {
    let s = s.trim().trim_start_matches(['+', '-']).strip_prefix('P')?;
    let mut total = chrono::Duration::zero();
    let mut number = String::new();
    let mut in_time = false;
    for ch in s.chars() {
        match ch {
            'T' => in_time = true,
            '0'..='9' => number.push(ch),
            unit => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                total += match (unit, in_time) {
                    ('W', false) => chrono::Duration::weeks(n),
                    ('D', false) => chrono::Duration::days(n),
                    ('H', true) => chrono::Duration::hours(n),
                    ('M', true) => chrono::Duration::minutes(n),
                    ('S', true) => chrono::Duration::seconds(n),
                    _ => return None,
                };
            }
        }
    }
    number.is_empty().then_some(total)
}

fn prop<'a, 'b>(c: &'b Component<'a>, name: &str) -> Option<&'b Property<'a>> {
    c.properties
        .iter()
        .find(|p| p.name.as_str().eq_ignore_ascii_case(name))
}

fn text_of(c: &Component, name: &str) -> Option<String> {
    prop(c, name).map(|p| unescape(p.val.as_str()))
}

/// Undoes RFC 5545 TEXT escaping.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

fn attendee_of(p: &Property) -> Attendee {
    let param = |name: &str| {
        p.params
            .iter()
            .find(|q| q.key.as_str().eq_ignore_ascii_case(name))
            .and_then(|q| q.val.as_ref())
            .map(|v| v.as_str().trim_matches('"').to_string())
    };
    let value = p.val.as_str().trim();
    let email = value
        .get(..7)
        .filter(|prefix| prefix.eq_ignore_ascii_case("mailto:"))
        .map(|_| value[7..].to_string());
    let response = param("PARTSTAT").map(|s| match s.to_ascii_uppercase().as_str() {
        "ACCEPTED" => "accepted".to_string(),
        "DECLINED" => "declined".to_string(),
        "TENTATIVE" => "tentative".to_string(),
        _ => "needsAction".to_string(),
    });
    Attendee {
        name: param("CN"),
        email,
        is_self: false,
        response,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(y: i32, m: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap()
    }

    fn window() -> (DateTime<Utc>, DateTime<Utc>) {
        (utc(2026, 10, 11, 0, 0), utc(2026, 10, 19, 0, 0))
    }

    const WEEKLY: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nX-WR-CALNAME:Work\r\n\
BEGIN:VEVENT\r\nUID:weekly@x\r\nSUMMARY:Planning\r\n\
DTSTART;TZID=Europe/Moscow:20260601T100000\r\nDTEND;TZID=Europe/Moscow:20260601T104500\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO,WE\r\nEXDATE;TZID=Europe/Moscow:20261014T100000\r\n\
X-GOOGLE-CONFERENCE:https://meet.google.com/abc-defg-hij\r\n\
DESCRIPTION:Notes\\nZoom backup https://zoom.us/j/777888999\r\n\
ATTENDEE;CN=Анна;PARTSTAT=ACCEPTED:mailto:anna@x.com\r\nORGANIZER;CN=Boss:mailto:boss@x.com\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:weekly@x\r\nRECURRENCE-ID;TZID=Europe/Moscow:20261012T100000\r\n\
SUMMARY:Planning (moved)\r\nDTSTART;TZID=Europe/Moscow:20261012T150000\r\nDTEND;TZID=Europe/Moscow:20261012T153000\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:allday@x\r\nSUMMARY:Holiday\r\nDTSTART;VALUE=DATE:20261013\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:cancel@x\r\nSUMMARY:Dropped\r\nSTATUS:CANCELLED\r\nDTSTART:20261015T080000Z\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:once@x\r\nSUMMARY:One-off\r\nDTSTART:20261016T120000Z\r\nDURATION:PT1H30M\r\nLOCATION:https://teams.microsoft.com/l/meetup-join/19%3ameeting\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn rrule_exdate_override_and_timezones() {
        let events = parse_ics(WEEKLY, "ics:1", window()).unwrap();
        let summary: Vec<_> = events
            .iter()
            .map(|e| (e.title.as_str(), e.start.to_rfc3339()))
            .collect();
        // Mon 12 moved to 15:00 MSK (12:00Z); Wed 14 is EXDATE'd; Mon 19 is outside the window.
        assert_eq!(
            summary,
            [
                ("Planning (moved)", "2026-10-12T12:00:00+00:00".to_string()),
                ("One-off", "2026-10-16T12:00:00+00:00".to_string()),
            ]
            .into_iter()
            .collect::<Vec<_>>()
        );
        assert_eq!(
            events[0].end - events[0].start,
            chrono::Duration::minutes(30)
        );
        assert_eq!(
            events[1].end - events[1].start,
            chrono::Duration::minutes(90)
        );
    }

    #[test]
    fn regular_occurrences_links_and_people() {
        let (from, to) = (utc(2026, 10, 19, 0, 0), utc(2026, 10, 22, 0, 0));
        let events = parse_ics(WEEKLY, "ics:1", (from, to)).unwrap();
        // Mon 19th and Wed 21st at 10:00 MSK = 07:00Z.
        let starts: Vec<_> = events.iter().map(|e| e.start).collect();
        assert_eq!(starts, [utc(2026, 10, 19, 7, 0), utc(2026, 10, 21, 7, 0)]);
        let e = &events[0];
        assert_eq!(e.title, "Planning");
        assert_eq!(e.end - e.start, chrono::Duration::minutes(45));
        assert_eq!(
            e.meet_url.as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );
        assert_eq!(e.other_links[0].provider, VideoProvider::Zoom);
        assert_eq!(e.calendar_title.as_deref(), Some("Work"));
        assert_eq!(e.attendees[0].email.as_deref(), Some("anna@x.com"));
        assert_eq!(e.attendees[0].response.as_deref(), Some("accepted"));
        assert_eq!(e.organizer.as_ref().unwrap().name.as_deref(), Some("Boss"));
        assert_eq!(e.ical_uid.as_deref(), Some("weekly@x"));
    }

    #[test]
    fn skips_all_day_and_cancelled_and_reads_teams_location() {
        let events = parse_ics(WEEKLY, "ics:1", window()).unwrap();
        assert!(
            events
                .iter()
                .all(|e| e.title != "Holiday" && e.title != "Dropped")
        );
        let once = events.iter().find(|e| e.title == "One-off").unwrap();
        assert_eq!(once.other_links[0].provider, VideoProvider::Teams);
    }

    #[test]
    fn dst_changes_are_respected() {
        // 10:00 Berlin: CEST (UTC+2) before 25 Oct 2026, CET (UTC+1) after.
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:d@x\r\nSUMMARY:Daily\r\n\
DTSTART;TZID=W. Europe Standard Time:20261022T100000\r\nDTEND;TZID=W. Europe Standard Time:20261022T101500\r\n\
RRULE:FREQ=DAILY;COUNT=6\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let events =
            parse_ics(ics, "a", (utc(2026, 10, 22, 0, 0), utc(2026, 10, 29, 0, 0))).unwrap();
        let hours: Vec<_> = events
            .iter()
            .map(|e| e.start.format("%d %H:%M").to_string())
            .collect();
        assert_eq!(
            hours,
            [
                "22 08:00", "23 08:00", "24 08:00", "25 09:00", "26 09:00", "27 09:00"
            ]
        );
    }

    #[test]
    fn floating_uses_x_wr_timezone_and_garbage_is_rejected() {
        let ics = "BEGIN:VCALENDAR\r\nX-WR-TIMEZONE:Asia/Tbilisi\r\nBEGIN:VEVENT\r\nUID:f@x\r\n\
SUMMARY:Float\r\nDTSTART:20261014T100000\r\nDTEND:20261014T110000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let events = parse_ics(ics, "a", window()).unwrap();
        assert_eq!(events[0].start, utc(2026, 10, 14, 6, 0));
        assert!(parse_ics("<html>nope</html>", "a", window()).is_err());
    }

    #[test]
    fn durations() {
        assert_eq!(
            parse_duration("PT1H30M"),
            Some(chrono::Duration::minutes(90))
        );
        assert_eq!(parse_duration("P1DT2H"), Some(chrono::Duration::hours(26)));
        assert_eq!(parse_duration("P1W"), Some(chrono::Duration::weeks(1)));
        assert_eq!(parse_duration("garbage"), None);
    }
}
