//! Pure, clock-injected scheduling: which reminder / auto-record events are due *now* for the
//! merged agenda, given per-account policies and per-event overrides. No I/O, no timers; the
//! caller feeds the current instant to [`Scheduler::poll`].

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};

use crate::links::meet_code_of;
use crate::types::{
    AccountPolicy, AgendaEvent, ConnectEvent, DetectedApp, DetectedCall, VideoProvider,
};

/// Reminder tuning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReminderSettings {
    /// Minutes before the start to remind (1, 2 or 5 in the UI; default 2).
    pub lead_minutes: u32,
    /// Also remind about events that have no video-call link (in-person meetings).
    pub include_without_link: bool,
    /// How long after the start a not-yet-handled event still triggers "starting" / auto-record
    /// (covers opening the app late).
    pub late_grace: Duration,
}

impl Default for ReminderSettings {
    fn default() -> Self {
        Self {
            lead_minutes: 2,
            include_without_link: false,
            late_grace: Duration::minutes(10),
        }
    }
}

/// Per-event decision that beats the account policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventOverride {
    /// Record this meeting automatically.
    Record,
    /// Ignore this meeting entirely.
    DontRecord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Reminder,
    Starting,
    Auto,
}

type FiredKey = (String, i64, Kind);

/// Decides when to emit [`ConnectEvent`]s.
#[derive(Debug, Default)]
pub struct Scheduler {
    settings: ReminderSettings,
    policies: HashMap<String, AccountPolicy>,
    overrides: HashMap<String, EventOverride>,
    agenda: Vec<AgendaEvent>,
    fired: HashSet<FiredKey>,
    /// Event currently being recorded and until when (blocks a second auto-record).
    recording: Option<(String, DateTime<Utc>)>,
    detected_seen: HashSet<(DetectedApp, Option<String>)>,
}

