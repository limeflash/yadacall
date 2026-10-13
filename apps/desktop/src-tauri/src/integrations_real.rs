//! The real [`Integrations`]: Google accounts, agenda, reminders and call detection from `yc-connect`.
//!
//! [`ConnectService`] owns the truth (accounts, merged agenda, the `Scheduler` that decides when a reminder or an
//! automatic recording is due, the call detector). This module translates it into the DTOs of the UI, keeps the
//! few things the service does not (reminder settings, "Хочу такое", which meeting recorded which event) in
//! `integrations.json` next to the accounts file, and turns [`ConnectEvent`]s into [`IntegrationEvent`]s.
//!
//! Refresh tokens never pass through here: they live in the OS keychain behind `yc-connect`'s token store.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Local, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;
use yc_connect as yc;
use yc_connect::detect::system_detector;
use yc_connect::google::TokenStore;
use yc_core::Error;
use yc_core::meeting::ParticipantHint;

use crate::integrations_api::*;

/// How often the detector looks (it only does real work while some process holds the microphone).
const DETECT_EVERY: Duration = Duration::from_secs(5);
/// A detected call that nobody answered is forgotten after this long.
const DETECTED_TTL_MINUTES: i64 = 15;
/// `integrations.json` keeps at most this many event -> meeting links.
const MAX_LINKS: usize = 300;
/// Where the setup instructions live for people who build YadaCall themselves.
pub const GOOGLE_DOCS_URL: &str =
    "https://github.com/limeflash/yadacall/blob/main/docs/GOOGLE_OAUTH.md";
const MEET_ID: &str = "meet";

/// Called for `AutoRecord` (policy "Всегда"): the shell starts the recorder with this outcome.
pub type AutoRecord = Arc<dyn Fn(JoinOutcome) + Send + Sync>;
/// Opens a URL in the user's browser (Tauri's opener in the app).
pub type OpenUrl = Arc<dyn Fn(&str) + Send + Sync>;

type CResult<T> = std::result::Result<T, Error>;

// ---- persisted preferences --------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Prefs {
    reminder: ReminderSettings,
    /// Services the user marked "Хочу такое".
    want: BTreeSet<String>,
    /// `AutoRecord` also opens the Meet link (off: the user is usually already in the call).
    auto_record_opens_meet: bool,
    /// `(event id, library meeting id)`, oldest first.
    event_meetings: Vec<(String, String)>,
}

impl Prefs {
    fn load(path: &Path) -> Self {
        let mut prefs = match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!("{}: {e}; starting with defaults", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        };
        // This is runtime state; do not restore a stale Automation denial from a prior launch.
        prefs.reminder.browser_automation_permission_required = None;
        prefs
    }

    fn save(&self, path: &Path) {
        let result = serde_json::to_vec_pretty(self)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                let tmp = path.with_extension("json.tmp");
                std::fs::write(&tmp, bytes)?;
                std::fs::rename(&tmp, path)
            });
        if let Err(e) = result {
            tracing::warn!("could not save {}: {e}", path.display());
        }
    }
}

// ---- the implementation ------------------------------------------------------------------------------------

