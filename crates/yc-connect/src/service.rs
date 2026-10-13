//! The async glue: periodic agenda refresh, scheduler ticks, detection loop and the broadcast
//! channel the app subscribes to.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{Notify, broadcast};
use tokio_util::sync::CancellationToken;

use crate::agenda::merge_agenda;
use crate::clock::Clock;
use crate::detect::{CallSources, Detector};
use crate::error::{ConnectError, ConnectResult};
use crate::google::calendar::{CalendarClient, CalendarSync};
use crate::google::oauth::DEFAULT_LOGIN_TIMEOUT;
use crate::google::{
    AccountManager, ClientCredentials, Endpoints, GoogleConfig, RemoveOutcome, TokenStore,
};
use crate::ics::IcsClient;
use crate::scheduler::{EventOverride, ReminderSettings, Scheduler};
use crate::types::{
    AccountKind, AccountMeta, AccountPolicy, AccountState, AgendaEvent, ConnectEvent, DetectedCall,
};

/// Configuration of the connect layer.
#[derive(Debug, Clone)]
pub struct ConnectConfig {
    /// JSON file with account metadata (label, policy, visibility, colour).
    pub accounts_path: PathBuf,
    /// Client ID/secret from settings; beats the environment and the compiled-in values.
    pub client_override: Option<ClientCredentials>,
    /// Endpoint overrides (tests, proxies). `None` uses Google's.
    pub endpoints: Option<Endpoints>,
    /// How often the agenda is refreshed (default 5 minutes).
    pub refresh_interval: Duration,
    /// How often the scheduler checks for due reminders (default 5 seconds).
    pub tick_interval: Duration,
    /// Days of history kept in the window (default 1, i.e. "yesterday").
    pub days_back: i64,
    /// Days ahead (default 7).
    pub days_ahead: i64,
    /// Reminder lead time and friends.
    pub reminder: ReminderSettings,
}

impl ConnectConfig {
    /// Defaults with the account file at `accounts_path`.
    pub fn new(accounts_path: impl Into<PathBuf>) -> Self {
        Self {
            accounts_path: accounts_path.into(),
            client_override: None,
            endpoints: None,
            refresh_interval: Duration::from_secs(300),
            tick_interval: Duration::from_secs(5),
            days_back: 1,
            days_ahead: 7,
            reminder: ReminderSettings::default(),
        }
    }
}

/// Outcome of one refresh.
#[derive(Debug)]
pub struct RefreshReport {
    /// The merged agenda after the refresh.
    pub agenda: Vec<AgendaEvent>,
    /// Accounts that could not be refreshed (their cached events are kept).
    pub errors: Vec<(String, ConnectError)>,
}

struct State {
    scheduler: Scheduler,
    google_sync: HashMap<String, HashMap<String, CalendarSync>>,
    per_account: HashMap<String, Vec<AgendaEvent>>,
    recording: bool,
}

struct Inner {
    cfg: ConnectConfig,
    accounts: Arc<AccountManager>,
    calendar: Option<CalendarClient>,
    ics: IcsClient,
    clock: Arc<dyn Clock>,
    events: broadcast::Sender<ConnectEvent>,
    state: Mutex<State>,
    wake: Notify,
    refresh_lock: tokio::sync::Mutex<()>,
}

/// Google accounts + ICS sources + scheduler behind one handle (cheap to clone).
#[derive(Clone)]
pub struct ConnectService {
    inner: Arc<Inner>,
}

