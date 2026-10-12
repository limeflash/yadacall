//! OS keychain storage for provider credentials. Secret values are never logged.

use yc_core::{Error, Result};

/// Keychain service shared by the desktop app and CLI.
pub const KEYCHAIN_SERVICE: &str = "app.yadacall.desktop";

/// Read a provider credential, returning `None` when no entry exists.
pub fn get(provider_id: &str) -> Result<Option<String>> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, provider_id)
        .map_err(|_| Error::Llm("could not access the OS keychain".to_owned()))?;
    match entry.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err(Error::Llm(
            "could not read a provider credential from the OS keychain".to_owned(),
        )),
    }
}

/// Store or replace a provider credential in the OS keychain.
pub fn set(provider_id: &str, secret: &str) -> Result<()> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, provider_id)
        .map_err(|_| Error::Llm("could not access the OS keychain".to_owned()))?;
    entry.set_password(secret).map_err(|_| {
        Error::Llm("could not store a provider credential in the OS keychain".to_owned())
    })
}

/// Delete a provider credential. Missing entries are already deleted.
pub fn delete(provider_id: &str) -> Result<()> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, provider_id)
        .map_err(|_| Error::Llm("could not access the OS keychain".to_owned()))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(Error::Llm(
            "could not delete a provider credential from the OS keychain".to_owned(),
        )),
    }
}
