//! Account registry: metadata JSON on disk, secrets in the keychain, access-token cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::config::GoogleConfig;
use super::oauth::{Authorized, GoogleOAuth, TokenResponse};
use super::tokens::{TokenStore, google_key};
use crate::clock::Clock;
use crate::error::{ConnectError, ConnectResult};
use crate::types::{AccountKind, AccountMeta, AccountPolicy, AccountState, ConnectEvent};

const PALETTE: [&str; 6] = [
    "#4285F4", "#EA4335", "#34A853", "#F9AB00", "#A142F4", "#12B5CB",
];
/// Access tokens are refreshed this long before they expire.
const EXPIRY_SKEW: i64 = 60;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Persisted {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    accounts: Vec<AccountMeta>,
}

#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    expires_at: DateTime<Utc>,
}

/// What [`AccountManager::remove_account`] managed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoveOutcome {
    /// Google confirmed (or already considered) the token revoked.
    pub revoked: bool,
}

/// Owns the list of accounts and hands out valid access tokens.
pub struct AccountManager {
    oauth: Option<GoogleOAuth>,
    store: Arc<dyn TokenStore>,
    path: PathBuf,
    accounts: Mutex<Vec<AccountMeta>>,
    access: tokio::sync::Mutex<HashMap<String, CachedToken>>,
    clock: Arc<dyn Clock>,
    events: broadcast::Sender<ConnectEvent>,
}

