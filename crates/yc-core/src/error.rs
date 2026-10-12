use std::path::PathBuf;

/// Errors that cross crate boundaries. Adapters map their library errors into these variants so
/// the engine and UI can react (retry, ask for permission, offer a download) without knowing which
/// library failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("permission denied: {0}")]
    Permission(PermissionKind),
    #[error("audio device error: {0}")]
    Device(String),
    #[error("model `{0}` is not installed")]
    ModelMissing(String),
    #[error("model `{model}` failed: {message}")]
    Model { model: String, message: String },
    #[error("not enough disk space: need {needed} bytes, have {available}")]
    DiskFull { needed: u64, available: u64 },
    #[error("network error: {0}")]
    Network(String),
    #[error("integrity check failed for {0}")]
    Integrity(PathBuf),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("llm provider error: {0}")]
    Llm(String),
    #[error("cancelled")]
    Cancelled,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    /// Whether retrying the same operation later can reasonably succeed.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Error::Network(_) | Error::Device(_) | Error::Io(_) | Error::Llm(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionKind {
    Microphone,
    SystemAudio,
}

impl std::fmt::Display for PermissionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PermissionKind::Microphone => "microphone",
            PermissionKind::SystemAudio => "system audio",
        })
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