/// [`Integrations`] over a [`yc::ConnectService`].
pub struct RealIntegrations {
    svc: yc::ConnectService,
    clock: Arc<dyn yc::Clock>,
    sink: IntegrationSink,
    open: OpenUrl,
    prefs_path: PathBuf,
    prefs: Mutex<Prefs>,
    browser_automation_permission_required: Arc<Mutex<Option<String>>>,
    /// Sign-in in progress (add or re-auth); cancelled by `cancel_add_account`.
    signing_in: Mutex<Option<CancellationToken>>,
    connecting: Mutex<HashSet<String>>,
    /// Per-event record decisions that differ from the account policy.
    overrides: Mutex<HashMap<String, bool>>,
    detected: Mutex<Option<DetectedCall>>,
    detection: Mutex<Option<CancellationToken>>,
    recording: AtomicBool,
    detected_seq: AtomicU64,
    shutdown: CancellationToken,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl RealIntegrations {
    /// The app wiring: accounts and preferences under `data_dir`, tokens in the OS keychain, wall clock.
    pub fn open(data_dir: &Path, sink: IntegrationSink, open: OpenUrl) -> CResult<Arc<Self>> {
        std::fs::create_dir_all(data_dir)?;
        let cfg = yc::ConnectConfig::new(data_dir.join("connect.json"));
        Self::new(
            cfg,
            data_dir.join("integrations.json"),
            Arc::new(yc::google::KeychainTokenStore),
            Arc::new(yc::SystemClock),
            sink,
            open,
        )
    }

    /// Builds the integrations from explicit parts (tests use the in-memory token store and a manual clock).
    pub fn new(
        mut cfg: yc::ConnectConfig,
        prefs_path: PathBuf,
        store: Arc<dyn TokenStore>,
        clock: Arc<dyn yc::Clock>,
        sink: IntegrationSink,
        open: OpenUrl,
    ) -> CResult<Arc<Self>> {
        let prefs = Prefs::load(&prefs_path);
        cfg.reminder = scheduler_settings(&prefs.reminder);
        let svc = yc::ConnectService::new(cfg, store, clock.clone())?;
        Ok(Arc::new(Self {
            svc,
            clock,
            sink,
            open,
            prefs_path,
            prefs: Mutex::new(prefs),
            browser_automation_permission_required: Arc::new(Mutex::new(None)),
            signing_in: Mutex::new(None),
            connecting: Mutex::new(HashSet::new()),
            overrides: Mutex::new(HashMap::new()),
            detected: Mutex::new(None),
            detection: Mutex::new(None),
            recording: AtomicBool::new(false),
            detected_seq: AtomicU64::new(0),
            shutdown: CancellationToken::new(),
        }))
    }

    /// Starts refreshing the agenda, the scheduler, call detection and the event forwarder. Runs until
    /// [`RealIntegrations::stop`]; must be awaited inside the async runtime.
    pub async fn run(self: Arc<Self>, auto_record: AutoRecord) {
        let mut events = self.svc.subscribe();
        self.svc.start(self.shutdown.clone());
        self.apply_detection();
        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => break,
                received = events.recv() => match received {
                    Ok(event) => self.handle(event, &auto_record),
                    Err(RecvError::Lagged(n)) => tracing::warn!("missed {n} connect events"),
                    Err(RecvError::Closed) => break,
                },
            }
        }
    }

    /// Stops the background loops.
    pub fn stop(&self) {
        self.shutdown.cancel();
        if let Some(t) = lock(&self.detection).take() {
            t.cancel();
        }
    }

    /// The underlying service (for the shell: wake on resume, tests).
    pub fn service(&self) -> &yc::ConnectService {
        &self.svc
    }

    /// The engine started or stopped recording: call detection must ignore our own microphone use.
    pub fn set_recording(&self, recording: bool) {
        self.recording.store(recording, Ordering::Relaxed);
        self.svc.set_recording(recording);
        if recording {
            *lock(&self.detected) = None;
        }
    }

    /// Remembers which library meeting recorded `event_id` (shown on the agenda card afterwards).
    pub fn note_meeting(&self, event_id: &str, meeting_id: &str) {
        let mut prefs = lock(&self.prefs);
        prefs.event_meetings.retain(|(e, _)| e != event_id);
        prefs
            .event_meetings
            .push((event_id.to_owned(), meeting_id.to_owned()));
        let excess = prefs.event_meetings.len().saturating_sub(MAX_LINKS);
        prefs.event_meetings.drain(..excess);
        prefs.save(&self.prefs_path);
        drop(prefs);
        self.emit(IntegrationEvent::AgendaChanged);
    }

    fn emit(&self, e: IntegrationEvent) {
        (self.sink)(e);
    }

    fn save_prefs(&self, f: impl FnOnce(&mut Prefs)) -> Prefs {
        let mut prefs = lock(&self.prefs);
        f(&mut prefs);
        prefs.save(&self.prefs_path);
        prefs.clone()
    }

    // ---- ConnectEvent -> IntegrationEvent ------------------------------------------------------------------

    fn handle(&self, event: yc::ConnectEvent, auto_record: &AutoRecord) {
        match event {
            yc::ConnectEvent::ReminderDue {
                event,
                lead_minutes,
            } => {
                if let Some((dto, label, email)) = self.dto_with_owner(&event) {
                    self.emit(IntegrationEvent::ReminderDue {
                        event: dto,
                        account_label: label,
                        account_email: email,
                        minutes_before: u8::try_from(lead_minutes).unwrap_or(u8::MAX),
                    });
                }
            }
            yc::ConnectEvent::CallStarting { event } => {
                if let Some((dto, label, email)) = self.dto_with_owner(&event) {
                    self.emit(IntegrationEvent::CallStarting {
                        event: dto,
                        account_label: label,
                        account_email: email,
                    });
                }
            }
            yc::ConnectEvent::AutoRecord { event } => {
                if self.recording.load(Ordering::Relaxed) {
                    return; // one recording at a time
                }
                let Some(meta) = self.svc.accounts().get(&event.account) else {
                    return;
                };
                let open = lock(&self.prefs).auto_record_opens_meet;
                self.svc.note_recording(&event.id, event.end);
                auto_record(self.outcome(&event, &meta, true, open));
            }
            yc::ConnectEvent::AccountAuthExpired { account } => {
                let email = self
                    .svc
                    .accounts()
                    .get(&account)
                    .map_or_else(|| account.clone(), |m| m.email);
                self.emit(IntegrationEvent::AuthExpired {
                    account_id: account,
                    email,
                });
            }
            yc::ConnectEvent::AgendaChanged => self.emit(IntegrationEvent::AgendaChanged),
            yc::ConnectEvent::CallDetected { call } => self.on_detected(&call),
        }
    }

    fn on_detected(&self, call: &yc::DetectedCall) {
        if !lock(&self.prefs).reminder.detect_unscheduled
            || self.recording.load(Ordering::Relaxed)
            || call.matched_event_id.is_some()
        {
            return;
        }
        let dto = DetectedCall {
            id: format!(
                "detected-{}",
                self.detected_seq.fetch_add(1, Ordering::Relaxed)
            ),
            platform: platform_name(&call.app),
            url: detected_url(call),
            code: call.code.clone(),
            title: call.title.clone(),
            detected_at: self.clock.now(),
        };
        tracing::info!(
            platform = %dto.platform,
            browser = ?call.browser,
            "unscheduled call detected"
        );
        *lock(&self.detected) = Some(dto.clone());
        self.emit(IntegrationEvent::CallDetected { call: dto });
    }

    /// (Re)starts detection. A browser tab is inspected only when that browser holds the microphone and the user
    /// enabled browser access.
    fn apply_detection(&self) {
        let settings = lock(&self.prefs).reminder.clone();
        let mut slot = lock(&self.detection);
        if let Some(t) = slot.take() {
            t.cancel();
        }
        if !settings.detect_unscheduled {
            *lock(&self.detected) = None;
            *lock(&self.browser_automation_permission_required) = None;
            self.svc.report_detection(None);
            return;
        }
        if !settings.browser_access {
            *lock(&self.browser_automation_permission_required) = None;
        }
        let token = self.shutdown.child_token();
        let permission_state = self.browser_automation_permission_required.clone();
        self.svc.start_detection_with_status(
            system_detector(settings.browser_access),
            DETECT_EVERY,
            token.clone(),
            move |browser| {
                let mut current = lock(&permission_state);
                if *current != browser {
                    tracing::info!(
                        browser = ?browser,
                        "browser Automation permission state changed"
                    );
                    *current = browser;
                }
            },
        );
        *slot = Some(token);
    }

    // ---- DTO mapping ---------------------------------------------------------------------------------------

    fn account_dto(&self, meta: &yc::AccountMeta, agenda: &[yc::AgendaEvent]) -> CalendarAccount {
        let status = if lock(&self.connecting).contains(&meta.id) {
            AccountStatus::Connecting
        } else if meta.state == yc::AccountState::AuthExpired {
            AccountStatus::Expired
        } else {
            AccountStatus::Ok
        };
        let from = local_midnight(self.clock.now());
        let to = from + chrono::Duration::days(7);
        CalendarAccount {
            id: meta.id.clone(),
            label: meta.label.clone(),
            email: meta.email.clone(),
            color: meta.color.clone(),
            workspace: meta.is_workspace(),
            status,
            meetings_this_week: agenda
                .iter()
                .filter(|e| {
                    e.account == meta.id && e.has_call_link() && e.start >= from && e.start < to
                })
                .count() as u32,
            record_policy: policy_dto(meta.policy),
            visible: meta.visible,
        }
    }

    /// Events of visible, working accounts, with the labels the UI needs.
    fn live_agenda(&self) -> Vec<yc::AgendaEvent> {
        let working: HashSet<String> = self
            .svc
            .accounts()
            .list()
            .into_iter()
            .filter(|m| m.visible && m.state == yc::AccountState::Ok)
            .map(|m| m.id)
            .collect();
        self.svc
            .agenda()
            .into_iter()
            .filter(|e| working.contains(&e.account))
            .collect()
    }

    fn event_dto(&self, e: &yc::AgendaEvent, live: &[yc::AgendaEvent]) -> Option<AgendaEvent> {
        let meta = self.svc.accounts().get(&e.account)?;
        let overridden = lock(&self.overrides).get(&e.id).copied();
        let meeting_id = lock(&self.prefs)
            .event_meetings
            .iter()
            .rev()
            .find(|(ev, _)| *ev == e.id)
            .map(|(_, m)| m.clone());
        Some(AgendaEvent {
            id: e.id.clone(),
            account_id: meta.id.clone(),
            title: e.title.clone(),
            start: e.start,
            end: e.end,
            meet_url: e.meet_url.clone(),
            attendees: remote_attendees(e),
            record_policy: policy_dto(meta.policy),
            will_record: overridden.unwrap_or(meta.policy == yc::AccountPolicy::Always),
            overlaps: e
                .overlaps
                .iter()
                .filter(|id| live.iter().any(|o| &o.id == *id && o.account != e.account))
                .cloned()
                .collect(),
            meeting_id,
        })
    }

    /// DTO plus the owning account's label and e-mail, for reminders. The shell's agenda is the live one.
    fn dto_with_owner(&self, e: &yc::AgendaEvent) -> Option<(AgendaEvent, String, String)> {
        let live = self.live_agenda();
        let meta = self.svc.accounts().get(&e.account)?;
        Some((self.event_dto(e, &live)?, meta.label, meta.email))
    }

    // ---- joining ---------------------------------------------------------------------------------------------

    fn find_event(&self, id: &str) -> CResult<(yc::AgendaEvent, yc::AccountMeta)> {
        let event = self
            .svc
            .agenda()
            .into_iter()
            .find(|e| e.id == id)
            .ok_or_else(|| Error::NotFound(format!("event {id}")))?;
        let meta = self
            .svc
            .accounts()
            .get(&event.account)
            .ok_or_else(|| Error::NotFound(format!("account {}", event.account)))?;
        Ok((event, meta))
    }

    /// What the shell must do for `event`: where to go (`open`: pinned to the owning Google account unless the
    /// user switched that off) and what to tell the recorder.
    fn outcome(
        &self,
        e: &yc::AgendaEvent,
        meta: &yc::AccountMeta,
        record: bool,
        open: bool,
    ) -> JoinOutcome {
        let pin = lock(&self.prefs).reminder.open_in_account;
        let remote = remote_hints(e);
        JoinOutcome {
            title: e.title.clone(),
            meet_url: if open {
                yc::join_url(e, pin.then_some(meta.email.as_str()))
            } else {
                None
            },
            account_email: meta.email.clone(),
            record,
            ends_at: e.end,
            event_id: Some(e.id.clone()),
            expected_speakers: if record {
                expected_speakers(remote.len())
            } else {
                None
            },
            participants: if record { remote } else { Vec::new() },
        }
    }

    fn set_decision(&self, event_id: &str, policy: yc::AccountPolicy, record: bool) {
        // Only a decision that differs from the account policy is stored; matching it clears the override so the
        // reminder still appears for "Спрашивать" accounts.
        let default_record = policy == yc::AccountPolicy::Always;
        let mut map = lock(&self.overrides);
        if record == default_record {
            map.remove(event_id);
            self.svc.set_override(event_id, None);
        } else {
            map.insert(event_id.to_owned(), record);
            self.svc.set_override(
                event_id,
                Some(if record {
                    yc::EventOverride::Record
                } else {
                    yc::EventOverride::DontRecord
                }),
            );
        }
    }

    fn forget_decisions_of(&self, account: &str) {
        let ids: Vec<String> = self
            .svc
            .agenda()
            .into_iter()
            .filter(|e| e.account == account)
            .map(|e| e.id)
            .collect();
        let mut map = lock(&self.overrides);
        for id in ids {
            map.remove(&id);
            self.svc.set_override(&id, None);
        }
    }

    /// Browser sign-in shared by "add" and "sign in again".
    fn begin_sign_in(&self) -> CancellationToken {
        let token = self.shutdown.child_token();
        if let Some(previous) = lock(&self.signing_in).replace(token.clone()) {
            previous.cancel();
        }
        token
    }

    fn opener(&self) -> impl Fn(&str) + Send + Sync + use<> {
        let open = self.open.clone();
        move |url: &str| open(url)
    }
}

