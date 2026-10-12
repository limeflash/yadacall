//! End-to-end tests of the Google integration against a local mock of the Google endpoints.

mod common;

use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, TimeZone, Utc};
use common::{Mock, Req, Resp};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use yc_connect::google::{AccountManager, Endpoints, GoogleConfig, MemoryTokenStore, TokenStore};
use yc_connect::{
    AccountPolicy, AccountState, ConnectConfig, ConnectError, ConnectEvent, ConnectService,
    ManualClock,
};

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 12, 9, 0, 0).unwrap()
}

fn id_token(email: &str, hd: Option<&str>) -> String {
    let claims = json!({"email": email, "name": "Test User", "picture": "https://p/x.png", "hd": hd, "sub": "42"});
    format!("h.{}.s", URL_SAFE_NO_PAD.encode(claims.to_string()))
}

fn event(id: &str, minutes_from_now: i64, link: Option<&str>) -> Value {
    let start = t0() + Duration::minutes(minutes_from_now);
    let mut e = json!({
        "id": id, "summary": format!("Meeting {id}"), "status": "confirmed",
        "start": {"dateTime": start.to_rfc3339()},
        "end": {"dateTime": (start + Duration::minutes(30)).to_rfc3339()},
        "iCalUID": format!("{id}@google.com"),
    });
    if let Some(link) = link {
        e["conferenceData"] = json!({"entryPoints": [{"entryPointType": "video", "uri": link}]});
    }
    e
}

struct Harness {
    mock: Mock,
    svc: ConnectService,
    store: Arc<MemoryTokenStore>,
    clock: ManualClock,
    dir: tempfile::TempDir,
    challenge: Arc<Mutex<Option<String>>>,
}

/// Starts a mock where `extra` can answer anything beyond the OAuth endpoints, plus a service
/// pointed at it.
async fn harness<F>(extra: F) -> Harness
where
    F: Fn(&Req) -> Option<Resp> + Send + Sync + 'static,
{
    let challenge: Arc<Mutex<Option<String>>> = Arc::default();
    let seen_challenge = challenge.clone();
    let mock = Mock::start(move |req| {
        if let Some(r) = extra(req) {
            return r;
        }
        match req.path.as_str() {
            "/token" => {
                let form = req.form();
                match form.get("grant_type").map(String::as_str) {
                    Some("authorization_code") => {
                        // PKCE: the verifier must hash to the challenge sent to /auth.
                        let verifier = form.get("code_verifier").cloned().unwrap_or_default();
                        let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
                        let sent = seen_challenge.lock().unwrap().clone();
                        if sent.as_deref() != Some(expected.as_str()) || form["code"] != "the-code" {
                            return Resp::json(400, json!({"error": "invalid_grant"}));
                        }
                        Resp::json(200, json!({
                            "access_token": "at-1", "expires_in": 3600, "refresh_token": "rt-1",
                            "scope": "openid email profile https://www.googleapis.com/auth/calendar.events.readonly",
                            "id_token": id_token("Anna@Corp.com", Some("corp.com")),
                        }))
                    }
                    Some("refresh_token") if form["refresh_token"] == "rt-1" => Resp::json(
                        200,
                        json!({"access_token": "at-2", "expires_in": 3600}),
                    ),
                    _ => Resp::json(400, json!({"error": "invalid_grant", "error_description": "Token has been expired or revoked."})),
                }
            }
            "/revoke" => Resp::json(200, json!({})),
            _ => Resp::json(404, json!({"error": "not found"})),
        }
    })
    .await;

    let dir = tempfile::tempdir().unwrap();
    let mut cfg = ConnectConfig::new(dir.path().join("accounts.json"));
    cfg.client_override = Some(yc_connect::google::ClientCredentials {
        client_id: "test-client".into(),
        client_secret: Some("test-secret".into()),
    });
    cfg.endpoints = Some(Endpoints {
        auth: format!("{}/auth", mock.base),
        token: format!("{}/token", mock.base),
        revoke: format!("{}/revoke", mock.base),
        userinfo: format!("{}/userinfo", mock.base),
        calendar: format!("{}/calendar/v3", mock.base),
    });
    let store = Arc::new(MemoryTokenStore::default());
    let clock = ManualClock::new(t0());
    let svc = ConnectService::new(cfg, store.clone(), Arc::new(clock.clone())).unwrap();
    Harness {
        mock,
        svc,
        store,
        clock,
        dir,
        challenge,
    }
}