impl Scheduler {
    /// New scheduler with the given settings.
    pub fn new(settings: ReminderSettings) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }

    /// Replaces the reminder settings.
    pub fn set_settings(&mut self, settings: ReminderSettings) {
        self.settings = settings;
    }

    /// Current settings.
    pub fn settings(&self) -> &ReminderSettings {
        &self.settings
    }

    /// Sets the policy of one account (default is `Ask`).
    pub fn set_policy(&mut self, account: &str, policy: AccountPolicy) {
        self.policies.insert(account.to_string(), policy);
    }

    /// Sets or clears (`None`) the override of one event.
    pub fn set_override(&mut self, event_id: &str, decision: Option<EventOverride>) {
        match decision {
            Some(d) => self.overrides.insert(event_id.to_string(), d),
            None => self.overrides.remove(event_id),
        };
    }

    /// The override of an event, if any.
    pub fn override_of(&self, event_id: &str) -> Option<EventOverride> {
        self.overrides.get(event_id).copied()
    }

    /// Tells the scheduler a recording of `event_id` is running until `until` (e.g. the user
    /// pressed "Join & record"), so overlapping `Always` events do not start a second one.
    pub fn note_recording(&mut self, event_id: &str, until: DateTime<Utc>) {
        self.recording = Some((event_id.to_string(), until));
    }

    /// Installs a fresh merged agenda. Returns `AgendaChanged` when it differs from the old one.
    pub fn set_agenda(&mut self, agenda: Vec<AgendaEvent>) -> Option<ConnectEvent> {
        if agenda == self.agenda {
            return None;
        }
        self.agenda = agenda;
        Some(ConnectEvent::AgendaChanged)
    }

    /// The agenda currently held.
    pub fn agenda(&self) -> &[AgendaEvent] {
        &self.agenda
    }

    fn action(&self, event: &AgendaEvent) -> AccountPolicy {
        match self.overrides.get(&event.id) {
            Some(EventOverride::Record) => AccountPolicy::Always,
            Some(EventOverride::DontRecord) => AccountPolicy::Never,
            None => self
                .policies
                .get(&event.account)
                .copied()
                .unwrap_or_default(),
        }
    }

    fn was_fired(&self, event: &AgendaEvent, kind: Kind) -> bool {
        self.fired
            .contains(&(event.id.clone(), event.start.timestamp(), kind))
    }

    fn mark(&mut self, event: &AgendaEvent, kind: Kind) {
        self.fired
            .insert((event.id.clone(), event.start.timestamp(), kind));
    }

    /// Events due at `now`, each emitted at most once per (event, start time).
    pub fn poll(&mut self, now: DateTime<Utc>) -> Vec<ConnectEvent> {
        self.fired
            .retain(|(_, start, _)| *start > (now - Duration::days(2)).timestamp());
        if self
            .recording
            .as_ref()
            .is_some_and(|(_, until)| *until <= now)
        {
            self.recording = None;
        }
        let lead = Duration::minutes(i64::from(self.settings.lead_minutes));
        let mut out = Vec::new();
        let agenda = self.agenda.clone();
        for event in &agenda {
            if (!event.has_call_link() && !self.settings.include_without_link) || now >= event.end {
                continue;
            }
            let action = self.action(event);
            let start_window_end = event.end.min(event.start + self.settings.late_grace);
            let starting = now >= event.start && now < start_window_end;
            match action {
                AccountPolicy::Never => {}
                AccountPolicy::Ask => {
                    if now >= event.start - lead
                        && now < event.start
                        && !self.was_fired(event, Kind::Reminder)
                    {
                        self.mark(event, Kind::Reminder);
                        out.push(ConnectEvent::ReminderDue {
                            event: event.clone(),
                            lead_minutes: self.settings.lead_minutes,
                        });
                    }
                    if starting && !self.was_fired(event, Kind::Starting) {
                        self.mark(event, Kind::Reminder);
                        self.mark(event, Kind::Starting);
                        out.push(ConnectEvent::CallStarting {
                            event: event.clone(),
                        });
                    }
                }
                AccountPolicy::Always => {
                    if starting && !self.was_fired(event, Kind::Auto) {
                        self.mark(event, Kind::Auto);
                        let busy = self
                            .recording
                            .as_ref()
                            .is_some_and(|(id, until)| *id != event.id && now < *until);
                        if busy {
                            // One recording at a time: let the user decide on the overlap.
                            out.push(ConnectEvent::CallStarting {
                                event: event.clone(),
                            });
                        } else {
                            self.recording = Some((event.id.clone(), event.end));
                            out.push(ConnectEvent::AutoRecord {
                                event: event.clone(),
                            });
                        }
                    }
                }
            }
        }
        out
    }

    /// Feeds a detection result. Returns `CallDetected` unless the call is covered by a calendar
    /// event that already triggered a reminder/auto-record (or is set to never record), or the
    /// same call was already announced.
    pub fn on_detected(
        &mut self,
        mut call: DetectedCall,
        now: DateTime<Utc>,
    ) -> Option<ConnectEvent> {
        let lead = Duration::minutes(i64::from(self.settings.lead_minutes));
        let matched = self.agenda.iter().find(|e| {
            let active = now >= e.start - lead.max(Duration::minutes(15)) && now < e.end;
            active && matches_call(e, &call)
        });
        if let Some(event) = matched {
            let event = event.clone();
            call.matched_event_id = Some(event.id.clone());
            let handled = [Kind::Reminder, Kind::Starting, Kind::Auto]
                .into_iter()
                .any(|k| self.was_fired(&event, k));
            if handled || self.action(&event) == AccountPolicy::Never {
                return None;
            }
        }
        let key = (call.app.clone(), call.meet_code.clone());
        if !self.detected_seen.insert(key) {
            return None;
        }
        Some(ConnectEvent::CallDetected { call })
    }

    /// The detector saw no call: forget what was announced so a later call is announced again.
    pub fn on_detection_cleared(&mut self) {
        self.detected_seen.clear();
    }
}

fn matches_call(event: &AgendaEvent, call: &DetectedCall) -> bool {
    match (&call.app, &call.meet_code) {
        (DetectedApp::GoogleMeet, Some(code)) => event
            .meet_url
            .as_deref()
            .and_then(meet_code_of)
            .is_some_and(|c| &c == code),
        (DetectedApp::GoogleMeet, None) => event.meet_url.is_some(),
        (DetectedApp::Zoom, _) => has_provider(event, VideoProvider::Zoom),
        (DetectedApp::Teams, _) => has_provider(event, VideoProvider::Teams),
        _ => false,
    }
}

