//! Secret storage: refresh tokens (and ICS secret URLs) live in the OS keychain.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::{ConnectError, ConnectResult};

/// Keychain service name shared with the rest of the app.
pub const KEYCHAIN_SERVICE: &str = "app.yadacall.desktop";

/// Keychain account name of a Google refresh token.
pub fn google_key(email: &str) -> String {
    format!("google:{}", email.to_ascii_lowercase())
}

/// A key/value store for small secrets.
pub trait TokenStore: Send + Sync {
    /// Reads a secret; `None` if absent.
    fn get(&self, key: &str) -> ConnectResult<Option<String>>;
    /// Stores or replaces a secret.
    fn set(&self, key: &str, value: &str) -> ConnectResult<()>;
    /// Removes a secret; absent is not an error.
    fn delete(&self, key: &str) -> ConnectResult<()>;
}

/// In-memory store for tests and for platforms without a keychain backend.
#[derive(Debug, Default)]
pub struct MemoryTokenStore(Mutex<HashMap<String, String>>);

impl TokenStore for MemoryTokenStore {
    fn get(&self, key: &str) -> ConnectResult<Option<String>> {
        Ok(self.lock()?.get(key).cloned())
    }
    fn set(&self, key: &str, value: &str) -> ConnectResult<()> {
        self.lock()?.insert(key.into(), value.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> ConnectResult<()> {
        self.lock()?.remove(key);
        Ok(())
    }
}

impl MemoryTokenStore {
    fn lock(&self) -> ConnectResult<std::sync::MutexGuard<'_, HashMap<String, String>>> {
        self.0
            .lock()
            .map_err(|_| ConnectError::Keychain("token store poisoned".into()))
    }
}

/// The OS keychain (macOS Keychain, Windows Credential Manager).
#[derive(Debug, Default, Clone, Copy)]
pub struct KeychainTokenStore;

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl KeychainTokenStore {
    fn entry(key: &str) -> ConnectResult<keyring::Entry> {
        keyring::Entry::new(KEYCHAIN_SERVICE, key)
            .map_err(|e| ConnectError::Keychain(e.to_string()))
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl TokenStore for KeychainTokenStore {
    fn get(&self, key: &str) -> ConnectResult<Option<String>> {
        match Self::entry(key)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(ConnectError::Keychain(e.to_string())),
        }
    }
    fn set(&self, key: &str, value: &str) -> ConnectResult<()> {
        Self::entry(key)?
            .set_password(value)
            .map_err(|e| ConnectError::Keychain(e.to_string()))
    }
    fn delete(&self, key: &str) -> ConnectResult<()> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(ConnectError::Keychain(e.to_string())),
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl TokenStore for KeychainTokenStore {
    fn get(&self, _key: &str) -> ConnectResult<Option<String>> {
        Err(ConnectError::Keychain(
            "no OS keychain backend on this platform".into(),
        ))
    }
    fn set(&self, _key: &str, _value: &str) -> ConnectResult<()> {
        self.get("").map(|_| ())
    }
    fn delete(&self, _key: &str) -> ConnectResult<()> {
        self.get("").map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_roundtrip() {
        let s = MemoryTokenStore::default();
        assert_eq!(s.get("a").unwrap(), None);
        s.set("a", "1").unwrap();
        assert_eq!(s.get("a").unwrap().as_deref(), Some("1"));
        s.delete("a").unwrap();
        s.delete("a").unwrap();
        assert_eq!(s.get("a").unwrap(), None);
    }

    #[test]
    fn google_key_is_lowercased() {
        assert_eq!(google_key("Foo@Bar.com"), "google:foo@bar.com");
    }
}
