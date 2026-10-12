//! OAuth client configuration and endpoint URLs.

use serde::{Deserialize, Serialize};

use crate::error::{ConnectError, ConnectResult};

/// Scopes requested at sign-in: basic identity, read-only events, and the read-only list of the
/// user's calendars (so shared/team/subscribed calendars show up, not just the primary one).
pub const SCOPES: &str = "openid email profile \
    https://www.googleapis.com/auth/calendar.events.readonly \
    https://www.googleapis.com/auth/calendar.calendarlist.readonly";

/// The scope that must be present in the grant (users can untick it on the consent screen).
pub const CALENDAR_SCOPE: &str = "https://www.googleapis.com/auth/calendar.events.readonly";

/// Client ID baked in at build time (`YADACALL_GOOGLE_CLIENT_ID`), if any.
pub const COMPILED_CLIENT_ID: Option<&str> = option_env!("YADACALL_GOOGLE_CLIENT_ID");
/// Client secret baked in at build time (`YADACALL_GOOGLE_CLIENT_SECRET`), if any. Desktop-app
/// secrets are not confidential (Google's installed-app docs say so), so embedding is fine.
pub const COMPILED_CLIENT_SECRET: Option<&str> = option_env!("YADACALL_GOOGLE_CLIENT_SECRET");

/// A client ID/secret pair, e.g. entered in settings to override the built-in one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCredentials {
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
}

/// URLs of the Google endpoints; overridable so tests can point at a local mock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub auth: String,
    pub token: String,
    pub revoke: String,
    pub userinfo: String,
    /// Calendar API root, without trailing slash.
    pub calendar: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            auth: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token: "https://oauth2.googleapis.com/token".into(),
            revoke: "https://oauth2.googleapis.com/revoke".into(),
            userinfo: "https://openidconnect.googleapis.com/v1/userinfo".into(),
            calendar: "https://www.googleapis.com/calendar/v3".into(),
        }
    }
}

/// Everything needed to talk to Google as an installed app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleConfig {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub endpoints: Endpoints,
}

impl GoogleConfig {
    /// Resolves credentials: explicit override (settings), then the runtime environment, then the
    /// compile-time values. Returns [`ConnectError::NotConfigured`] when none is set.
    pub fn resolve(override_credentials: Option<&ClientCredentials>) -> ConnectResult<Self> {
        Self::resolve_from(
            override_credentials,
            (
                std::env::var("YADACALL_GOOGLE_CLIENT_ID").ok().as_deref(),
                std::env::var("YADACALL_GOOGLE_CLIENT_SECRET")
                    .ok()
                    .as_deref(),
            ),
            (COMPILED_CLIENT_ID, COMPILED_CLIENT_SECRET),
        )
    }

    /// [`resolve`](Self::resolve) with the environment and compile-time values passed in.
    pub fn resolve_from(
        override_credentials: Option<&ClientCredentials>,
        runtime: (Option<&str>, Option<&str>),
        compiled: (Option<&str>, Option<&str>),
    ) -> ConnectResult<Self> {
        let nonempty = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
        let pick = |(id, secret): (Option<&str>, Option<&str>)| {
            id.and_then(nonempty)
                .map(|id| (id, secret.and_then(nonempty)))
        };
        let (client_id, client_secret) = override_credentials
            .filter(|c| !c.client_id.trim().is_empty())
            .map(|c| {
                (
                    c.client_id.trim().to_string(),
                    c.client_secret.as_deref().and_then(nonempty),
                )
            })
            .or_else(|| pick(runtime))
            .or_else(|| pick(compiled))
            .ok_or(ConnectError::NotConfigured)?;
        Ok(Self {
            client_id,
            client_secret,
            endpoints: Endpoints::default(),
        })
    }

    /// A config with explicit values and custom endpoints (tests, self-hosted proxies).
    pub fn with_endpoints(
        client_id: impl Into<String>,
        client_secret: Option<String>,
        endpoints: Endpoints,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret,
            endpoints,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_wins_and_blank_override_is_ignored() {
        let c = ClientCredentials {
            client_id: " abc.apps.googleusercontent.com ".into(),
            client_secret: Some("s".into()),
        };
        let cfg = GoogleConfig::resolve_from(Some(&c), (None, None), (None, None)).unwrap();
        assert_eq!(cfg.client_id, "abc.apps.googleusercontent.com");
        assert_eq!(cfg.client_secret.as_deref(), Some("s"));
    }

    #[test]
    fn nothing_configured_is_a_clean_error() {
        let err =
            GoogleConfig::resolve_from(None, (None, Some("x")), (Some(" "), None)).unwrap_err();
        assert!(matches!(err, ConnectError::NotConfigured));
        assert!(err.to_string().contains("GOOGLE_OAUTH.md"));
    }

    #[test]
    fn runtime_env_beats_compiled() {
        let cfg =
            GoogleConfig::resolve_from(None, (Some("rt"), None), (Some("ct"), Some("s"))).unwrap();
        assert_eq!(cfg.client_id, "rt");
        assert_eq!(cfg.client_secret, None);
    }

    #[test]
    fn scopes_are_read_only() {
        assert!(SCOPES.contains("calendar.events.readonly"));
        assert!(!SCOPES.contains("auth/calendar ") && !SCOPES.ends_with("auth/calendar"));
    }
}
