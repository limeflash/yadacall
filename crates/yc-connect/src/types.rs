//! Data types shared by the agenda, scheduler and detection modules (all serde-friendly so the
//! desktop shell can forward them to the UI unchanged).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Video-call provider of a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VideoProvider {
    Meet,
    Zoom,
    Teams,
    Telemost,
    Jitsi,
    Other,
}

/// A video-call link found in an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoLink {
    pub provider: VideoProvider,
    pub url: String,
}

/// Person attached to an event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attendee {
    pub name: Option<String>,
    pub email: Option<String>,
    /// This attendee is the account owner.
    pub is_self: bool,
    /// `accepted`, `declined`, `tentative`, `needsAction`.
    pub response: Option<String>,
}

/// One upcoming call/meeting from any source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgendaEvent {
    /// Globally unique and stable: `<account>|<calendar>|<source event id>`.
    pub id: String,
    /// Account id (Google e-mail or `ics:<hash>`).
    pub account: String,
    pub calendar: String,
    pub calendar_title: Option<String>,
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// Google Meet URL (from conference data, `hangoutLink` or the description).
    pub meet_url: Option<String>,
    /// Zoom / Teams / Telemost / other links found in conference data, location or description.
    pub other_links: Vec<VideoLink>,
    pub attendees: Vec<Attendee>,
    pub organizer: Option<Attendee>,
    /// iCalendar UID, used to collapse one meeting seen through several accounts.
    pub ical_uid: Option<String>,
    /// Ids of events whose time range overlaps this one (filled by [`crate::merge_agenda`]).
    #[serde(default)]
    pub overlaps: Vec<String>,
    /// Other accounts that carry the same meeting (filled by [`crate::merge_agenda`]).
    #[serde(default)]
    pub also_in: Vec<String>,
}

impl AgendaEvent {
    /// Whether the event has any video-call link.
    pub fn has_call_link(&self) -> bool {
        self.meet_url.is_some() || !self.other_links.is_empty()
    }
}

/// What to do when an account's meeting is about to start.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AccountPolicy {
    /// Show a reminder with "Join & record" / "Skip" actions.
    #[default]
    Ask,
    /// Start recording automatically at the start time.
    Always,
    /// Never remind or record.
    Never,
}

/// Where an account's data comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AccountKind {
    Google,
    Ics,
}

/// Health of an account.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AccountState {
    #[default]
    Ok,
    /// Refresh token rejected: show "вход истёк · Войти снова".
    AuthExpired,
}

/// Persisted per-account metadata (never contains secrets).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountMeta {
    /// Google e-mail, or `ics:<hash>` for an ICS subscription.
    pub id: String,
    pub kind: AccountKind,
    /// Empty for ICS sources.
    pub email: String,
    pub name: Option<String>,
    pub picture: Option<String>,
    /// Google Workspace domain (`hd` claim); drives the "Workspace" badge.
    pub hd: Option<String>,
    pub label: String,
    pub policy: AccountPolicy,
    pub visible: bool,
    pub color: String,
    #[serde(default)]
    pub state: AccountState,
}

impl AccountMeta {
    /// True for Google Workspace accounts.
    pub fn is_workspace(&self) -> bool {
        self.hd.is_some()
    }
}

/// A call that is in progress but not (necessarily) in the calendar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedCall {
    pub app: DetectedApp,
    /// `abc-defg-hij` for Google Meet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meet_code: Option<String>,
    /// Provider-specific room code or identifier, when it can be read from the call URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The call page URL, when browser access exposed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Browser the Meet tab was found in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<String>,
    /// Page title, or a generic title such as `Звонок в Brave`.
    #[serde(default)]
    pub title: String,
    /// Browser requiring Automation permission, when macOS denied Apple Events access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automation_permission_required: Option<String>,
    /// Calendar event this call corresponds to, when one matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_event_id: Option<String>,
    /// `true` when a Meet tab/window was actually seen; `false` when only a call app plus
    /// "microphone is in use" was observed.
    pub certain: bool,
}

/// Application a detected call runs in.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DetectedApp {
    GoogleMeet,
    Zoom,
    Teams,
    Telegram,
    Slack,
    Discord,
    Telemost,
    Jitsi,
    Other(String),
}

/// Events the connect layer publishes (forwarded to the UI by the shell).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum ConnectEvent {
    /// `lead_minutes` before the start: show the reminder with join/record actions.
    ReminderDue {
        event: AgendaEvent,
        lead_minutes: u32,
    },
    /// The meeting is starting now (or the user opened the app late).
    CallStarting { event: AgendaEvent },
    /// `Always` policy: start recording now.
    AutoRecord { event: AgendaEvent },
    /// The account needs a fresh sign-in.
    AccountAuthExpired { account: String },
    /// The merged agenda changed.
    AgendaChanged,
    /// A call was noticed that no reminder has covered.
    CallDetected { call: DetectedCall },
}