impl ConnectService {
    /// Builds the service. A missing OAuth client is not an error here: ICS sources keep working
    /// and Google operations fail with [`ConnectError::NotConfigured`].
    pub fn new(
        cfg: ConnectConfig,
        store: Arc<dyn TokenStore>,
        clock: Arc<dyn Clock>,
    ) -> ConnectResult<Self> {
        let google = match GoogleConfig::resolve(cfg.client_override.as_ref()) {
            Ok(mut g) => {
                if let Some(endpoints) = &cfg.endpoints {
                    g.endpoints = endpoints.clone();
                }
                Some(g)
            }
            Err(ConnectError::NotConfigured) => None,
            Err(e) => return Err(e),
        };
        let calendar = google
            .as_ref()
            .map(|g| CalendarClient::new(g.endpoints.calendar.clone()))
            .transpose()?;
        let (events, _) = broadcast::channel(64);
        let accounts = Arc::new(AccountManager::new(
            &cfg.accounts_path,
            store,
            google,
            clock.clone(),
            events.clone(),
        )?);
        let mut scheduler = Scheduler::new(cfg.reminder.clone());
        for a in accounts.list() {
            scheduler.set_policy(&a.id, a.policy);
        }
        Ok(Self {
            inner: Arc::new(Inner {
                cfg,
                accounts,
                calendar,
                ics: IcsClient::new()?,
                clock,
                events,
                state: Mutex::new(State {
                    scheduler,
                    google_sync: HashMap::new(),
                    per_account: HashMap::new(),
                    recording: false,
                }),
                wake: Notify::new(),
                refresh_lock: tokio::sync::Mutex::new(()),
            }),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn publish(&self, event: ConnectEvent) {
        let _ = self.inner.events.send(event);
    }

    /// Subscribes to [`ConnectEvent`]s.
    pub fn subscribe(&self) -> broadcast::Receiver<ConnectEvent> {
        self.inner.events.subscribe()
    }

    /// The account registry.
    pub fn accounts(&self) -> &Arc<AccountManager> {
        &self.inner.accounts
    }

    /// Whether Google sign-in is available (an OAuth client is configured).
    pub fn google_configured(&self) -> bool {
        self.inner.accounts.is_configured()
    }

    /// Browser sign-in for a new Google account, then an immediate refresh.
    pub async fn add_google_account<F>(
        &self,
        open_url: F,
        cancel: &CancellationToken,
    ) -> ConnectResult<AccountMeta>
    where
        F: Fn(&str) + Send + Sync,
    {
        let meta = self
            .inner
            .accounts
            .add_google_account(open_url, cancel, DEFAULT_LOGIN_TIMEOUT)
            .await?;
        self.state().scheduler.set_policy(&meta.id, meta.policy);
        self.wake();
        Ok(meta)
    }

    /// Re-authenticates an expired account.
    pub async fn reauthenticate<F>(
        &self,
        email: &str,
        open_url: F,
        cancel: &CancellationToken,
    ) -> ConnectResult<AccountMeta>
    where
        F: Fn(&str) + Send + Sync,
    {
        let meta = self
            .inner
            .accounts
            .reauthenticate(email, open_url, cancel, DEFAULT_LOGIN_TIMEOUT)
            .await?;
        self.wake();
        Ok(meta)
    }

    /// Signs an account out (revokes the Google token) and drops its events.
    pub async fn remove_account(&self, id: &str) -> ConnectResult<RemoveOutcome> {
        let outcome = self.inner.accounts.remove_account(id).await?;
        {
            let mut state = self.state();
            state.google_sync.remove(id);
            state.per_account.remove(id);
        }
        self.rebuild_agenda();
        Ok(outcome)
    }

    /// Subscribes to an ICS URL after checking that it downloads and parses.
    pub async fn add_ics(&self, url: &str, label: &str) -> ConnectResult<AccountMeta> {
        let url = crate::google::accounts::normalize_ics_url(url)?;
        let (from, to) = self.window();
        self.inner
            .ics
            .fetch_agenda(&url, "ics:probe", (from, to))
            .await?;
        let meta = self.inner.accounts.add_ics(&url, label)?;
        self.wake();
        Ok(meta)
    }

    /// Sets an account's policy (Ask / Always / Never).
    pub fn set_policy(&self, id: &str, policy: AccountPolicy) -> ConnectResult<AccountMeta> {
        let meta = self.inner.accounts.set_policy(id, policy)?;
        self.state().scheduler.set_policy(&meta.id, policy);
        Ok(meta)
    }

    /// Shows/hides an account's events in the agenda.
    pub fn set_visible(&self, id: &str, visible: bool) -> ConnectResult<AccountMeta> {
        let meta = self.inner.accounts.set_visible(id, visible)?;
        self.rebuild_agenda();
        Ok(meta)
    }

    /// Per-event override ("record" / "don't record"); `None` clears it.
    pub fn set_override(&self, event_id: &str, decision: Option<EventOverride>) {
        self.state().scheduler.set_override(event_id, decision);
    }

    /// Changes the reminder lead/behaviour.
    pub fn set_reminder_settings(&self, settings: ReminderSettings) {
        self.state().scheduler.set_settings(settings);
    }

    /// Tells the scheduler the app is recording `event_id` until `until` (pauses detection too).
    pub fn note_recording(&self, event_id: &str, until: DateTime<Utc>) {
        self.state().scheduler.note_recording(event_id, until);
    }

    /// While the app itself records, the microphone is in use by definition: pause detection.
    pub fn set_recording(&self, recording: bool) {
        self.state().recording = recording;
    }

    /// The merged agenda as of the last refresh.
    pub fn agenda(&self) -> Vec<AgendaEvent> {
        self.state().scheduler.agenda().to_vec()
    }

    /// Asks the background loop to refresh now (call on wake from sleep / network change).
    pub fn wake(&self) {
        self.inner.wake.notify_one();
    }

    fn window(&self) -> (DateTime<Utc>, DateTime<Utc>) {
        let now = self.inner.clock.now();
        (
            now - chrono::Duration::days(self.inner.cfg.days_back),
            now + chrono::Duration::days(self.inner.cfg.days_ahead),
        )
    }

    /// Fetches every source, merges, hands the result to the scheduler and publishes
    /// `AgendaChanged` if it differs. Sources that fail keep their previous events.
    pub async fn refresh(&self) -> RefreshReport {
        self.refresh_window(self.window()).await
    }

    /// [`refresh`](Self::refresh) over an explicit window.
    pub async fn refresh_window(&self, window: (DateTime<Utc>, DateTime<Utc>)) -> RefreshReport {
        let _serial = self.inner.refresh_lock.lock().await;
        let now = self.inner.clock.now();
        let mut errors = Vec::new();
        for meta in self.inner.accounts.list() {
            if meta.state == AccountState::AuthExpired {
                continue;
            }
            let result = match meta.kind {
                AccountKind::Google => self.refresh_google(&meta.id, window, now).await,
                AccountKind::Ics => self.refresh_ics(&meta.id, window).await,
            };
            match result {
                Ok(events) => {
                    self.state().per_account.insert(meta.id.clone(), events);
                }
                Err(e) => {
                    tracing::warn!("refresh of {} failed: {e}", meta.id);
                    errors.push((meta.id.clone(), e));
                }
            }
        }
        let agenda = self.rebuild_agenda();
        RefreshReport { agenda, errors }
    }

    async fn refresh_google(
        &self,
        id: &str,
        window: (DateTime<Utc>, DateTime<Utc>),
        now: DateTime<Utc>,
    ) -> ConnectResult<Vec<AgendaEvent>> {
        let client = self
            .inner
            .calendar
            .as_ref()
            .ok_or(ConnectError::NotConfigured)?;
        let mut sync = self.state().google_sync.remove(id).unwrap_or_default();
        let result = client
            .fetch_agenda(&self.inner.accounts, id, &mut sync, window, now)
            .await;
        self.state().google_sync.insert(id.to_string(), sync);
        result
    }

    async fn refresh_ics(
        &self,
        id: &str,
        window: (DateTime<Utc>, DateTime<Utc>),
    ) -> ConnectResult<Vec<AgendaEvent>> {
        let url = self.inner.accounts.ics_url(id)?;
        self.inner.ics.fetch_agenda(&url, id, window).await
    }

    /// Merges the cached per-account events (visible accounts only) into the scheduler.
    fn rebuild_agenda(&self) -> Vec<AgendaEvent> {
        let metas = self.inner.accounts.list();
        let mut state = self.state();
        let sources: Vec<Vec<AgendaEvent>> = metas
            .iter()
            .filter(|m| m.visible)
            .filter_map(|m| state.per_account.get(&m.id).cloned())
            .collect();
        for m in &metas {
            state.scheduler.set_policy(&m.id, m.policy);
        }
        let merged = merge_agenda(sources);
        let changed = state.scheduler.set_agenda(merged.clone());
        drop(state);
        if let Some(event) = changed {
            self.publish(event);
        }
        merged
    }

    /// Runs the scheduler for the current instant and publishes what is due.
    pub fn tick(&self) -> Vec<ConnectEvent> {
        let due = self.state().scheduler.poll(self.inner.clock.now());
        for event in &due {
            self.publish(event.clone());
        }
        due
    }

    /// Feeds one detection result (or `None` for "no call") through the scheduler's suppression.
    pub fn report_detection(&self, call: Option<DetectedCall>) -> Option<ConnectEvent> {
        let now = self.inner.clock.now();
        let event = {
            let mut state = self.state();
            match call {
                Some(call) => state.scheduler.on_detected(call, now),
                None => {
                    state.scheduler.on_detection_cleared();
                    None
                }
            }
        };
        if let Some(e) = &event {
            self.publish(e.clone());
        }
        event
    }

    /// Starts the background loops (refresh + tick). Cancel the token to stop them.
    pub fn start(&self, cancel: CancellationToken) -> tokio::task::JoinHandle<()> {
        let refresher = {
            let svc = self.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(svc.inner.cfg.refresh_interval);
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        _ = interval.tick() => {}
                        () = svc.inner.wake.notified() => {}
                    }
                    svc.refresh().await;
                }
            })
        };
        let ticker = {
            let svc = self.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(svc.inner.cfg.tick_interval);
                loop {
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        _ = interval.tick() => { svc.tick(); }
                    }
                }
            })
        };
        tokio::spawn(async move {
            let _ = refresher.await;
            let _ = ticker.await;
        })
    }

    /// Runs call detection every `interval` until cancelled (the blocking OS probes run on the
    /// blocking pool). Skipped while the app itself is recording.
    pub fn start_detection<S>(
        &self,
        detector: Detector<S>,
        interval: Duration,
        cancel: CancellationToken,
    ) -> tokio::task::JoinHandle<()>
    where
        S: CallSources + 'static,
    {
        self.start_detection_with_status(detector, interval, cancel, |_| {})
    }

    /// Starts call detection and reports changes in the browser Automation permission state.
    pub fn start_detection_with_status<S, F>(
        &self,
        detector: Detector<S>,
        interval: Duration,
        cancel: CancellationToken,
        status_changed: F,
    ) -> tokio::task::JoinHandle<()>
    where
        S: CallSources + 'static,
        F: Fn(Option<String>) + Send + Sync + 'static,
    {
        let svc = self.clone();
        let detector = Arc::new(Mutex::new(detector));
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(interval);
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut last_call: Option<DetectedCall> = None;
            let mut last_permission: Option<String> = None;
            loop {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    _ = timer.tick() => {}
                }
                if svc.state().recording {
                    continue;
                }
                let detector = detector.clone();
                let scan =
                    tokio::task::spawn_blocking(move || detector.lock().ok().map(|d| d.scan()))
                        .await
                        .ok()
                        .flatten();
                let Some(scan) = scan else {
                    continue;
                };
                let permission = scan
                    .call
                    .as_ref()
                    .and_then(|call| call.automation_permission_required.clone());
                if permission != last_permission {
                    status_changed(permission.clone());
                    last_permission = permission;
                }
                if scan.call != last_call {
                    if let Some(call) = &scan.call {
                        tracing::info!(
                            app = ?call.app,
                            browser = ?call.browser,
                            "microphone call state detected"
                        );
                    } else if last_call.is_some() {
                        tracing::info!("microphone call state cleared");
                    }
                    last_call.clone_from(&scan.call);
                }
                svc.report_detection(scan.call);
            }
        })
    }
}