#[async_trait]
impl Integrations for RealIntegrations {
    async fn accounts(&self) -> CResult<Vec<CalendarAccount>> {
        let agenda = self.svc.agenda();
        Ok(self
            .svc
            .accounts()
            .list()
            .iter()
            .filter(|m| m.kind == yc::AccountKind::Google)
            .map(|m| self.account_dto(m, &agenda))
            .collect())
    }

    async fn add_google_account(&self) -> CResult<CalendarAccount> {
        if !self.svc.google_configured() {
            return Err(yc::ConnectError::NotConfigured.into());
        }
        let token = self.begin_sign_in();
        self.emit(IntegrationEvent::AddProgress { step: 1 });
        let result = self.svc.add_google_account(self.opener(), &token).await;
        let meta = result?;
        self.emit(IntegrationEvent::AddProgress { step: 3 });
        self.svc.refresh().await;
        self.emit(IntegrationEvent::AgendaChanged);
        let agenda = self.svc.agenda();
        let meta = self.svc.accounts().get(&meta.id).unwrap_or(meta);
        Ok(self.account_dto(&meta, &agenda))
    }

    async fn cancel_add_account(&self) -> CResult<()> {
        if let Some(t) = lock(&self.signing_in).take() {
            t.cancel();
        }
        Ok(())
    }

