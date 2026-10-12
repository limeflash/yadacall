//! In-memory [`Integrations`] fed from the `integrations` section of `demo_data.json` (shared with the browser
//! mock in `ui/src/lib/api/mock.ts`). The agenda is anchored to the moment the app started — the next call is a
//! few minutes away — so reminders, countdowns and the menu-bar title can be tried at any time of day.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Local, NaiveTime, TimeZone, Timelike, Utc};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use yc_core::Error;

use crate::integrations_api::*;

const DATA: &str = include_str!("demo_data.json");

// ---- raw demo data ---------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct Root {
    integrations: Raw,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    accounts: Vec<CalendarAccount>,
    new_account: CalendarAccount,
    events: Vec<RawEvent>,
    tomorrow: Vec<RawEvent>,
    connections: Vec<Connection>,
    want_default: Vec<String>,
    detected_call: RawDetected,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEvent {
    id: String,
    account: String,
    title: String,
    /// Minutes from the app start (today's calls).
    #[serde(default)]
    start_offset_min: i64,
    /// `HH:MM` tomorrow (tomorrow's calls).
    #[serde(default)]
    time: Option<String>,
    duration_min: i64,
    #[serde(default)]
    attendees: Vec<String>,
    #[serde(default)]
    meet_url: Option<String>,
    #[serde(default)]
    meeting_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDetected {
    platform: String,
    url: String,
    after_sec: u64,
}

// ---- state --------------------------------------------------------------------------------------------

struct Event {
    id: String,
    account: String,
    title: String,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    attendees: Vec<String>,
    meet_url: Option<String>,
    meeting_id: Option<String>,
}

struct State {
    accounts: Vec<CalendarAccount>,
    events: Vec<Event>,
    /// Per-event "record / don't record" chosen by the user; wins over the account policy.
    chosen: HashMap<String, bool>,
    want: HashSet<String>,
    settings: ReminderSettings,
    detected: Option<DetectedCall>,
    adding: Option<CancellationToken>,
}

/// Demo [`Integrations`]: everything is clickable, nothing leaves the machine.
pub struct DemoIntegrations {
    raw: Raw,
    state: Mutex<State>,
    sink: IntegrationSink,
    /// Length of one fake browser step; the real OAuth round-trip is as slow as the user.
    step: Duration,
}

fn lock(s: &Mutex<State>) -> MutexGuard<'_, State> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

fn not_found(what: &str, id: &str) -> Error {
    Error::NotFound(format!("{what} {id}"))
}

impl DemoIntegrations {
    pub fn new(sink: IntegrationSink) -> Arc<Self> {
        Self::at(sink, Utc::now(), Duration::from_millis(1500))
    }

    /// `now` anchors the agenda; tests pass a fixed instant.
    fn at(sink: IntegrationSink, now: DateTime<Utc>, step: Duration) -> Arc<Self> {
        // The data file is compiled in: a parse error is a build-time bug caught by the tests below.
        let root: Root = serde_json::from_str(DATA).expect("demo_data.json: integrations section");
        let raw = root.integrations;
        let t0 = now
            .with_second(0)
            .and_then(|t| t.with_nanosecond(0))
            .unwrap_or(now);
        let tomorrow = (Local::now() + ChronoDuration::days(1)).date_naive();
        let events = raw
            .events
            .iter()
            .map(|e| (e, t0 + ChronoDuration::minutes(e.start_offset_min)))
            .chain(raw.tomorrow.iter().filter_map(|e| {
                let time = NaiveTime::parse_from_str(e.time.as_deref()?, "%H:%M").ok()?;
                let start = Local
                    .from_local_datetime(&tomorrow.and_time(time))
                    .earliest()?
                    .with_timezone(&Utc);
                Some((e, start))
            }))
            .map(|(e, start)| Event {
                id: e.id.clone(),
                account: e.account.clone(),
                title: e.title.clone(),
                start,
                end: start + ChronoDuration::minutes(e.duration_min),
                attendees: e.attendees.clone(),
                meet_url: e.meet_url.clone(),
                meeting_id: e.meeting_id.clone(),
            })
            .collect();
        let state = State {
            accounts: raw.accounts.clone(),
            events,
            chosen: HashMap::new(),
            want: raw.want_default.iter().cloned().collect(),
            settings: ReminderSettings::default(),
            detected: None,
            adding: None,
        };
        Arc::new(Self {
            raw,
            state: Mutex::new(state),
            sink,
            step,
        })
    }

    /// Background simulation: a call "noticed" shortly after start, one expired sign-in notice.
    pub async fn run(self: Arc<Self>) {
        let after = self.raw.detected_call.after_sec;
        tokio::time::sleep(Duration::from_secs(25.min(after))).await;
        let expired = lock(&self.state)
            .accounts
            .iter()
            .find(|a| a.status == AccountStatus::Expired)
            .cloned();
        if let Some(a) = expired {
            (self.sink)(IntegrationEvent::AuthExpired {
                account_id: a.id,
                email: a.email,
            });
        }
        tokio::time::sleep(Duration::from_secs(after.saturating_sub(25))).await;
        let call = {
            let mut st = lock(&self.state);
            if !st.settings.detect_unscheduled || st.detected.is_some() {
                return;
            }
            let call = DetectedCall {
                id: "call-detected-1".into(),
                platform: self.raw.detected_call.platform.clone(),
                url: self.raw.detected_call.url.clone(),
                detected_at: Utc::now(),
            };
            st.detected = Some(call.clone());
            call
        };
        (self.sink)(IntegrationEvent::CallDetected { call });
    }

    fn changed(&self) {
        (self.sink)(IntegrationEvent::AgendaChanged);
    }
}

/// Events of visible, working accounts, shaped as DTOs. Overlap = another account's event at the same time.
fn visible_events(st: &State) -> Vec<AgendaEvent> {
    let working = |id: &str| {
        st.accounts
            .iter()
            .find(|a| a.id == id)
            .filter(|a| a.status == AccountStatus::Ok && a.visible)
    };
    let mut live: Vec<(&Event, &CalendarAccount)> = st
        .events
        .iter()
        .filter_map(|e| working(&e.account).map(|a| (e, a)))
        .collect();
    live.sort_by_key(|(e, _)| e.start);
    live.iter()
        .map(|(e, a)| AgendaEvent {
            id: e.id.clone(),
            account_id: a.id.clone(),
            title: e.title.clone(),
            start: e.start,
            end: e.end,
            meet_url: e.meet_url.clone(),
            attendees: e.attendees.clone(),
            record_policy: a.record_policy,
            will_record: st
                .chosen
                .get(&e.id)
                .copied()
                .unwrap_or(a.record_policy == RecordPolicy::Always),
            overlaps: live
                .iter()
                .filter(|(o, oa)| {
                    o.id != e.id && oa.id != a.id && o.start < e.end && e.start < o.end
                })
                .map(|(o, _)| o.id.clone())
                .collect(),
            meeting_id: e.meeting_id.clone(),
        })
        .collect()
}

fn outcome(st: &State, event_id: &str, record: bool, open_url: bool) -> Result<JoinOutcome> {
    let e = st
        .events
        .iter()
        .find(|e| e.id == event_id)
        .ok_or_else(|| not_found("event", event_id))?;
    let a = st
        .accounts
        .iter()
        .find(|a| a.id == e.account)
        .ok_or_else(|| not_found("account", &e.account))?;
    Ok(JoinOutcome {
        title: e.title.clone(),
        // The demo has no real Meet rooms: never open a browser.
        meet_url: if open_url { e.meet_url.clone() } else { None },
        account_email: a.email.clone(),
        record,
        ends_at: e.end,
    })
}

#[async_trait]
impl Integrations for DemoIntegrations {
    async fn accounts(&self) -> Result<Vec<CalendarAccount>> {
        Ok(lock(&self.state).accounts.clone())
    }

    async fn add_google_account(&self) -> Result<CalendarAccount> {
        let token = CancellationToken::new();
        lock(&self.state).adding = Some(token.clone());
        for step in 1..=3u8 {
            (self.sink)(IntegrationEvent::AddProgress { step });
            tokio::select! {
                () = token.cancelled() => return Err(Error::Cancelled),
                () = tokio::time::sleep(self.step) => {}
            }
        }
        let mut st = lock(&self.state);
        st.adding = None;
        let new = self.raw.new_account.clone();
        let acc = match st.accounts.iter().find(|a| a.id == new.id) {
            Some(a) => a.clone(),
            None => {
                st.accounts.push(new.clone());
                new
            }
        };
        drop(st);
        self.changed();
        Ok(acc)
    }

    async fn cancel_add_account(&self) -> Result<()> {
        if let Some(t) = lock(&self.state).adding.take() {
            t.cancel();
        }
        Ok(())
    }

    async fn remove_account(&self, id: &str) -> Result<()> {
        let mut st = lock(&self.state);
        let before = st.accounts.len();
        st.accounts.retain(|a| a.id != id);
        if st.accounts.len() == before {
            return Err(not_found("account", id));
        }
        drop(st);
        self.changed();
        Ok(())
    }

    async fn reauth_account(&self, id: &str) -> Result<CalendarAccount> {
        {
            let mut st = lock(&self.state);
            let a = st
                .accounts
                .iter_mut()
                .find(|a| a.id == id)
                .ok_or_else(|| not_found("account", id))?;
            a.status = AccountStatus::Connecting;
        }
        self.changed();
        tokio::time::sleep(self.step * 8 / 5).await;
        let acc = {
            let mut st = lock(&self.state);
            let a = st
                .accounts
                .iter_mut()
                .find(|a| a.id == id)
                .ok_or_else(|| not_found("account", id))?;
            a.status = AccountStatus::Ok;
            a.clone()
        };
        self.changed();
        Ok(acc)
    }

    async fn set_account_policy(&self, id: &str, policy: RecordPolicy) -> Result<()> {
        let mut st = lock(&self.state);
        let a = st
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or_else(|| not_found("account", id))?;
        a.record_policy = policy;
        // A policy change resets the per-event choices of that account.
        let ids: Vec<String> = st
            .events
            .iter()
            .filter(|e| e.account == id)
            .map(|e| e.id.clone())
            .collect();
        for i in ids {
            st.chosen.remove(&i);
        }
        Ok(())
    }

    async fn set_account_visible(&self, id: &str, visible: bool) -> Result<()> {
        let mut st = lock(&self.state);
        let a = st
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or_else(|| not_found("account", id))?;
        a.visible = visible;
        Ok(())
    }

    async fn agenda(&self, range: TimeRange) -> Result<Vec<AgendaEvent>> {
        Ok(visible_events(&lock(&self.state))
            .into_iter()
            .filter(|e| e.start < range.to && e.end > range.from)
            .collect())
    }

    async fn set_event_recording(&self, event_id: &str, record: bool) -> Result<()> {
        let mut st = lock(&self.state);
        if !st.events.iter().any(|e| e.id == event_id) {
            return Err(not_found("event", event_id));
        }
        st.chosen.insert(event_id.to_string(), record);
        Ok(())
    }

    async fn join_and_record(&self, event_id: &str) -> Result<JoinOutcome> {
        let mut st = lock(&self.state);
        let out = outcome(&st, event_id, true, false)?;
        st.chosen.insert(event_id.to_string(), true);
        Ok(out)
    }

    async fn join_without_recording(&self, event_id: &str) -> Result<JoinOutcome> {
        let mut st = lock(&self.state);
        let out = outcome(&st, event_id, false, false)?;
        st.chosen.insert(event_id.to_string(), false);
        Ok(out)
    }

    async fn connections(&self) -> Result<Vec<Connection>> {
        let st = lock(&self.state);
        let working = st
            .accounts
            .iter()
            .filter(|a| a.status == AccountStatus::Ok)
            .count() as u32;
        Ok(self
            .raw
            .connections
            .iter()
            .map(|c| Connection {
                accounts: (c.state == ConnectionState::Connected).then_some(working),
                want: st.want.contains(&c.id),
                ..c.clone()
            })
            .collect())
    }

    async fn toggle_want(&self, service: &str) -> Result<bool> {
        let soon = self
            .raw
            .connections
            .iter()
            .any(|c| c.id == service && c.state == ConnectionState::Soon);
        if !soon {
            return Err(Error::Invalid(format!(
                "{service} is not a coming-soon service"
            )));
        }
        let mut st = lock(&self.state);
        if st.want.remove(service) {
            Ok(false)
        } else {
            st.want.insert(service.to_string());
            Ok(true)
        }
    }

    async fn reminder_settings(&self) -> Result<ReminderSettings> {
        Ok(lock(&self.state).settings.clone())
    }

    async fn set_reminder_settings(&self, s: ReminderSettings) -> Result<ReminderSettings> {
        if ![1, 2, 5].contains(&s.remind_minutes) {
            return Err(Error::Invalid("remind_minutes must be 1, 2 or 5".into()));
        }
        lock(&self.state).settings = s.clone();
        Ok(s)
    }

    async fn detected_call(&self) -> Result<Option<DetectedCall>> {
        Ok(lock(&self.state).detected.clone())
    }

    async fn resolve_detected_call(&self, id: &str, record: bool) -> Result<Option<JoinOutcome>> {
        let mut st = lock(&self.state);
        let Some(call) = st.detected.take_if(|c| c.id == id) else {
            return Ok(None);
        };
        Ok(record.then(|| JoinOutcome {
            title: format!(
                "{} · {}",
                call.platform,
                call.url.rsplit('/').next().unwrap_or("")
            ),
            meet_url: None,
            account_email: String::new(),
            record: true,
            ends_at: Utc::now() + ChronoDuration::hours(1),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> Arc<DemoIntegrations> {
        DemoIntegrations::at(Arc::new(|_| {}), Utc::now(), Duration::from_millis(20))
    }
    fn today() -> TimeRange {
        let now = Utc::now();
        TimeRange {
            from: now - ChronoDuration::hours(6),
            to: now + ChronoDuration::hours(3),
        }
    }

    #[test]
    fn demo_data_parses() {
        let d = demo();
        assert_eq!(d.raw.accounts.len(), 3);
        assert_eq!(d.raw.connections.len(), 6);
    }

    #[tokio::test]
    async fn agenda_hides_expired_and_hidden_accounts() {
        let d = demo();
        assert!(
            d.agenda(today())
                .await
                .unwrap()
                .iter()
                .all(|e| e.account_id != "free")
        );
        d.set_account_visible("home", false).await.unwrap();
        let ev = d.agenda(today()).await.unwrap();
        assert!(ev.iter().all(|e| e.account_id == "work"));
        assert!(ev.iter().all(|e| e.overlaps.is_empty()));
    }

    #[tokio::test]
    async fn overlap_is_between_accounts_only() {
        let d = demo();
        let ev = d.agenda(today()).await.unwrap();
        let client = ev.iter().find(|e| e.id == "ev-client").unwrap();
        assert_eq!(client.overlaps, vec!["ev-realtor".to_string()]);
        let anya = ev.iter().find(|e| e.id == "ev-anya").unwrap();
        assert!(anya.overlaps.is_empty());
    }

    #[tokio::test]
    async fn will_record_follows_policy_and_choice() {
        let d = demo();
        let get =
            |ev: &[AgendaEvent], id: &str| ev.iter().find(|e| e.id == id).unwrap().will_record;
        let ev = d.agenda(today()).await.unwrap();
        assert!(get(&ev, "ev-anya"), "work account records always");
        assert!(!get(&ev, "ev-acc"), "personal account asks, default off");
        d.set_event_recording("ev-acc", true).await.unwrap();
        d.set_event_recording("ev-anya", false).await.unwrap();
        let ev = d.agenda(today()).await.unwrap();
        assert!(get(&ev, "ev-acc") && !get(&ev, "ev-anya"));
        d.set_account_policy("work", RecordPolicy::Never)
            .await
            .unwrap();
        let ev = d.agenda(today()).await.unwrap();
        assert!(!get(&ev, "ev-client"));
    }

    #[tokio::test]
    async fn want_flag_is_local_and_only_for_soon_services() {
        let d = demo();
        assert!(d.toggle_want("zoom").await.unwrap());
        assert!(!d.toggle_want("zoom").await.unwrap());
        assert!(d.toggle_want("meet").await.is_err());
        let c = d.connections().await.unwrap();
        assert!(c.iter().find(|c| c.id == "discord").unwrap().want);
        assert_eq!(c[0].accounts, Some(2));
    }

    #[tokio::test]
    async fn add_account_can_be_cancelled() {
        let d = demo();
        let d2 = d.clone();
        let h = tokio::spawn(async move { d2.add_google_account().await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        d.cancel_add_account().await.unwrap();
        assert!(matches!(h.await.unwrap(), Err(Error::Cancelled)));
        assert_eq!(d.accounts().await.unwrap().len(), 3);
        let acc = d.add_google_account().await.unwrap();
        assert_eq!(acc.id, "studio");
        assert_eq!(d.accounts().await.unwrap().len(), 4);
    }

    #[tokio::test]
    async fn reauth_turns_expired_into_ok() {
        let d = demo();
        let acc = d.reauth_account("free").await.unwrap();
        assert_eq!(acc.status, AccountStatus::Ok);
    }

    #[tokio::test]
    async fn detected_call_resolves_once() {
        let d = demo();
        lock(&d.state).detected = Some(DetectedCall {
            id: "c".into(),
            platform: "Google Meet".into(),
            url: "meet.google.com/kqv-tdre-xmn".into(),
            detected_at: Utc::now(),
        });
        let out = d.resolve_detected_call("c", true).await.unwrap().unwrap();
        assert!(out.record && out.title.ends_with("kqv-tdre-xmn"));
        assert!(d.detected_call().await.unwrap().is_none());
        assert!(d.resolve_detected_call("c", true).await.unwrap().is_none());
    }
}
