//! Error type of this crate and its mapping into [`yc_core::Error`].

/// Errors from account, calendar and detection code.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// No Google OAuth client ID was compiled in or configured at runtime.
    #[error(
        "Google sign-in is not configured: set YADACALL_GOOGLE_CLIENT_ID (see docs/GOOGLE_OAUTH.md)"
    )]
    NotConfigured,
    /// The refresh token was revoked or expired (`invalid_grant`); the user must sign in again.
    #[error("Google sign-in for {0} expired; sign in again")]
    AuthExpired(String),
    /// The user closed or denied the consent screen.
    #[error("sign-in was denied: {0}")]
    Denied(String),
    /// The sign-in did not complete in time.
    #[error("sign-in timed out")]
    Timeout,
    /// The operation was cancelled by the caller.
    #[error("cancelled")]
    Cancelled,
    /// Transport or unexpected HTTP failure.
    #[error("network error: {0}")]
    Network(String),
    /// Google or an ICS server answered with something we cannot use.
    #[error("unexpected response: {0}")]
    Protocol(String),
    /// Keychain access failed.
    #[error("keychain error: {0}")]
    Keychain(String),
    /// Account metadata could not be read or written.
    #[error("storage error: {0}")]
    Storage(String),
    /// Unknown account, calendar or source.
    #[error("not found: {0}")]
    NotFound(String),
    /// Bad caller input (malformed URL, empty email, ...).
    #[error("invalid input: {0}")]
    Invalid(String),
}

/// Result alias for this crate.
pub type ConnectResult<T> = Result<T, ConnectError>;

impl ConnectError {
    /// True when the user has to go through the browser sign-in again.
    pub fn needs_reauth(&self) -> bool {
        matches!(self, ConnectError::AuthExpired(_))
    }
}

impl From<ConnectError> for yc_core::Error {
    fn from(error: ConnectError) -> Self {
        match error {
            ConnectError::Cancelled => yc_core::Error::Cancelled,
            ConnectError::Network(m) | ConnectError::Protocol(m) => yc_core::Error::Network(m),
            ConnectError::Timeout => yc_core::Error::Network("sign-in timed out".into()),
            ConnectError::Keychain(m) | ConnectError::Storage(m) => yc_core::Error::Storage(m),
            ConnectError::NotFound(m) => yc_core::Error::NotFound(m),
            other => yc_core::Error::Invalid(other.to_string()),
        }
    }
}

impl From<reqwest::Error> for ConnectError {
    fn from(error: reqwest::Error) -> Self {
        // `without_url` keeps one-time codes and calendar ids out of logs.
        ConnectError::Network(error.without_url().to_string())
    }
}

impl From<std::io::Error> for ConnectError {
    fn from(error: std::io::Error) -> Self {
        ConnectError::Storage(error.to_string())
    }
}