    async fn remove_account(&self, id: &str) -> CResult<()> {
        self.forget_decisions_of(id);
        let outcome = self.svc.remove_account(id).await?;
        if !outcome.revoked {
            tracing::warn!("Google did not confirm the revoke for {id}; the local sign-in is gone");
        }
        self.emit(IntegrationEvent::AgendaChanged);
        Ok(())
    }

    async fn reauth_account(&self, id: &str) -> CResult<CalendarAccount> {
        let meta = self
            .svc
            .accounts()
            .get(id)
            .ok_or_else(|| Error::NotFound(format!("account {id}")))?;
        let token = self.begin_sign_in();
        lock(&self.connecting).insert(meta.id.clone());
        self.emit(IntegrationEvent::AgendaChanged);
        let result = self
            .svc
            .reauthenticate(&meta.id, self.opener(), &token)
            .await;
        lock(&self.connecting).remove(&meta.id);
        if result.is_ok() {
            self.svc.refresh().await;
        }
        self.emit(IntegrationEvent::AgendaChanged);
        result?;
        let meta = self
            .svc
            .accounts()
            .get(id)
            .ok_or_else(|| Error::NotFound(format!("account {id}")))?;
        Ok(self.account_dto(&meta, &self.svc.agenda()))
    }

    async fn set_account_policy(&self, id: &str, policy: RecordPolicy) -> CResult<()> {
        self.forget_decisions_of(id);
        self.svc.set_policy(id, policy_from_dto(policy))?;
        self.emit(IntegrationEvent::AgendaChanged);
        Ok(())
    }

    async fn set_account_visible(&self, id: &str, visible: bool) -> CResult<()> {
        self.svc.set_visible(id, visible)?;
        self.emit(IntegrationEvent::AgendaChanged);
        Ok(())
    }

    async fn agenda(&self, range: TimeRange) -> CResult<Vec<AgendaEvent>> {
        let live = self.live_agenda();
        Ok(live
            .iter()
            .filter(|e| e.start < range.to && e.end > range.from)
            .filter_map(|e| self.event_dto(e, &live))
            .collect())
    }

    async fn set_event_recording(&self, event_id: &str, record: bool) -> CResult<()> {
        let (event, meta) = self.find_event(event_id)?;
        self.set_decision(&event.id, meta.policy, record);
        Ok(())
    }

    async fn join_and_record(&self, event_id: &str) -> CResult<JoinOutcome> {
        let (event, meta) = self.find_event(event_id)?;
        // The user chose: no second prompt and no automatic start for this event.
        self.set_decision(&event.id, meta.policy, true);
        self.svc.note_recording(&event.id, event.end);
        Ok(self.outcome(&event, &meta, true, true))
    }

    async fn join_without_recording(&self, event_id: &str) -> CResult<JoinOutcome> {
        let (event, meta) = self.find_event(event_id)?;
        self.set_decision(&event.id, meta.policy, false);
        Ok(self.outcome(&event, &meta, false, true))
    }

    async fn connections(&self) -> CResult<Vec<Connection>> {
        let working = self
            .svc
            .accounts()
            .list()
            .iter()
            .filter(|m| m.kind == yc::AccountKind::Google && m.state == yc::AccountState::Ok)
            .count() as u32;
        let want = lock(&self.prefs).want.clone();
        Ok(catalog(self.svc.google_configured(), working, &want))
    }

    async fn toggle_want(&self, service: &str) -> CResult<bool> {
        if !CATALOG.iter().any(|c| c.id == service && c.id != MEET_ID) {
            return Err(Error::Invalid(format!(
                "{service} is not a coming-soon service"
            )));
        }
        let mut on = false;
        self.save_prefs(|p| {
            on = p.want.insert(service.to_owned());
            if !on {
                p.want.remove(service);
            }
        });
        Ok(on)
    }

    async fn reminder_settings(&self) -> CResult<ReminderSettings> {
        let mut settings = lock(&self.prefs).reminder.clone();
        settings.browser_automation_permission_required =
            lock(&self.browser_automation_permission_required).clone();
        Ok(settings)
    }

