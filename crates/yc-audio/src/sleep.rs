//! Keep the machine awake while a call is being recorded.

use yc_core::{Error, Result};

/// Holds an "idle sleep is not allowed" assertion until dropped.
pub struct SleepGuard {
    _inner: keepawake::KeepAwake,
}

/// Prevents idle system sleep (the display may still turn off) until the guard is dropped.
pub fn prevent_sleep(reason: &str) -> Result<SleepGuard> {
    let inner = keepawake::Builder::default()
        .idle(true)
        .reason(reason)
        .app_name("YadaCall")
        .app_reverse_domain("app.yadacall")
        .create()
        .map_err(|e| Error::Device(format!("cannot prevent sleep: {e}")))?;
    Ok(SleepGuard { _inner: inner })
}
