//! Google Calendar API: calendar list, paged `events.list`, incremental sync with `syncToken`.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;
use url::Url;

use super::accounts::AccountManager;
use crate::error::{ConnectError, ConnectResult};
use crate::links::{find_video_links, link_from_url};
use crate::types::{AgendaEvent, Attendee, VideoLink, VideoProvider};

/// A full resync is forced this often so the sliding window and any missed changes catch up.
const FULL_RESYNC_AFTER: chrono::Duration = chrono::Duration::hours(6);

/// A calendar the user has selected (or their primary one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarInfo {
    pub id: String,
    pub title: String,
    pub primary: bool,
}

/// Cached state of one calendar between refreshes.
#[derive(Debug, Default, Clone)]
pub struct CalendarSync {
    sync_token: Option<String>,
    events: HashMap<String, AgendaEvent>,
    full_at: Option<DateTime<Utc>>,
}

impl CalendarSync {
    /// Whether an incremental token is held.
    pub fn has_sync_token(&self) -> bool {
        self.sync_token.is_some()
    }
}

/// Change produced by one event resource.
#[derive(Debug, Clone, PartialEq)]
pub enum EventChange {
    Upsert(Box<AgendaEvent>),
    Remove(String),
}

/// Non-error outcomes of an authorised GET other than a body.
enum Fetched {
    Body(Value),
    /// HTTP 410: the sync token is no longer valid.
    Gone,
    /// HTTP 403: e.g. the granted scopes do not cover this endpoint.
    Forbidden,
}

/// Thin Calendar API client; authentication goes through the [`AccountManager`].
#[derive(Debug, Clone)]
pub struct CalendarClient {
    http: reqwest::Client,
    base: String,
}