    async fn set_reminder_settings(&self, mut s: ReminderSettings) -> CResult<ReminderSettings> {
        if !matches!(s.remind_minutes, 1 | 2 | 5) {
            return Err(Error::Invalid("remind_minutes must be 1, 2 or 5".into()));
        }
        // The permission state is runtime-only; ignore client values and do not persist it.
        s.browser_automation_permission_required = None;
        let before = lock(&self.prefs).reminder.clone();
        self.save_prefs(|p| p.reminder = s.clone());
        self.svc.set_reminder_settings(scheduler_settings(&s));
        if s.detect_unscheduled != before.detect_unscheduled
            || s.browser_access != before.browser_access
        {
            *lock(&self.browser_automation_permission_required) = None;
            self.apply_detection();
        }
        s.browser_automation_permission_required =
            lock(&self.browser_automation_permission_required).clone();
        Ok(s)
    }

    async fn detected_call(&self) -> CResult<Option<DetectedCall>> {
        let now = self.clock.now();
        let mut slot = lock(&self.detected);
        if slot
            .as_ref()
            .is_some_and(|c| now - c.detected_at > chrono::Duration::minutes(DETECTED_TTL_MINUTES))
        {
            *slot = None;
        }
        Ok(slot.clone())
    }

    async fn resolve_detected_call(&self, id: &str, record: bool) -> CResult<Option<JoinOutcome>> {
        let call = {
            let mut slot = lock(&self.detected);
            if slot.as_ref().is_none_or(|c| c.id != id) {
                return Ok(None);
            }
            slot.take()
        };
        let Some(call) = call.filter(|_| record) else {
            return Ok(None);
        };
        let room = call
            .code
            .as_deref()
            .or_else(|| call.url.as_deref().and_then(|url| url.rsplit('/').next()))
            .unwrap_or_default();
        Ok(Some(JoinOutcome {
            title: if call.title.is_empty() {
                format!("{} · {room}", call.platform)
            } else {
                call.title
            },
            meet_url: None,
            account_email: String::new(),
            record: true,
            ends_at: self.clock.now() + chrono::Duration::hours(1),
            event_id: None,
            expected_speakers: None,
            participants: Vec::new(),
        }))
    }
}

// ---- helpers -------------------------------------------------------------------------------------------------

fn scheduler_settings(s: &ReminderSettings) -> yc::ReminderSettings {
    yc::ReminderSettings {
        lead_minutes: u32::from(s.remind_minutes),
        ..yc::ReminderSettings::default()
    }
}

fn policy_dto(p: yc::AccountPolicy) -> RecordPolicy {
    match p {
        yc::AccountPolicy::Ask => RecordPolicy::Ask,
        yc::AccountPolicy::Always => RecordPolicy::Always,
        yc::AccountPolicy::Never => RecordPolicy::Never,
    }
}

fn policy_from_dto(p: RecordPolicy) -> yc::AccountPolicy {
    match p {
        RecordPolicy::Ask => yc::AccountPolicy::Ask,
        RecordPolicy::Always => yc::AccountPolicy::Always,
        RecordPolicy::Never => yc::AccountPolicy::Never,
    }
}

fn local_midnight(now: DateTime<Utc>) -> DateTime<Utc> {
    let day = now.with_timezone(&Local).date_naive();
    day.and_hms_opt(0, 0, 0)
        .and_then(|t| Local.from_local_datetime(&t).earliest())
        .map_or(now, |t| t.with_timezone(&Utc))
}

/// The people the user will talk to (name and email): everyone but the user, declined invitations and rooms.
pub fn remote_hints(e: &yc::AgendaEvent) -> Vec<ParticipantHint> {
    e.attendees
        .iter()
        .filter(|a| !a.is_self && a.response.as_deref() != Some("declined"))
        .filter(|a| {
            !a.email
                .as_deref()
                .is_some_and(|m| m.ends_with("@resource.calendar.google.com"))
        })
        .filter_map(|a| {
            let name = a
                .name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .or_else(|| a.email.as_deref().and_then(|m| m.split('@').next()))?;
            Some(ParticipantHint {
                name: name.to_owned(),
                email: a.email.clone().filter(|m| !m.trim().is_empty()),
            })
        })
        .collect()
}

/// Display names of [`remote_hints`].
pub fn remote_attendees(e: &yc::AgendaEvent) -> Vec<String> {
    remote_hints(e).into_iter().map(|h| h.name).collect()
}

/// `None` when the guest list is unknown (an ad-hoc event), so diarisation decides.
pub fn expected_speakers(remote: usize) -> Option<u8> {
    (remote > 0).then(|| u8::try_from(remote).unwrap_or(u8::MAX))
}

fn platform_name(app: &yc::types::DetectedApp) -> String {
    use yc::types::DetectedApp::*;
    match app {
        GoogleMeet => "Google Meet".into(),
        Zoom => "Zoom".into(),
        Teams => "Microsoft Teams".into(),
        Telegram => "Telegram".into(),
        Slack => "Slack".into(),
        Discord => "Discord".into(),
        Telemost => "Yandex Telemost".into(),
        Jitsi => "Jitsi".into(),
        Other(name) => name.clone(),
    }
}

/// A readable call address, or `None` when only the active app is known.
fn detected_url(call: &yc::DetectedCall) -> Option<String> {
    if let Some(code) = &call.meet_code {
        return Some(format!("meet.google.com/{code}"));
    }
    call.url.as_deref().map(|u| {
        u.trim_start_matches("https://")
            .trim_start_matches("http://")
            .to_owned()
    })
}

struct CatalogEntry {
    id: &'static str,
    name: &'static str,
    mono: &'static str,
    description: &'static str,
}