fn has_provider(event: &AgendaEvent, provider: VideoProvider) -> bool {
    event.other_links.iter().any(|l| l.provider == provider)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::clock::{Clock, ManualClock};
    use crate::testutil::ev;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 12, 9, 0, 0).unwrap()
    }

    fn sched(events: Vec<AgendaEvent>) -> Scheduler {
        let mut s = Scheduler::new(ReminderSettings::default());
        s.set_agenda(events);
        s
    }

    fn kinds(events: &[ConnectEvent]) -> Vec<&'static str> {
        events
            .iter()
            .map(|e| match e {
                ConnectEvent::ReminderDue { .. } => "reminder",
                ConnectEvent::CallStarting { .. } => "starting",
                ConnectEvent::AutoRecord { .. } => "auto",
                ConnectEvent::AccountAuthExpired { .. } => "expired",
                ConnectEvent::AgendaChanged => "changed",
                ConnectEvent::CallDetected { .. } => "detected",
            })
            .collect()
    }

    #[test]
    fn ask_emits_reminder_at_lead_then_starting_once() {
        let clock = ManualClock::new(t0() + Duration::minutes(50));
        let mut s = sched(vec![ev("a", "x", 60, 30)]);
        assert!(s.poll(clock.now()).is_empty()); // 10 min before
        clock.advance(Duration::minutes(7)); // 3 min before: not yet
        assert!(s.poll(clock.now()).is_empty());
        clock.advance(Duration::minutes(1)); // exactly 2 min before
        let due = s.poll(clock.now());
        assert!(matches!(
            due.as_slice(),
            [ConnectEvent::ReminderDue {
                lead_minutes: 2,
                ..
            }]
        ));
        assert!(s.poll(clock.now()).is_empty(), "no duplicate reminder");
        clock.advance(Duration::minutes(2));
        assert_eq!(kinds(&s.poll(clock.now())), ["starting"]);
        assert!(s.poll(clock.now()).is_empty());
    }

    #[test]
    fn lead_time_is_configurable() {
        let mut s = sched(vec![ev("a", "x", 60, 30)]);
        s.set_settings(ReminderSettings {
            lead_minutes: 5,
            ..ReminderSettings::default()
        });
        assert!(s.poll(t0() + Duration::minutes(54)).is_empty());
        let due = s.poll(t0() + Duration::minutes(55));
        assert!(matches!(
            due.as_slice(),
            [ConnectEvent::ReminderDue {
                lead_minutes: 5,
                ..
            }]
        ));
    }

    #[test]
    fn opening_the_app_late_still_offers_the_call_but_not_after_the_grace() {
        let mut s = sched(vec![ev("a", "x", 0, 60)]);
        assert_eq!(kinds(&s.poll(t0() + Duration::minutes(4))), ["starting"]);
        let mut s = sched(vec![ev("a", "x", 0, 60)]);
        assert!(s.poll(t0() + Duration::minutes(11)).is_empty());
    }

    #[test]
    fn always_auto_records_at_start_and_never_is_silent() {
        let mut s = sched(vec![ev("a", "work", 10, 30), ev("b", "spam", 10, 30)]);
        s.set_policy("work", AccountPolicy::Always);
        s.set_policy("spam", AccountPolicy::Never);
        assert!(
            s.poll(t0() + Duration::minutes(8)).is_empty(),
            "no reminder for Always"
        );
        let out = s.poll(t0() + Duration::minutes(10));
        assert!(matches!(out.as_slice(), [ConnectEvent::AutoRecord { event }] if event.id == "a"));
        assert!(s.poll(t0() + Duration::minutes(11)).is_empty());
    }

    #[test]
    fn overrides_beat_policies() {
        let mut s = sched(vec![ev("a", "x", 10, 30), ev("b", "y", 10, 30)]);
        s.set_policy("x", AccountPolicy::Always);
        s.set_override("a", Some(EventOverride::DontRecord));
        s.set_override("b", Some(EventOverride::Record)); // y is Ask by default
        let out = s.poll(t0() + Duration::minutes(10));
        assert!(matches!(out.as_slice(), [ConnectEvent::AutoRecord { event }] if event.id == "b"));
        s.set_override("a", None);
        assert_eq!(s.override_of("a"), None);
    }

    #[test]
    fn overlapping_always_events_record_only_the_first() {
        let mut s = sched(vec![ev("a", "x", 0, 60), ev("b", "x", 30, 60)]);
        s.set_policy("x", AccountPolicy::Always);
        assert_eq!(kinds(&s.poll(t0())), ["auto"]);
        // b starts while a is still being recorded: downgraded to a prompt.
        assert_eq!(kinds(&s.poll(t0() + Duration::minutes(30))), ["starting"]);
        assert!(s.poll(t0() + Duration::minutes(31)).is_empty());
    }

    #[test]
    fn user_recording_blocks_auto_record_of_overlap() {
        let mut s = sched(vec![ev("b", "x", 0, 30)]);
        s.set_policy("x", AccountPolicy::Always);
        s.note_recording("other", t0() + Duration::minutes(45));
        assert_eq!(kinds(&s.poll(t0())), ["starting"]);
    }

    #[test]
    fn events_without_links_are_skipped_unless_enabled() {
        let mut e = ev("a", "x", 0, 30);
        e.meet_url = None;
        let mut s = sched(vec![e]);
        assert!(s.poll(t0()).is_empty());
        s.set_settings(ReminderSettings {
            include_without_link: true,
            ..ReminderSettings::default()
        });
        assert_eq!(kinds(&s.poll(t0())), ["starting"]);
    }

    #[test]
    fn moved_event_fires_again_and_agenda_change_is_reported() {
        let mut s = sched(vec![ev("a", "x", 0, 30)]);
        assert_eq!(kinds(&s.poll(t0())), ["starting"]);
        assert!(
            s.set_agenda(vec![ev("a", "x", 0, 30)]).is_none(),
            "same agenda"
        );
        let changed = s.set_agenda(vec![ev("a", "x", 120, 30)]);
        assert_eq!(changed, Some(ConnectEvent::AgendaChanged));
        assert_eq!(kinds(&s.poll(t0() + Duration::minutes(120))), ["starting"]);
    }

    fn meet_call(code: &str) -> DetectedCall {
        DetectedCall {
            app: DetectedApp::GoogleMeet,
            meet_code: Some(code.into()),
            code: Some(code.into()),
            url: None,
            browser: None,
            title: "Google Meet".into(),
            automation_permission_required: None,
            matched_event_id: None,
            certain: true,
        }
    }

    #[test]
    fn detection_is_suppressed_for_calendar_calls_that_already_had_a_reminder() {
        let mut s = sched(vec![ev("abc-defg-hij", "x", 0, 30)]);
        // Not yet reminded: announced, with the matching event attached.
        let first = s.on_detected(meet_call("abc-defg-hij"), t0() - Duration::minutes(5));
        assert!(matches!(
            first,
            Some(ConnectEvent::CallDetected { call }) if call.matched_event_id.as_deref() == Some("abc-defg-hij")
        ));
        // After the call-starting prompt it is suppressed.
        s.on_detection_cleared();
        s.poll(t0());
        assert!(s.on_detected(meet_call("abc-defg-hij"), t0()).is_none());
    }

    #[test]
    fn unknown_calls_are_announced_once_until_cleared() {
        let mut s = sched(vec![ev("abc-defg-hij", "x", 0, 30)]);
        let now = t0() + Duration::hours(3);
        assert!(s.on_detected(meet_call("zzz-zzzz-zzz"), now).is_some());
        assert!(s.on_detected(meet_call("zzz-zzzz-zzz"), now).is_none());
        s.on_detection_cleared();
        assert!(s.on_detected(meet_call("zzz-zzzz-zzz"), now).is_some());
    }

    #[test]
    fn never_policy_events_do_not_trigger_detection() {
        let mut s = sched(vec![ev("abc-defg-hij", "x", 0, 30)]);
        s.set_policy("x", AccountPolicy::Never);
        assert!(s.on_detected(meet_call("abc-defg-hij"), t0()).is_none());
    }
}