impl CalendarClient {
    /// `base` is the API root, e.g. `https://www.googleapis.com/calendar/v3`.
    pub fn new(base: impl Into<String>) -> ConnectResult<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("YadaCall/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            base: base.into().trim_end_matches('/').to_string(),
        })
    }

    fn url(&self, segments: &[&str]) -> ConnectResult<Url> {
        let mut url =
            Url::parse(&self.base).map_err(|e| ConnectError::Invalid(format!("base URL: {e}")))?;
        url.path_segments_mut()
            .map_err(|()| ConnectError::Invalid("base URL cannot be a base".into()))?
            .pop_if_empty()
            .extend(segments);
        Ok(url)
    }

    /// Authorised GET with one transparent token refresh on 401.
    async fn get_json(
        &self,
        accounts: &AccountManager,
        account: &str,
        url: Url,
        query: &[(&str, String)],
    ) -> ConnectResult<Fetched> {
        for attempt in 0..2 {
            let token = accounts.access_token(account).await?;
            let response = self
                .http
                .get(url.clone())
                .query(query)
                .bearer_auth(token)
                .send()
                .await?;
            let status = response.status();
            match status.as_u16() {
                200..=299 => return Ok(Fetched::Body(response.json().await?)),
                401 if attempt == 0 => {
                    accounts.invalidate_access_token(account).await;
                }
                401 => {
                    accounts.mark_expired(account);
                    return Err(ConnectError::AuthExpired(account.to_string()));
                }
                410 => return Ok(Fetched::Gone),
                403 => return Ok(Fetched::Forbidden),
                429 | 500..=599 => {
                    return Err(ConnectError::Network(format!(
                        "calendar API: HTTP {status}"
                    )));
                }
                _ => {
                    let body = response.text().await.unwrap_or_default();
                    return Err(ConnectError::Protocol(format!(
                        "calendar API: HTTP {status}: {}",
                        body.chars().take(200).collect::<String>()
                    )));
                }
            }
        }
        Err(ConnectError::AuthExpired(account.to_string()))
    }

    /// Selected and primary calendars of the account.
    pub async fn list_calendars(
        &self,
        accounts: &AccountManager,
        account: &str,
    ) -> ConnectResult<Vec<CalendarInfo>> {
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut query = vec![("minAccessRole", "reader".to_string())];
            if let Some(p) = &page {
                query.push(("pageToken", p.clone()));
            }
            let body = match self
                .get_json(
                    accounts,
                    account,
                    self.url(&["users", "me", "calendarList"])?,
                    &query,
                )
                .await?
            {
                Fetched::Body(body) => body,
                // `events.readonly` alone does not cover calendarList: stay within the minimal
                // scope and read the primary calendar only.
                Fetched::Forbidden => {
                    tracing::info!(
                        "calendarList not permitted for {account}; using the primary calendar"
                    );
                    return Ok(vec![CalendarInfo {
                        id: "primary".into(),
                        title: account.to_string(),
                        primary: true,
                    }]);
                }
                Fetched::Gone => {
                    return Err(ConnectError::Protocol("calendarList: HTTP 410".into()));
                }
            };
            for item in body["items"].as_array().into_iter().flatten() {
                let primary = item["primary"].as_bool().unwrap_or(false);
                let selected = item["selected"].as_bool().unwrap_or(false);
                let deleted = item["deleted"].as_bool().unwrap_or(false);
                let Some(id) = item["id"].as_str() else {
                    continue;
                };
                if (primary || selected) && !deleted {
                    let title = item["summaryOverride"]
                        .as_str()
                        .or(item["summary"].as_str())
                        .unwrap_or(id)
                        .to_string();
                    out.push(CalendarInfo {
                        id: id.to_string(),
                        title,
                        primary,
                    });
                }
            }
            page = body["nextPageToken"].as_str().map(str::to_string);
            if page.is_none() {
                return Ok(out);
            }
        }
    }

    /// Brings `state` up to date for the window, incrementally when possible.
    pub async fn sync_calendar(
        &self,
        accounts: &AccountManager,
        account: &str,
        calendar: &CalendarInfo,
        state: &mut CalendarSync,
        window: (DateTime<Utc>, DateTime<Utc>),
        now: DateTime<Utc>,
    ) -> ConnectResult<()> {
        let stale = state.full_at.is_none_or(|t| now - t > FULL_RESYNC_AFTER);
        if let Some(token) = state.sync_token.clone().filter(|_| !stale) {
            match self
                .list_events(accounts, account, calendar, state, Some(token), window)
                .await?
            {
                SyncOutcome::Done => return Ok(()),
                SyncOutcome::TokenExpired => {
                    tracing::debug!("sync token for {} expired, resyncing", calendar.id);
                }
            }
        }
        state.sync_token = None;
        state.events.clear();
        match self
            .list_events(accounts, account, calendar, state, None, window)
            .await?
        {
            SyncOutcome::Done => {
                state.full_at = Some(now);
                Ok(())
            }
            SyncOutcome::TokenExpired => Err(ConnectError::Protocol(
                "events.list: unexpected HTTP 410 on a full sync".into(),
            )),
        }
    }

    async fn list_events(
        &self,
        accounts: &AccountManager,
        account: &str,
        calendar: &CalendarInfo,
        state: &mut CalendarSync,
        sync_token: Option<String>,
        window: (DateTime<Utc>, DateTime<Utc>),
    ) -> ConnectResult<SyncOutcome> {
        let url = self.url(&["calendars", &calendar.id, "events"])?;
        let mut page: Option<String> = None;
        let mut staged: Vec<EventChange> = Vec::new();
        loop {
            let mut query = vec![
                ("singleEvents", "true".to_string()),
                ("maxResults", "250".to_string()),
            ];
            match &sync_token {
                // Google rejects timeMin/timeMax/orderBy together with syncToken; the token
                // already carries the restrictions of the initial request.
                Some(token) => query.push(("syncToken", token.clone())),
                None => {
                    query.push(("orderBy", "startTime".into()));
                    query.push(("timeMin", rfc3339(window.0)));
                    query.push(("timeMax", rfc3339(window.1)));
                }
            }
            if let Some(p) = &page {
                query.push(("pageToken", p.clone()));
            }
            let body = match self
                .get_json(accounts, account, url.clone(), &query)
                .await?
            {
                Fetched::Body(body) => body,
                Fetched::Gone => return Ok(SyncOutcome::TokenExpired),
                Fetched::Forbidden => {
                    return Err(ConnectError::Protocol(format!(
                        "calendar {}: access forbidden (is the Calendar API enabled and the scope granted?)",
                        calendar.id
                    )));
                }
            };
            for item in body["items"].as_array().into_iter().flatten() {
                if let Some(change) = parse_event(account, calendar, item) {
                    staged.push(change);
                }
            }
            page = body["nextPageToken"].as_str().map(str::to_string);
            if page.is_none() {
                // Commit only after the last page so a failure mid-way leaves old state intact.
                for change in staged {
                    match change {
                        EventChange::Upsert(e) => {
                            state.events.insert(e.id.clone(), *e);
                        }
                        EventChange::Remove(id) => {
                            state.events.remove(&id);
                        }
                    }
                }
                state.sync_token = body["nextSyncToken"]
                    .as_str()
                    .map(str::to_string)
                    .or(sync_token);
                return Ok(SyncOutcome::Done);
            }
        }
    }

    /// Refreshes every selected calendar of `account` and returns its events inside `window`.
    /// A failing calendar keeps its cached events; auth failures abort.
    pub async fn fetch_agenda(
        &self,
        accounts: &AccountManager,
        account: &str,
        states: &mut HashMap<String, CalendarSync>,
        window: (DateTime<Utc>, DateTime<Utc>),
        now: DateTime<Utc>,
    ) -> ConnectResult<Vec<AgendaEvent>> {
        let calendars = self.list_calendars(accounts, account).await?;
        states.retain(|id, _| calendars.iter().any(|c| &c.id == id));
        for calendar in &calendars {
            let state = states.entry(calendar.id.clone()).or_default();
            match self
                .sync_calendar(accounts, account, calendar, state, window, now)
                .await
            {
                Ok(()) => {}
                Err(e @ ConnectError::AuthExpired(_)) => return Err(e),
                Err(e) => tracing::warn!("calendar {} of {account}: {e}", calendar.id),
            }
        }
        let mut out: Vec<AgendaEvent> = states
            .values()
            .flat_map(|s| s.events.values())
            .filter(|e| e.end > window.0 && e.start < window.1)
            .cloned()
            .collect();
        out.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }
}