const CATALOG: [CatalogEntry; 6] = [
    CatalogEntry {
        id: MEET_ID,
        name: "Google Meet",
        mono: "G",
        description: "Встречи из Google Календаря, напоминание и вход в звонок сразу в нужном аккаунте.",
    },
    CatalogEntry {
        id: "zoom",
        name: "Zoom",
        mono: "Z",
        description: "Встречи из Zoom и вход по ссылке без выбора аккаунта.",
    },
    CatalogEntry {
        id: "teams",
        name: "Microsoft Teams",
        mono: "T",
        description: "Встречи из календаря Outlook и звонки в Teams.",
    },
    CatalogEntry {
        id: "discord",
        name: "Discord",
        mono: "D",
        description: "Голосовые каналы и звонки на серверах, где ты есть.",
    },
    CatalogEntry {
        id: "telegram",
        name: "Telegram",
        mono: "T",
        description: "Групповые и личные звонки в Telegram.",
    },
    CatalogEntry {
        id: "slack",
        name: "Slack",
        mono: "S",
        description: "Хаддлы в каналах и личных сообщениях.",
    },
];

/// The catalog; only Google Meet is real, the rest are "Скоро" with the local "Хочу такое" flag.
fn catalog(
    google_configured: bool,
    working_accounts: u32,
    want: &BTreeSet<String>,
) -> Vec<Connection> {
    CATALOG
        .iter()
        .map(|c| {
            let meet = c.id == MEET_ID;
            Connection {
                id: c.id.into(),
                name: c.name.into(),
                mono: c.mono.into(),
                description: c.description.into(),
                state: match (meet, google_configured) {
                    (false, _) => ConnectionState::Soon,
                    (true, true) => ConnectionState::Connected,
                    (true, false) => ConnectionState::Unconfigured,
                },
                accounts: (meet && google_configured).then_some(working_accounts),
                want: !meet && want.contains(c.id),
                docs_url: (meet && !google_configured).then(|| GOOGLE_DOCS_URL.into()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as D;
    use yc::google::{ClientCredentials, MemoryTokenStore};

    const ACCOUNTS: &str = r##"{"version":1,"accounts":[
        {"id":"lime@company.com","kind":"google","email":"lime@company.com","name":"Lime","picture":null,"hd":"company.com",
         "label":"Рабочий","policy":"always","visible":true,"color":"#c9f23b","state":"ok"},
        {"id":"lime@gmail.com","kind":"google","email":"lime@gmail.com","name":"Lime","picture":null,"hd":null,
         "label":"Личный","policy":"ask","visible":true,"color":"#8fb8ff","state":"authExpired"}]}"##;

    struct Rig {
        real: Arc<RealIntegrations>,
        events: Arc<Mutex<Vec<IntegrationEvent>>>,
        _dir: tempfile::TempDir,
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 12, 11, 55, 0).unwrap()
    }

    fn rig(configured: bool) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("connect.json"), ACCOUNTS).unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink: IntegrationSink = {
            let events = events.clone();
            Arc::new(move |e| lock(&events).push(e))
        };
        let mut cfg = yc::ConnectConfig::new(dir.path().join("connect.json"));
        cfg.client_override = configured.then(|| ClientCredentials {
            client_id: "test.apps.googleusercontent.com".into(),
            client_secret: None,
        });
        let real = RealIntegrations::new(
            cfg,
            dir.path().join("integrations.json"),
            Arc::new(MemoryTokenStore::default()),
            Arc::new(yc::ManualClock::new(now())),
            sink,
            Arc::new(|_| {}),
        )
        .unwrap();
        Rig {
            real,
            events,
            _dir: dir,
        }
    }

    fn person(name: Option<&str>, email: &str, is_self: bool, response: &str) -> yc::Attendee {
        yc::Attendee {
            name: name.map(str::to_owned),
            email: Some(email.to_owned()),
            is_self,
            response: Some(response.to_owned()),
        }
    }

    fn event(
        account: &str,
        id: &str,
        start_in_min: i64,
        attendees: Vec<yc::Attendee>,
    ) -> yc::AgendaEvent {
        let start = now() + D::minutes(start_in_min);
        yc::AgendaEvent {
            id: id.into(),
            account: account.into(),
            calendar: "primary".into(),
            calendar_title: None,
            title: "1:1 с Аней".into(),
            start,
            end: start + D::minutes(30),
            meet_url: Some("https://meet.google.com/abc-defg-hij".into()),
            other_links: vec![],
            attendees,
            organizer: None,
            ical_uid: None,
            overlaps: vec![],
            also_in: vec![],
        }
    }

    fn guests() -> Vec<yc::Attendee> {
        vec![
            person(Some("Лайм"), "lime@company.com", true, "accepted"),
            person(Some("Аня Петрова"), "anya@company.com", false, "accepted"),
            person(None, "boris@company.com", false, "tentative"),
            person(
                Some("Переговорка 3"),
                "x@resource.calendar.google.com",
                false,
                "accepted",
            ),
            person(Some("Вова"), "vova@company.com", false, "declined"),
        ]
    }

    #[test]
    fn attendees_exclude_self_rooms_and_declined() {
        let e = event("lime@company.com", "e1", 5, guests());
        let names = remote_attendees(&e);
        assert_eq!(names, ["Аня Петрова", "boris"]);
        assert_eq!(expected_speakers(names.len()), Some(2));
        assert_eq!(expected_speakers(0), None);
        let hints = remote_hints(&e);
        assert_eq!(hints[0].email.as_deref(), Some("anya@company.com"));
        assert_eq!(hints[1].name, "boris");
    }

    #[test]
    fn outcome_pins_the_google_account_and_carries_the_guest_list() {
        let r = rig(true);
        let e = event("lime@company.com", "e1", 5, guests());
        let meta = r.real.svc.accounts().get("lime@company.com").unwrap();
        let out = r.real.outcome(&e, &meta, true, true);
        assert!(
            out.meet_url
                .as_deref()
                .unwrap()
                .contains("authuser=lime%40company.com")
        );
        assert_eq!(out.expected_speakers, Some(2));
        let names: Vec<_> = out.participants.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Аня Петрова", "boris"]);
        assert_eq!(
            out.participants[0].email.as_deref(),
            Some("anya@company.com")
        );
        assert_eq!(out.event_id.as_deref(), Some("e1"));

        let no_browser = r.real.outcome(&e, &meta, true, false);
        assert_eq!(no_browser.meet_url, None);

        let plain = r.real.outcome(&e, &meta, false, true);
        assert!(plain.expected_speakers.is_none() && plain.participants.is_empty());

        lock(&r.real.prefs).reminder.open_in_account = false;
        let unpinned = r.real.outcome(&e, &meta, true, true);
        assert_eq!(
            unpinned.meet_url.as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );
    }

    #[test]
    fn unknown_guest_list_leaves_speakers_to_diarisation() {
        let r = rig(true);
        let e = event("lime@company.com", "e2", 5, vec![]);
        let meta = r.real.svc.accounts().get("lime@company.com").unwrap();
        assert_eq!(
            r.real.outcome(&e, &meta, true, true).expected_speakers,
            None
        );
    }

    #[test]
    fn will_record_follows_policy_and_per_event_choice() {
        let r = rig(true);
        let e = event("lime@company.com", "e1", 5, guests());
        let live = vec![e.clone()];
        assert!(
            r.real.event_dto(&e, &live).unwrap().will_record,
            "policy Always"
        );
        r.real.set_decision("e1", yc::AccountPolicy::Always, false);
        assert!(!r.real.event_dto(&e, &live).unwrap().will_record);
        r.real.set_decision("e1", yc::AccountPolicy::Always, true);
        assert!(
            lock(&r.real.overrides).is_empty(),
            "matching the policy stores nothing"
        );

        let ask = event("lime@gmail.com", "e3", 5, vec![]);
        assert!(
            !r.real
                .event_dto(&ask, std::slice::from_ref(&ask))
                .unwrap()
                .will_record
        );
        r.real.set_decision("e3", yc::AccountPolicy::Ask, true);
        assert!(
            r.real
                .event_dto(&ask, std::slice::from_ref(&ask))
                .unwrap()
                .will_record
        );
    }

    #[test]
    fn overlaps_only_name_other_accounts() {
        let r = rig(true);
        let mut a = event("lime@company.com", "a", 5, vec![]);
        let mut b = event("lime@gmail.com", "b", 10, vec![]);
        let mut c = event("lime@company.com", "c", 15, vec![]);
        a.overlaps = vec!["b".into(), "c".into()];
        b.overlaps = vec!["a".into(), "c".into()];
        c.overlaps = vec!["a".into(), "b".into()];
        let live = vec![a.clone(), b, c];
        assert_eq!(r.real.event_dto(&a, &live).unwrap().overlaps, ["b"]);
    }

    #[tokio::test]
    async fn accounts_show_expired_status_and_policy() {
        let r = rig(true);
        let accounts = r.real.accounts().await.unwrap();
        assert_eq!(accounts.len(), 2);
        assert!(accounts[0].workspace && accounts[0].status == AccountStatus::Ok);
        assert_eq!(accounts[0].record_policy, RecordPolicy::Always);
        assert_eq!(accounts[1].status, AccountStatus::Expired);
        lock(&r.real.connecting).insert("lime@gmail.com".into());
        assert_eq!(
            r.real.accounts().await.unwrap()[1].status,
            AccountStatus::Connecting
        );
    }

    #[tokio::test]
    async fn adding_an_account_without_an_oauth_client_is_a_clear_error() {
        // The environment may carry a client; only assert when this build is unconfigured.
        let r = rig(false);
        if !r.real.svc.google_configured() {
            let err = r.real.add_google_account().await.unwrap_err();
            assert!(err.to_string().contains("GOOGLE_OAUTH.md"), "{err}");
            let meet = &r.real.connections().await.unwrap()[0];
            assert_eq!(meet.state, ConnectionState::Unconfigured);
            assert!(
                meet.docs_url
                    .as_deref()
                    .unwrap()
                    .ends_with("GOOGLE_OAUTH.md")
            );
        }
    }

    #[tokio::test]
    async fn catalog_marks_only_meet_real_and_persists_want() {
        let r = rig(true);
        let all = r.real.connections().await.unwrap();
        assert_eq!(all[0].state, ConnectionState::Connected);
        assert_eq!(
            all[0].accounts,
            Some(1),
            "the expired account does not count"
        );
        assert!(
            all[1..]
                .iter()
                .all(|c| c.state == ConnectionState::Soon && !c.want)
        );

        assert!(r.real.toggle_want("zoom").await.unwrap());
        assert!(r.real.toggle_want("meet").await.is_err());
        assert!(r.real.toggle_want("nope").await.is_err());
        let reloaded = Prefs::load(&r.real.prefs_path);
        assert!(reloaded.want.contains("zoom"));
        assert!(!r.real.toggle_want("zoom").await.unwrap());
        assert!(Prefs::load(&r.real.prefs_path).want.is_empty());
    }

    #[tokio::test]
    async fn reminder_settings_are_validated_and_persisted() {
        let r = rig(true);
        assert!(
            r.real
                .set_reminder_settings(ReminderSettings {
                    remind_minutes: 3,
                    ..Default::default()
                })
                .await
                .is_err()
        );
        let s = ReminderSettings {
            remind_minutes: 5,
            open_in_account: false,
            detect_unscheduled: false,
            browser_access: false,
            browser_automation_permission_required: None,
        };
        r.real.set_reminder_settings(s.clone()).await.unwrap();
        assert_eq!(Prefs::load(&r.real.prefs_path).reminder, s);
        assert_eq!(r.real.reminder_settings().await.unwrap(), s);
        assert!(lock(&r.real.detection).is_none(), "detection stays off");
    }

    #[test]
    fn connect_events_become_integration_events() {
        let r = rig(true);
        let auto: AutoRecord = Arc::new(|_| panic!("not an auto-record event"));
        let e = event("lime@gmail.com", "e1", 2, guests());
        r.real.handle(
            yc::ConnectEvent::ReminderDue {
                event: e.clone(),
                lead_minutes: 2,
            },
            &auto,
        );
        r.real
            .handle(yc::ConnectEvent::CallStarting { event: e }, &auto);
        r.real.handle(
            yc::ConnectEvent::AccountAuthExpired {
                account: "lime@gmail.com".into(),
            },
            &auto,
        );
        r.real.handle(yc::ConnectEvent::AgendaChanged, &auto);
        let got = lock(&r.events).clone();
        assert!(matches!(
            &got[0],
            IntegrationEvent::ReminderDue { account_label, account_email, minutes_before: 2, .. }
                if account_label == "Личный" && account_email == "lime@gmail.com"
        ));
        assert!(matches!(&got[1], IntegrationEvent::CallStarting { .. }));
        assert!(
            matches!(&got[2], IntegrationEvent::AuthExpired { email, .. } if email == "lime@gmail.com")
        );
        assert!(matches!(&got[3], IntegrationEvent::AgendaChanged));
    }

    #[test]
    fn auto_record_starts_without_opening_the_browser_and_never_twice() {
        let r = rig(true);
        let started = Arc::new(Mutex::new(Vec::<JoinOutcome>::new()));
        let auto: AutoRecord = {
            let started = started.clone();
            Arc::new(move |o| lock(&started).push(o))
        };
        let e = event("lime@company.com", "e1", 0, guests());
        r.real
            .handle(yc::ConnectEvent::AutoRecord { event: e.clone() }, &auto);
        {
            let got = lock(&started);
            assert_eq!(got.len(), 1);
            assert!(got[0].record && got[0].meet_url.is_none());
            assert_eq!(got[0].expected_speakers, Some(2));
        }
        lock(&r.real.prefs).auto_record_opens_meet = true;
        r.real.set_recording(true);
        r.real
            .handle(yc::ConnectEvent::AutoRecord { event: e.clone() }, &auto);
        assert_eq!(lock(&started).len(), 1, "already recording");
        r.real.set_recording(false);
        r.real
            .handle(yc::ConnectEvent::AutoRecord { event: e }, &auto);
        let got = lock(&started);
        assert_eq!(got.len(), 2);
        assert!(got[1].meet_url.is_some(), "configured to open Meet");
    }

    fn meet_call() -> yc::DetectedCall {
        yc::DetectedCall {
            app: yc::types::DetectedApp::GoogleMeet,
            meet_code: Some("kqv-tdre-xmn".into()),
            code: Some("kqv-tdre-xmn".into()),
            url: Some("https://meet.google.com/kqv-tdre-xmn".into()),
            browser: Some("Google Chrome".into()),
            title: "Weekly sync".into(),
            automation_permission_required: None,
            matched_event_id: None,
            certain: true,
        }
    }

    #[tokio::test]
    async fn detected_call_is_offered_and_resolved_once() {
        let r = rig(true);
        let auto: AutoRecord = Arc::new(|_| {});
        r.real
            .handle(yc::ConnectEvent::CallDetected { call: meet_call() }, &auto);
        let call = r.real.detected_call().await.unwrap().unwrap();
        assert_eq!(call.platform, "Google Meet");
        assert_eq!(call.url.as_deref(), Some("meet.google.com/kqv-tdre-xmn"));
        assert_eq!(call.code.as_deref(), Some("kqv-tdre-xmn"));
        assert_eq!(call.title, "Weekly sync");
        assert!(matches!(
            lock(&r.events).last(),
            Some(IntegrationEvent::CallDetected { .. })
        ));

        assert!(
            r.real
                .resolve_detected_call("other", true)
                .await
                .unwrap()
                .is_none()
        );
        let out = r
            .real
            .resolve_detected_call(&call.id, true)
            .await
            .unwrap()
            .unwrap();
        assert!(out.record && out.meet_url.is_none());
        assert_eq!(out.title, "Weekly sync");
        assert!(r.real.detected_call().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn declined_and_calendar_matched_calls_are_dropped() {
        let r = rig(true);
        let auto: AutoRecord = Arc::new(|_| {});
        r.real
            .handle(yc::ConnectEvent::CallDetected { call: meet_call() }, &auto);
        let id = r.real.detected_call().await.unwrap().unwrap().id;
        assert!(
            r.real
                .resolve_detected_call(&id, false)
                .await
                .unwrap()
                .is_none()
        );
        assert!(r.real.detected_call().await.unwrap().is_none());

        let mut matched = meet_call();
        matched.matched_event_id = Some("e1".into());
        r.real
            .handle(yc::ConnectEvent::CallDetected { call: matched }, &auto);
        r.real.set_recording(true);
        r.real
            .handle(yc::ConnectEvent::CallDetected { call: meet_call() }, &auto);
        assert!(r.real.detected_call().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn event_meeting_links_are_remembered_and_bounded() {
        let r = rig(true);
        r.real.note_meeting("e1", "m1");
        r.real.note_meeting("e1", "m2");
        let prefs = Prefs::load(&r.real.prefs_path);
        assert_eq!(prefs.event_meetings, [("e1".into(), "m2".into())]);
        for i in 0..MAX_LINKS + 20 {
            r.real.note_meeting(&format!("x{i}"), "m");
        }
        assert_eq!(lock(&r.real.prefs).event_meetings.len(), MAX_LINKS);
    }
}