impl std::fmt::Debug for AccountManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountManager")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl AccountManager {
    /// Loads (or starts) the registry at `path`. `config` is `None` when no OAuth client is
    /// configured; adding/refreshing Google accounts then fails with `NotConfigured`.
    pub fn new(
        path: impl Into<PathBuf>,
        store: Arc<dyn TokenStore>,
        config: Option<GoogleConfig>,
        clock: Arc<dyn Clock>,
        events: broadcast::Sender<ConnectEvent>,
    ) -> ConnectResult<Self> {
        let path = path.into();
        let accounts = match std::fs::read_to_string(&path) {
            Ok(text) => {
                serde_json::from_str::<Persisted>(&text)
                    .map_err(|e| ConnectError::Storage(format!("{}: {e}", path.display())))?
                    .accounts
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        let oauth = config.map(GoogleOAuth::new).transpose()?;
        Ok(Self {
            oauth,
            store,
            path,
            accounts: Mutex::new(accounts),
            access: tokio::sync::Mutex::new(HashMap::new()),
            clock,
            events,
        })
    }

    fn lock(&self) -> MutexGuard<'_, Vec<AccountMeta>> {
        self.accounts.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn oauth(&self) -> ConnectResult<&GoogleOAuth> {
        self.oauth.as_ref().ok_or(ConnectError::NotConfigured)
    }

    /// Whether a Google OAuth client is available.
    pub fn is_configured(&self) -> bool {
        self.oauth.is_some()
    }

    /// Path of the metadata file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// All accounts (Google and ICS), in the order they were added.
    pub fn list(&self) -> Vec<AccountMeta> {
        self.lock().clone()
    }

    /// One account by id.
    pub fn get(&self, id: &str) -> Option<AccountMeta> {
        let id = normalize_id(id);
        self.lock().iter().find(|a| a.id == id).cloned()
    }

    fn persist(&self, accounts: &[AccountMeta]) -> ConnectResult<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(&Persisted {
            version: 1,
            accounts: accounts.to_vec(),
        })
        .map_err(|e| ConnectError::Storage(e.to_string()))?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Applies `f` to an account's metadata and saves.
    pub fn update(&self, id: &str, f: impl FnOnce(&mut AccountMeta)) -> ConnectResult<AccountMeta> {
        let id = normalize_id(id);
        let mut accounts = self.lock();
        let meta = accounts
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or_else(|| ConnectError::NotFound(id.clone()))?;
        f(meta);
        let updated = meta.clone();
        self.persist(&accounts)?;
        Ok(updated)
    }

    /// Renames an account.
    pub fn set_label(&self, id: &str, label: &str) -> ConnectResult<AccountMeta> {
        self.update(id, |a| a.label = label.trim().to_string())
    }

    /// Sets the Ask/Always/Never policy.
    pub fn set_policy(&self, id: &str, policy: AccountPolicy) -> ConnectResult<AccountMeta> {
        self.update(id, |a| a.policy = policy)
    }

    /// Shows or hides the account's events.
    pub fn set_visible(&self, id: &str, visible: bool) -> ConnectResult<AccountMeta> {
        self.update(id, |a| a.visible = visible)
    }

    /// Sets the display colour (`#RRGGBB`).
    pub fn set_color(&self, id: &str, color: &str) -> ConnectResult<AccountMeta> {
        self.update(id, |a| a.color = color.to_string())
    }

    /// Browser sign-in for a new Google account (or re-adding an existing one).
    pub async fn add_google_account<F>(
        &self,
        open_url: F,
        cancel: &CancellationToken,
        timeout: Duration,
    ) -> ConnectResult<AccountMeta>
    where
        F: Fn(&str) + Send + Sync,
    {
        let authorized = self.oauth()?.authorize(None, open_url, cancel, timeout).await?;
        self.finish_login(authorized, None).await
    }

    /// Browser sign-in for an account in `AuthExpired` state (uses `login_hint`).
    pub async fn reauthenticate<F>(
        &self,
        email: &str,
        open_url: F,
        cancel: &CancellationToken,
        timeout: Duration,
    ) -> ConnectResult<AccountMeta>
    where
        F: Fn(&str) + Send + Sync,
    {
        let email = normalize_id(email);
        if self.get(&email).is_none() {
            return Err(ConnectError::NotFound(email));
        }
        let authorized = self
            .oauth()?
            .authorize(Some(&email), open_url, cancel, timeout)
            .await?;
        self.finish_login(authorized, Some(&email)).await
    }

    async fn finish_login(
        &self,
        authorized: Authorized,
        expected: Option<&str>,
    ) -> ConnectResult<AccountMeta> {
        let Authorized { tokens, profile } = authorized;
        let email = normalize_id(&profile.email);
        if let Some(expected) = expected.filter(|e| *e != email) {
            return Err(ConnectError::Invalid(format!(
                "signed in as {email}, but {expected} was expected"
            )));
        }
        let key = google_key(&email);
        match tokens.refresh_token.as_deref() {
            Some(refresh) => self.store.set(&key, refresh)?,
            None if self.store.get(&key)?.is_some() => {}
            None => {
                return Err(ConnectError::Protocol(
                    "Google did not return a refresh token; remove YadaCall at \
                     myaccount.google.com/permissions and sign in again"
                        .into(),
                ));
            }
        }
        self.cache_access(&email, &tokens).await;
        let meta = {
            let mut accounts = self.lock();
            let meta = match accounts.iter_mut().find(|a| a.id == email) {
                Some(existing) => {
                    existing.name = profile.name.clone().or(existing.name.take());
                    existing.picture = profile.picture.clone().or(existing.picture.take());
                    existing.hd = profile.hd.clone();
                    existing.state = AccountState::Ok;
                    existing.clone()
                }
                None => {
                    let color = PALETTE[accounts.len() % PALETTE.len()].to_string();
                    let meta = AccountMeta {
                        id: email.clone(),
                        kind: AccountKind::Google,
                        email: email.clone(),
                        name: profile.name.clone(),
                        picture: profile.picture.clone(),
                        hd: profile.hd.clone(),
                        label: email.clone(),
                        policy: AccountPolicy::Ask,
                        visible: true,
                        color,
                        state: AccountState::Ok,
                    };
                    accounts.push(meta.clone());
                    meta
                }
            };
            self.persist(&accounts)?;
            meta
        };
        Ok(meta)
    }

    async fn cache_access(&self, id: &str, tokens: &TokenResponse) {
        let expires_at = self.clock.now() + chrono::Duration::seconds(tokens.expires_in as i64);
        self.access.lock().await.insert(
            id.to_string(),
            CachedToken {
                token: tokens.access_token.clone(),
                expires_at,
            },
        );
    }

    /// A valid access token, refreshing when the cached one is (nearly) expired.
    /// `invalid_grant` flips the account to `AuthExpired` and publishes the event.
    pub async fn access_token(&self, id: &str) -> ConnectResult<String> {
        let id = normalize_id(id);
        let meta = self.get(&id).ok_or_else(|| ConnectError::NotFound(id.clone()))?;
        if meta.state == AccountState::AuthExpired {
            return Err(ConnectError::AuthExpired(id));
        }
        let mut cache = self.access.lock().await;
        if let Some(c) = cache.get(&id)
            && c.expires_at - chrono::Duration::seconds(EXPIRY_SKEW) > self.clock.now()
        {
            return Ok(c.token.clone());
        }
        let key = google_key(&id);
        let Some(refresh) = self.store.get(&key)? else {
            self.mark_expired(&id);
            return Err(ConnectError::AuthExpired(id));
        };
        match self.oauth()?.refresh(&refresh, &id).await {
            Ok(tokens) => {
                if let Some(new_refresh) = tokens.refresh_token.as_deref() {
                    self.store.set(&key, new_refresh)?;
                }
                let expires_at =
                    self.clock.now() + chrono::Duration::seconds(tokens.expires_in as i64);
                cache.insert(
                    id,
                    CachedToken {
                        token: tokens.access_token.clone(),
                        expires_at,
                    },
                );
                Ok(tokens.access_token)
            }
            Err(e @ ConnectError::AuthExpired(_)) => {
                cache.remove(&id);
                self.mark_expired(&id);
                Err(e)
            }
            Err(e) => Err(e),
        }
    }

    /// Forgets the cached access token (call after a 401) so the next call refreshes.
    pub async fn invalidate_access_token(&self, id: &str) {
        self.access.lock().await.remove(&normalize_id(id));
    }

    /// Marks an account as needing a new sign-in and publishes `AccountAuthExpired` once.
    pub fn mark_expired(&self, id: &str) {
        let id = normalize_id(id);
        let changed = {
            let mut accounts = self.lock();
            let Some(a) = accounts.iter_mut().find(|a| a.id == id) else {
                return;
            };
            let changed = a.state != AccountState::AuthExpired;
            a.state = AccountState::AuthExpired;
            if changed && let Err(e) = self.persist(&accounts) {
                tracing::warn!("could not persist account state: {e}");
            }
            changed
        };
        if changed {
            let _ = self.events.send(ConnectEvent::AccountAuthExpired { account: id });
        }
    }

    /// Revokes the token at Google (best effort), deletes the secret and the account.
    pub async fn remove_account(&self, id: &str) -> ConnectResult<RemoveOutcome> {
        let id = normalize_id(id);
        let meta = self.get(&id).ok_or_else(|| ConnectError::NotFound(id.clone()))?;
        let mut revoked = false;
        if meta.kind == AccountKind::Google {
            let key = google_key(&id);
            if let (Some(token), Some(oauth)) = (self.store.get(&key)?, self.oauth.as_ref()) {
                match oauth.revoke(&token).await {
                    Ok(()) => revoked = true,
                    Err(e) => tracing::warn!("token revoke failed for {id}: {e}"),
                }
            }
            self.store.delete(&key)?;
        } else {
            self.store.delete(&id)?;
        }
        self.access.lock().await.remove(&id);
        let mut accounts = self.lock();
        accounts.retain(|a| a.id != id);
        self.persist(&accounts)?;
        Ok(RemoveOutcome { revoked })
    }

    /// Registers an ICS subscription; the secret URL goes to the keychain, not to the JSON.
    pub fn add_ics(&self, url: &str, label: &str) -> ConnectResult<AccountMeta> {
        let url = normalize_ics_url(url)?;
        let id = format!("ics:{}", short_hash(&url));
        if self.get(&id).is_some() {
            return Err(ConnectError::Invalid("this calendar is already added".into()));
        }
        self.store.set(&id, &url)?;
        let mut accounts = self.lock();
        let meta = AccountMeta {
            id: id.clone(),
            kind: AccountKind::Ics,
            email: String::new(),
            name: None,
            picture: None,
            hd: None,
            label: if label.trim().is_empty() {
                "ICS".into()
            } else {
                label.trim().to_string()
            },
            policy: AccountPolicy::Ask,
            visible: true,
            color: PALETTE[accounts.len() % PALETTE.len()].to_string(),
            state: AccountState::Ok,
        };
        accounts.push(meta.clone());
        self.persist(&accounts)?;
        Ok(meta)
    }

    /// The secret URL of an ICS source.
    pub fn ics_url(&self, id: &str) -> ConnectResult<String> {
        self.store
            .get(id)?
            .ok_or_else(|| ConnectError::NotFound(id.to_string()))
    }
}

/// Google e-mails are case-insensitive; ICS ids are already canonical.
fn normalize_id(id: &str) -> String {
    if id.starts_with("ics:") {
        id.to_string()
    } else {
        id.trim().to_ascii_lowercase()
    }
}

/// Accepts `https://`, `http://` and `webcal://` (rewritten to https).
pub fn normalize_ics_url(url: &str) -> ConnectResult<String> {
    let url = url.trim();
    let rewritten = if let Some(rest) = url.strip_prefix("webcal://") {
        format!("https://{rest}")
    } else {
        url.to_string()
    };
    let parsed =
        url::Url::parse(&rewritten).map_err(|e| ConnectError::Invalid(format!("bad URL: {e}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ConnectError::Invalid("ICS URL must be http(s) or webcal".into()));
    }
    Ok(rewritten)
}

fn short_hash(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes())[..6]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webcal_is_rewritten_and_junk_rejected() {
        assert_eq!(
            normalize_ics_url(" webcal://x.com/a.ics ").unwrap(),
            "https://x.com/a.ics"
        );
        assert!(normalize_ics_url("ftp://x.com/a.ics").is_err());
        assert!(normalize_ics_url("not a url").is_err());
    }
}