impl Harness {
    /// Runs the whole add-account flow with a fake browser: parse and check the consent URL,
    /// then hit the loopback redirect the way Google's redirect would.
    async fn login(&self) -> yc_connect::AccountMeta {
        let challenge = self.challenge.clone();
        let handle = tokio::runtime::Handle::current();
        let auth_base = format!("{}/auth", self.mock.base);
        let open = move |url: &str| {
            let parsed = url::Url::parse(url).unwrap();
            assert!(url.starts_with(&auth_base), "{url}");
            let q: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
            assert_eq!(q["client_id"], "test-client");
            assert_eq!(q["response_type"], "code");
            assert_eq!(q["code_challenge_method"], "S256");
            assert_eq!(q["access_type"], "offline");
            assert_eq!(q["prompt"], "consent");
            assert!(q["redirect_uri"].starts_with("http://127.0.0.1:"));
            assert!(q["scope"].contains("calendar.events.readonly"));
            assert!(!q["scope"].contains("auth/calendar "));
            *challenge.lock().unwrap() = Some(q["code_challenge"].clone());
            let redirect = format!("{}?code=the-code&state={}", q["redirect_uri"], q["state"]);
            handle.spawn(async move {
                let _ = reqwest::get(redirect).await;
            });
        };
        self.svc
            .add_google_account(open, &CancellationToken::new())
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn login_uses_pkce_and_stores_the_refresh_token_in_the_keychain_only() {
    let h = harness(|_| None).await;
    let meta = h.login().await;
    assert_eq!(meta.email, "anna@corp.com");
    assert!(meta.is_workspace());
    assert_eq!(meta.hd.as_deref(), Some("corp.com"));
    assert_eq!(meta.policy, AccountPolicy::Ask);
    assert_eq!(
        h.store.get("google:anna@corp.com").unwrap().as_deref(),
        Some("rt-1")
    );
    let json = std::fs::read_to_string(h.dir.path().join("accounts.json")).unwrap();
    assert!(
        !json.contains("rt-1") && !json.contains("at-1"),
        "secrets leaked: {json}"
    );
    // Token exchange carried the client secret and redirect URI.
    let exchange = h.mock.hits("/token").remove(0).form();
    assert_eq!(exchange["client_secret"], "test-secret");
    assert!(exchange["redirect_uri"].ends_with("/callback"));
}

#[tokio::test]
async fn expired_access_token_is_refreshed_and_invalid_grant_expires_the_account() {
    let revoked = Arc::new(Mutex::new(false));
    let flag = revoked.clone();
    let h = harness(move |req| {
        (req.path == "/token"
            && *flag.lock().unwrap()
            && req.form()["grant_type"] == "refresh_token")
            .then(|| Resp::json(400, json!({"error": "invalid_grant"})))
    })
    .await;
    let meta = h.login().await;
    let mut events = h.svc.subscribe();
    let accounts = h.svc.accounts();

    assert_eq!(accounts.access_token(&meta.id).await.unwrap(), "at-1");
    assert_eq!(h.mock.hits("/token").len(), 1, "cached token reused");
    h.clock
        .advance(Duration::minutes(59) + Duration::seconds(30)); // inside the 60 s skew
    assert_eq!(accounts.access_token(&meta.id).await.unwrap(), "at-2");

    *revoked.lock().unwrap() = true;
    h.clock.advance(Duration::hours(2));
    let err = accounts.access_token(&meta.id).await.unwrap_err();
    assert!(err.needs_reauth(), "{err}");
    assert_eq!(
        accounts.get(&meta.id).unwrap().state,
        AccountState::AuthExpired
    );
    assert_eq!(
        events.try_recv().unwrap(),
        ConnectEvent::AccountAuthExpired {
            account: "anna@corp.com".into()
        }
    );
    // No network round-trip once expired, and the event is not repeated.
    let before = h.mock.hits("/token").len();
    assert!(accounts.access_token(&meta.id).await.is_err());
    assert_eq!(h.mock.hits("/token").len(), before);
    assert!(events.try_recv().is_err());
    // The state survives a restart.
    let persisted = std::fs::read_to_string(h.dir.path().join("accounts.json")).unwrap();
    assert!(persisted.contains("authExpired"));
}

#[tokio::test]
async fn remove_account_revokes_then_deletes_everything() {
    let h = harness(|_| None).await;
    let meta = h.login().await;
    let outcome = h.svc.remove_account(&meta.id).await.unwrap();
    assert!(outcome.revoked);
    assert_eq!(h.mock.hits("/revoke")[0].form()["token"], "rt-1");
    assert_eq!(h.store.get("google:anna@corp.com").unwrap(), None);
    assert!(h.svc.accounts().list().is_empty());
}

#[tokio::test]
async fn not_configured_is_a_clean_error() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, _rx) = tokio::sync::broadcast::channel(4);
    let manager = AccountManager::new(
        dir.path().join("a.json"),
        Arc::new(MemoryTokenStore::default()),
        None,
        Arc::new(ManualClock::new(t0())),
        tx,
    )
    .unwrap();
    assert!(!manager.is_configured());
    let err = manager
        .add_google_account(
            |_| panic!("the browser must not open"),
            &CancellationToken::new(),
            std::time::Duration::from_secs(1),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ConnectError::NotConfigured));
    let core: yc_core::Error = err.into();
    assert!(core.to_string().contains("GOOGLE_OAUTH.md"));
    assert!(GoogleConfig::resolve_from(None, (None, None), (None, None)).is_err());
}

