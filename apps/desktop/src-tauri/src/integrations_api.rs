//! Integrations boundary of the desktop shell: Google Calendar accounts, the call agenda, the connections
//! catalog and the "before the call" reminders.
//!
//! Kept apart from [`crate::api::Backend`] on purpose: the real `yc-connect` crate implements [`Integrations`]
//! while the recording engine implements `Backend`; both trait objects live side by side in Tauri state.
//! DTOs are camelCase and mirrored by `ui/src/lib/api/types.ts`. [`crate::integrations_demo`] is the in-memory
//! implementation used until the real one lands.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub type Result<T, E = yc_core::Error> = std::result::Result<T, E>;

/// Pushes [`IntegrationEvent`]s to the shell and to every webview (the global `integrations` event).
pub type IntegrationSink = Arc<dyn Fn(IntegrationEvent) + Send + Sync>;

// ---- accounts -----------------------------------------------------------------------------------------

/// Whether YadaCall may start recording an account's calls on its own.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RecordPolicy {
    /// Offer a recording for every call, the user decides per event (default).
    #[default]
    Ask,
    /// Every call of the account is recorded.
    Always,
    /// Calls of the account are never recorded.
    Never,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AccountStatus {
    #[default]
    Ok,
    /// Google asked to sign in again; the account's events are hidden until then.
    Expired,
    /// Waiting for the browser (adding or re-authorising).
    Connecting,
}

/// One connected Google account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarAccount {
    pub id: String,
    /// "Рабочий", "Личный" — user-editable name.
    pub label: String,
    pub email: String,
    /// Avatar colour, `#rrggbb`.
    pub color: String,
    /// Google Workspace (company) account.
    #[serde(default)]
    pub workspace: bool,
    pub status: AccountStatus,
    /// Calls on the calendar for the current week.
    pub meetings_this_week: u32,
    pub record_policy: RecordPolicy,
    /// "В календаре": show the account's events in the agenda.
    pub visible: bool,
}

// ---- agenda -------------------------------------------------------------------------------------------

/// Half-open time range `[from, to)` an agenda is requested for.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeRange {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

/// One call from a connected calendar.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgendaEvent {
    pub id: String,
    pub account_id: String,
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// Google Meet link, if the event has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meet_url: Option<String>,
    /// Display names of the other participants (without the user).
    #[serde(default)]
    pub attendees: Vec<String>,
    /// Policy of the owning account.
    pub record_policy: RecordPolicy,
    /// Result of policy + the user's per-event choice.
    pub will_record: bool,
    /// Ids of visible events of *other* accounts that run at the same time.
    #[serde(default)]
    pub overlaps: Vec<String>,
    /// Library meeting that was recorded for this event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting_id: Option<String>,
}

/// What the shell must do after the user chose to join a call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinOutcome {
    pub title: String,
    /// Link to open in the browser (already pinned to the right Google account). `None`: nothing to open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meet_url: Option<String>,
    /// Address of the account the call belongs to, for messages like "Открыл Meet в lime@company.com".
    pub account_email: String,
    /// Whether the recorder should start.
    pub record: bool,
    /// When the call is expected to end (for the "звонок без записи" menu-bar state).
    pub ends_at: DateTime<Utc>,
    /// Calendar event the call belongs to (links the recorded meeting back to the agenda).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    /// How many remote people to expect; `None` when the attendee list is unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_speakers: Option<u8>,
    /// The other attendees (name and email), passed to the engine to name the speakers.
    #[serde(default)]
    pub participants: Vec<yc_core::meeting::ParticipantHint>,
}

// ---- connections catalog ------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionState {
    Connected,
    Soon,
    /// The service exists but this build has no credentials for it (see `Connection::docs_url`).
    Unconfigured,
}

/// One service card of the catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub id: String,
    pub name: String,
    /// One letter for the tile.
    pub mono: String,
    pub description: String,
    pub state: ConnectionState,
    /// Number of working accounts, for connected services.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accounts: Option<u32>,
    /// "Хочу такое": a purely local flag, never sent anywhere.
    #[serde(default)]
    pub want: bool,
    /// Setup instructions for an [`ConnectionState::Unconfigured`] service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
}

