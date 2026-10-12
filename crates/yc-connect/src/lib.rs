//! Google accounts, calendar agenda, pre-call scheduler and call detection.
//!
//! Everything here is local-first: refresh tokens live in the OS keychain, calendar data is only
//! fetched from Google (or from an ICS URL the user pasted) and never sent anywhere else.
//!
//! Module map:
//! - [`google`]: OAuth 2.0 installed-app flow, token store, account manager, Calendar API client.
//! - [`ics`]: zero-OAuth fallback source (secret ICS URL, RRULE expansion).
//! - [`agenda`]: merging sources, overlap marking, join URLs.
//! - [`scheduler`]: pure, clock-injected reminder/auto-record logic.
//! - [`detect`]: call detection for calls that are not in the calendar.
//! - [`service`]: the async runner that glues refresh, scheduling and events together.

pub mod agenda;
pub mod clock;
pub mod detect;
pub mod error;
pub mod google;
pub mod ics;
pub mod links;
pub mod scheduler;
pub mod service;
#[cfg(test)]
mod testutil;
pub mod types;

pub use agenda::{join_url, merge_agenda};
pub use clock::{Clock, ManualClock, SystemClock};
pub use error::{ConnectError, ConnectResult};
pub use scheduler::{EventOverride, ReminderSettings, Scheduler};
pub use service::{ConnectConfig, ConnectService};
pub use types::{
    AccountKind, AccountMeta, AccountPolicy, AccountState, AgendaEvent, Attendee, ConnectEvent,
    DetectedCall, VideoLink, VideoProvider,
};