/// Server side of the calendar API with a scripted event list.
#[derive(Default)]
struct CalendarScript {
    expire_sync_token: bool,
    unauthorized_once: bool,
    incremental_items: Vec<Value>,
}

#[tokio::test]
async fn calendar_paging_sync_token_incremental_changes_and_410_resync() {
    let script = Arc::new(Mutex::new(CalendarScript::default()));
    let s = script.clone();
    let h = harness(move |req| {
        let mut script = s.lock().unwrap();
        if req.path.starts_with("/calendar/v3/") && req.bearer() == Some("at-1") && script.unauthorized_once {
            script.unauthorized_once = false;
            return Some(Resp::json(401, json!({"error": {"code": 401}})));
        }
        match req.path.as_str() {
            "/calendar/v3/users/me/calendarList" => Some(Resp::json(200, json!({"items": [
                {"id": "anna@corp.com", "summary": "Anna", "primary": true, "selected": true},
                {"id": "team@group.calendar.google.com", "summary": "Team", "selected": true},
                {"id": "holidays@group.v.calendar.google.com", "summary": "Holidays", "selected": false},
            ]}))),
            "/calendar/v3/calendars/anna@corp.com/events" => {
                if let Some(token) = req.query.get("syncToken") {
                    assert!(!req.query.contains_key("timeMin") && !req.query.contains_key("orderBy"));
                    if script.expire_sync_token {
                        script.expire_sync_token = false;
                        assert_eq!(token, "sync-2");
                        return Some(Resp::json(410, json!({"error": {"code": 410}})));
                    }
                    assert_eq!(token, "sync-1");
                    return Some(Resp::json(200, json!({"items": script.incremental_items, "nextSyncToken": "sync-2"})));
                }
                assert_eq!(req.query["singleEvents"], "true");
                assert_eq!(req.query["orderBy"], "startTime");
                assert_eq!(req.query["timeMin"], "2026-10-11T09:00:00Z");
                assert_eq!(req.query["timeMax"], "2026-10-19T09:00:00Z");
                Some(match req.query.get("pageToken").map(String::as_str) {
                    None => Resp::json(200, json!({
                        "items": [event("e1", 5, Some("https://meet.google.com/abc-defg-hij"))],
                        "nextPageToken": "p2"})),
                    Some("p2") => Resp::json(200, json!({
                        "items": [event("e2", 60, Some("https://us02web.zoom.us/j/123456789")), {
                            "id": "allday", "summary": "Holiday",
                            "start": {"date": "2026-10-13"}, "end": {"date": "2026-10-14"}}],
                        "nextSyncToken": "sync-1"})),
                    other => panic!("unexpected page {other:?}"),
                })
            }
            "/calendar/v3/calendars/team@group.calendar.google.com/events" => Some(Resp::json(200, json!({
                "items": [event("t1", 120, None)], "nextSyncToken": "team-sync"}))),
            _ => None,
        }
    })
    .await;
    h.login().await;
    script.lock().unwrap().unauthorized_once = true; // first calendar call gets a 401

    // Full sync: 401 -> refresh -> retry, two pages, unselected calendar ignored.
    let report = h.svc.refresh().await;
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let ids: Vec<_> = report.agenda.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(ids, ["Meeting e1", "Meeting e2", "Meeting t1"]);
    assert_eq!(
        report.agenda[0].meet_url.as_deref(),
        Some("https://meet.google.com/abc-defg-hij")
    );
    assert_eq!(
        report.agenda[1].other_links[0].url,
        "https://us02web.zoom.us/j/123456789"
    );
    assert!(
        h.mock
            .hits("/calendar/v3/calendars/holidays@group.v.calendar.google.com/events")
            .is_empty()
    );
    assert!(
        h.mock
            .hits("/token")
            .iter()
            .any(|r| r.form()["grant_type"] == "refresh_token")
    );

    // Incremental: cancelled event disappears, a new one appears.
    script.lock().unwrap().incremental_items = vec![
        json!({"id": "e2", "status": "cancelled"}),
        event("e3", 90, Some("https://meet.google.com/zzz-zzzz-zzz")),
    ];
    let report = h.svc.refresh().await;
    let ids: Vec<_> = report.agenda.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(ids, ["Meeting e1", "Meeting e3", "Meeting t1"]);

    // 410 Gone on the stored token: falls back to a full resync with the window.
    script.lock().unwrap().expire_sync_token = true;
    let full_syncs_before = h
        .mock
        .hits("/calendar/v3/calendars/anna@corp.com/events")
        .iter()
        .filter(|r| r.query.contains_key("timeMin"))
        .count();
    let report = h.svc.refresh().await;
    assert!(report.errors.is_empty());
    let ids: Vec<_> = report.agenda.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(
        ids,
        ["Meeting e1", "Meeting e2", "Meeting t1"],
        "state rebuilt from scratch"
    );
    let full_syncs_after = h
        .mock
        .hits("/calendar/v3/calendars/anna@corp.com/events")
        .iter()
        .filter(|r| r.query.contains_key("timeMin"))
        .count();
    assert_eq!(
        full_syncs_after,
        full_syncs_before + 2,
        "two pages re-fetched"
    );
}