// ---- reminders ----------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct ReminderSettings {
    /// How many minutes before the start to remind: 1, 2 or 5.
    pub remind_minutes: u8,
    /// Open Meet under the account the event belongs to (no "switch account" dance).
    pub open_in_account: bool,
    /// Notice calls that are not on any calendar and offer to record them.
    pub detect_unscheduled: bool,
    /// The user pressed "Разрешить": detection may read the browsers' tabs (macOS asks for Automation
    /// access the first time). Off: mic-in-use plus window titles and running processes only.
    pub browser_access: bool,
}

impl Default for ReminderSettings {
    fn default() -> Self {
        Self {
            remind_minutes: 2,
            open_in_account: true,
            detect_unscheduled: true,
            browser_access: false,
        }
    }
}

/// A Google Meet call the user is in that no calendar knows about.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedCall {
    pub id: String,
    pub platform: String,
    /// `meet.google.com/kqv-tdre-xmn`
    pub url: String,
    pub detected_at: DateTime<Utc>,
}

// ---- events -------------------------------------------------------------------------------------------

/// Pushed through the `integrations` Tauri event (tag = `type`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum IntegrationEvent {
    /// A call starts in `minutes_before` minutes.
    #[serde(rename_all = "camelCase")]
    ReminderDue {
        event: AgendaEvent,
        account_label: String,
        account_email: String,
        minutes_before: u8,
    },
    /// A calendar call is starting right now (or the app was opened late) and nothing has been decided.
    #[serde(rename_all = "camelCase")]
    CallStarting {
        event: AgendaEvent,
        account_label: String,
        account_email: String,
    },
    /// The user joined a Meet call that is not on any calendar.
    CallDetected { call: DetectedCall },
    /// The sign-in of an account expired.
    #[serde(rename_all = "camelCase")]
    AuthExpired { account_id: String, email: String },
    /// Progress of "add account": step 1..=3 (pick account, allow access, fetch events).
    AddProgress { step: u8 },
    /// Accounts, policies or events changed: refetch.
    AgendaChanged,
}

// ---- the trait ----------------------------------------------------------------------------------------

/// Everything the UI and the shell can ask of the calendar integrations.
#[async_trait]
pub trait Integrations: Send + Sync {
    // accounts
    async fn accounts(&self) -> Result<Vec<CalendarAccount>>;
    /// Starts the OAuth flow in the browser and resolves with the new account once access is granted.
    /// Reports progress as [`IntegrationEvent::AddProgress`]. Resolves with `Error::Cancelled` after
    /// [`Integrations::cancel_add_account`].
    async fn add_google_account(&self) -> Result<CalendarAccount>;
    async fn cancel_add_account(&self) -> Result<()>;
    async fn remove_account(&self, id: &str) -> Result<()>;
    /// Signs an expired account in again; resolves when the browser round-trip finished.
    async fn reauth_account(&self, id: &str) -> Result<CalendarAccount>;
    async fn set_account_policy(&self, id: &str, policy: RecordPolicy) -> Result<()>;
    async fn set_account_visible(&self, id: &str, visible: bool) -> Result<()>;

    // agenda
    /// Events of every visible, working account that intersect `range`, ordered by start.
    async fn agenda(&self, range: TimeRange) -> Result<Vec<AgendaEvent>>;
    async fn set_event_recording(&self, event_id: &str, record: bool) -> Result<()>;
    /// "Войти и записать": marks the event as recorded and returns where to go.
    async fn join_and_record(&self, event_id: &str) -> Result<JoinOutcome>;
    /// "Без записи": opens the call only.
    async fn join_without_recording(&self, event_id: &str) -> Result<JoinOutcome>;

    // catalog
    async fn connections(&self) -> Result<Vec<Connection>>;
    /// Flips "Хочу такое" for a service that is not available yet; returns the new value.
    async fn toggle_want(&self, service: &str) -> Result<bool>;

    // before the call
    async fn reminder_settings(&self) -> Result<ReminderSettings>;
    async fn set_reminder_settings(&self, s: ReminderSettings) -> Result<ReminderSettings>;
    /// The unscheduled call that is waiting for a decision, if any.
    async fn detected_call(&self) -> Result<Option<DetectedCall>>;
    /// Answers the "Записать его?" notification. `record == false` is "Не сейчас".
    async fn resolve_detected_call(&self, id: &str, record: bool) -> Result<Option<JoinOutcome>>;
}
