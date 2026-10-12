//! Google integration: OAuth 2.0 for installed apps, token storage, accounts and the Calendar API.

pub mod accounts;
pub mod calendar;
pub mod config;
pub mod loopback;
pub mod oauth;
pub mod pkce;
pub mod tokens;

pub use accounts::{AccountManager, RemoveOutcome};
pub use calendar::{CalendarClient, CalendarInfo, CalendarSync};
pub use config::{ClientCredentials, Endpoints, GoogleConfig, SCOPES};
pub use tokens::{KeychainTokenStore, MemoryTokenStore, TokenStore};