#[tokio::test]
async fn reminder_flows_from_a_refreshed_agenda_with_the_fake_clock() {
    let h = harness(|req| match req.path.as_str() {
        "/calendar/v3/users/me/calendarList" => Some(Resp::json(
            200,
            json!({"items": [
            {"id": "anna@corp.com", "summary": "Anna", "primary": true, "selected": true}]}),
        )),
        "/calendar/v3/calendars/anna@corp.com/events" => Some(Resp::json(
            200,
            json!({
            "items": [event("soon", 10, Some("https://meet.google.com/abc-defg-hij"))],
            "nextSyncToken": "s"}),
        )),
        _ => None,
    })
    .await;
    let meta = h.login().await;
    let mut rx = h.svc.subscribe();
    h.svc.refresh().await;
    assert_eq!(rx.try_recv().unwrap(), ConnectEvent::AgendaChanged);

    assert!(h.svc.tick().is_empty());
    h.clock.advance(Duration::minutes(8));
    let due = h.svc.tick();
    let [
        ConnectEvent::ReminderDue {
            event,
            lead_minutes: 2,
        },
    ] = due.as_slice()
    else {
        panic!("{due:?}")
    };
    assert_eq!(
        yc_connect::join_url(event, Some(&meta.email)).unwrap(),
        "https://meet.google.com/abc-defg-hij?authuser=anna%40corp.com"
    );
    assert!(matches!(
        rx.try_recv().unwrap(),
        ConnectEvent::ReminderDue { .. }
    ));

    // Switching the account to Always auto-records at the start instead.
    h.svc.set_policy(&meta.id, AccountPolicy::Always).unwrap();
    h.clock.advance(Duration::minutes(2));
    let due = h.svc.tick();
    assert!(
        matches!(due.as_slice(), [ConnectEvent::AutoRecord { .. }]),
        "{due:?}"
    );
}