enum SyncOutcome {
    Done,
    TokenExpired,
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Maps one Google event resource to a change. Cancelled, all-day, declined and non-meeting
/// (out-of-office, focus time, working location) events become removals.
pub fn parse_event(account: &str, calendar: &CalendarInfo, item: &Value) -> Option<EventChange> {
    let source_id = item["id"].as_str()?;
    let id = format!("{account}|{}|{source_id}", calendar.id);
    let skip = || Some(EventChange::Remove(id.clone()));
    if item["status"].as_str() == Some("cancelled") {
        return skip();
    }
    if matches!(
        item["eventType"].as_str(),
        Some("workingLocation" | "outOfOffice" | "focusTime")
    ) {
        return skip();
    }
    let (Some(start), Some(end)) = (
        item["start"]["dateTime"].as_str(),
        item["end"]["dateTime"].as_str(),
    ) else {
        return skip(); // all-day
    };
    let (Ok(start), Ok(end)) = (
        DateTime::parse_from_rfc3339(start),
        DateTime::parse_from_rfc3339(end),
    ) else {
        return skip();
    };
    let attendees: Vec<Attendee> = item["attendees"]
        .as_array()
        .into_iter()
        .flatten()
        .map(attendee_of)
        .collect();
    if attendees
        .iter()
        .any(|a| a.is_self && a.response.as_deref() == Some("declined"))
    {
        return skip();
    }

    let mut meet_url: Option<String> = None;
    let mut other_links: Vec<VideoLink> = Vec::new();
    let mut add = |link: VideoLink| {
        if link.provider == VideoProvider::Meet {
            meet_url.get_or_insert(link.url);
        } else if !other_links.iter().any(|l| l.url == link.url) {
            other_links.push(link);
        }
    };
    for entry in item["conferenceData"]["entryPoints"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if entry["entryPointType"].as_str() == Some("video")
            && let Some(uri) = entry["uri"].as_str()
        {
            add(link_from_url(uri));
        }
    }
    if let Some(hangout) = item["hangoutLink"].as_str() {
        add(link_from_url(hangout));
    }
    for text in [item["location"].as_str(), item["description"].as_str()]
        .into_iter()
        .flatten()
    {
        find_video_links(text).into_iter().for_each(&mut add);
    }

    Some(EventChange::Upsert(Box::new(AgendaEvent {
        id,
        account: account.to_string(),
        calendar: calendar.id.clone(),
        calendar_title: Some(calendar.title.clone()),
        title: item["summary"]
            .as_str()
            .unwrap_or("(без названия)")
            .to_string(),
        start: start.with_timezone(&Utc),
        end: end.with_timezone(&Utc),
        meet_url,
        other_links,
        attendees,
        organizer: item["organizer"]
            .is_object()
            .then(|| attendee_of(&item["organizer"])),
        ical_uid: item["iCalUID"].as_str().map(str::to_string),
        overlaps: Vec::new(),
        also_in: Vec::new(),
    })))
}

fn attendee_of(v: &Value) -> Attendee {
    Attendee {
        name: v["displayName"].as_str().map(str::to_string),
        email: v["email"].as_str().map(str::to_string),
        is_self: v["self"].as_bool().unwrap_or(false),
        response: v["responseStatus"].as_str().map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cal() -> CalendarInfo {
        CalendarInfo {
            id: "primary@x.com".into(),
            title: "Main".into(),
            primary: true,
        }
    }

    fn event(extra: Value) -> Value {
        let mut base = json!({
            "id": "e1", "summary": "Sync",
            "start": {"dateTime": "2026-10-12T10:00:00+03:00"},
            "end": {"dateTime": "2026-10-12T10:30:00+03:00"},
        });
        base.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        base
    }

    fn upsert(v: &Value) -> AgendaEvent {
        match parse_event("a@x.com", &cal(), v).unwrap() {
            EventChange::Upsert(e) => *e,
            EventChange::Remove(id) => panic!("unexpected removal of {id}"),
        }
    }

    #[test]
    fn conference_data_beats_hangout_link_and_times_are_utc() {
        let e = upsert(&event(json!({
            "hangoutLink": "https://meet.google.com/zzz-zzzz-zzz",
            "conferenceData": {"entryPoints": [
                {"entryPointType": "phone", "uri": "tel:+1"},
                {"entryPointType": "video", "uri": "https://meet.google.com/abc-defg-hij"}]},
            "organizer": {"email": "boss@x.com", "displayName": "Boss"},
            "attendees": [{"email": "a@x.com", "self": true, "responseStatus": "accepted"}],
            "iCalUID": "uid1@google.com",
        })));
        assert_eq!(
            e.meet_url.as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );
        assert_eq!(e.start.to_rfc3339(), "2026-10-12T07:00:00+00:00");
        assert_eq!(e.id, "a@x.com|primary@x.com|e1");
        assert_eq!(e.organizer.unwrap().name.as_deref(), Some("Boss"));
        assert!(e.attendees[0].is_self);
        assert_eq!(e.ical_uid.as_deref(), Some("uid1@google.com"));
    }

    #[test]
    fn zoom_addon_goes_to_other_links_and_description_is_scanned() {
        let e = upsert(&event(json!({
            "conferenceData": {"entryPoints": [
                {"entryPointType": "video", "uri": "https://us02web.zoom.us/j/5551234"}]},
            "description": "<p>Backup: <a href=\"https://telemost.yandex.ru/j/9876543210\">link</a></p>",
        })));
        assert_eq!(e.meet_url, None);
        let providers: Vec<_> = e.other_links.iter().map(|l| l.provider).collect();
        assert_eq!(providers, [VideoProvider::Zoom, VideoProvider::Telemost]);
    }

    #[test]
    fn skips_cancelled_all_day_and_declined() {
        let removed = |v: Value| {
            matches!(
                parse_event("a@x.com", &cal(), &v),
                Some(EventChange::Remove(_))
            )
        };
        assert!(removed(event(json!({"status": "cancelled"}))));
        assert!(removed(
            json!({"id": "d", "start": {"date": "2026-10-12"}, "end": {"date": "2026-10-13"}})
        ));
        assert!(removed(event(json!({"attendees": [
            {"email": "a@x.com", "self": true, "responseStatus": "declined"}]}))));
        assert!(removed(event(json!({"eventType": "outOfOffice"}))));
        assert!(!removed(event(json!({"attendees": [
            {"email": "b@x.com", "responseStatus": "declined"}]}))));
    }
}