#[tokio::test]
async fn ics_source_is_validated_stored_in_keychain_and_merged() {
    let ics = "BEGIN:VCALENDAR\r\nX-WR-CALNAME:Home\r\nBEGIN:VEVENT\r\nUID:i1@x\r\nSUMMARY:Family call\r\n\
DTSTART:20261012T093000Z\r\nDTEND:20261012T100000Z\r\nDESCRIPTION:https://telemost.yandex.ru/j/12345678901234\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let h = harness(move |req| {
        (req.path == "/secret/basic.ics").then(|| Resp::text(200, "text/calendar", ics))
    })
    .await;
    let url = format!("{}/secret/basic.ics", h.mock.base);
    let meta = h.svc.add_ics(&url, "Home").await.unwrap();
    assert_eq!(
        h.store.get(&meta.id).unwrap().as_deref(),
        Some(url.as_str())
    );
    let json = std::fs::read_to_string(h.dir.path().join("accounts.json")).unwrap();
    assert!(
        !json.contains("secret"),
        "ICS URL must not be in the metadata file"
    );

    let report = h.svc.refresh().await;
    assert_eq!(report.agenda.len(), 1);
    assert_eq!(report.agenda[0].title, "Family call");
    assert_eq!(
        report.agenda[0].other_links[0].provider,
        yc_connect::VideoProvider::Telemost
    );

    // Hiding the source drops its events from the agenda.
    h.svc.set_visible(&meta.id, false).unwrap();
    assert!(h.svc.agenda().is_empty());
}

#[tokio::test]
async fn calendar_list_forbidden_falls_back_to_the_primary_calendar() {
    // With only `calendar.events.readonly`, Google answers 403 to calendarList.
    let h = harness(|req| match req.path.as_str() {
        "/calendar/v3/users/me/calendarList" => Some(Resp::json(
            403,
            json!({"error": {"code": 403,
            "message": "Request had insufficient authentication scopes."}}),
        )),
        "/calendar/v3/calendars/primary/events" => Some(Resp::json(
            200,
            json!({
            "items": [event("p1", 20, Some("https://meet.google.com/abc-defg-hij"))],
            "nextSyncToken": "s"}),
        )),
        _ => None,
    })
    .await;
    h.login().await;
    let report = h.svc.refresh().await;
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.agenda.len(), 1);
    assert_eq!(report.agenda[0].calendar, "primary");
}
