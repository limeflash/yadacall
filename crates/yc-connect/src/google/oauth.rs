//! The Google OAuth 2.0 installed-app flow: authorization URL, code exchange, refresh, revoke.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::config::{CALENDAR_SCOPE, GoogleConfig, SCOPES};
use super::loopback::CallbackServer;
use super::pkce::{Pkce, random_token};
use crate::error::{ConnectError, ConnectResult};

/// Default time the user gets to finish the browser sign-in.
pub const DEFAULT_LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

/// Response of the token endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    /// Seconds until the access token expires.
    #[serde(default)]
    pub expires_in: u64,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
}

/// Who signed in.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GoogleProfile {
    pub email: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub picture: Option<String>,
    /// Workspace domain of the account, absent for consumer accounts.
    #[serde(default)]
    pub hd: Option<String>,
}

/// Result of a completed sign-in.
#[derive(Debug, Clone)]
pub struct Authorized {
    pub tokens: TokenResponse,
    pub profile: GoogleProfile,
}

/// Client for Google's OAuth endpoints.
#[derive(Debug, Clone)]
pub struct GoogleOAuth {
    http: reqwest::Client,
    config: GoogleConfig,
}

impl GoogleOAuth {
    /// Creates a client for `config`.
    pub fn new(config: GoogleConfig) -> ConnectResult<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("YadaCall/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { http, config })
    }

    /// The configuration in use.
    pub fn config(&self) -> &GoogleConfig {
        &self.config
    }

    /// Builds the consent URL. `login_hint` set means re-authentication of a known account;
    /// otherwise this is a first add and `prompt=consent` makes Google issue a refresh token.
    pub fn authorization_url(
        &self,
        redirect_uri: &str,
        pkce: &Pkce,
        state: &str,
        login_hint: Option<&str>,
    ) -> ConnectResult<Url> {
        let mut url = Url::parse(&self.config.endpoints.auth)
            .map_err(|e| ConnectError::Invalid(e.to_string()))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("client_id", &self.config.client_id)
                .append_pair("redirect_uri", redirect_uri)
                .append_pair("response_type", "code")
                .append_pair("scope", SCOPES)
                .append_pair("code_challenge", &pkce.challenge)
                .append_pair("code_challenge_method", "S256")
                .append_pair("state", state)
                .append_pair("access_type", "offline");
            match login_hint {
                Some(hint) => q.append_pair("login_hint", hint),
                None => q.append_pair("prompt", "consent"),
            };
        }
        Ok(url)
    }

    /// Runs the whole browser flow: bind loopback, open the browser, wait for the redirect,
    /// exchange the code and read the profile.
    pub async fn authorize<F>(
        &self,
        login_hint: Option<&str>,
        open_url: F,
        cancel: &CancellationToken,
        timeout: Duration,
    ) -> ConnectResult<Authorized>
    where
        F: Fn(&str) + Send + Sync,
    {
        let server = CallbackServer::bind().await?;
        let redirect_uri = server.redirect_uri();
        let pkce = Pkce::generate();
        let state = random_token(24);
        let url = self.authorization_url(&redirect_uri, &pkce, &state, login_hint)?;
        open_url(url.as_str());
        let code = server.wait_for_code(&state, cancel, timeout).await?;
        let tokens = self.exchange_code(&code, &pkce.verifier, &redirect_uri).await?;
        let granted = tokens.scope.as_deref().unwrap_or_default();
        if !granted.split_whitespace().any(|s| s == CALENDAR_SCOPE) {
            return Err(ConnectError::Denied(
                "calendar access was not granted; tick the calendar permission on the consent screen"
                    .into(),
            ));
        }
        let profile = self.profile(&tokens).await?;
        Ok(Authorized { tokens, profile })
    }

    /// Exchanges the authorization code (with the PKCE verifier) for tokens.
    pub async fn exchange_code(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> ConnectResult<TokenResponse> {
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect_uri),
            ("client_id", self.config.client_id.as_str()),
        ];
        if let Some(secret) = &self.config.client_secret {
            form.push(("client_secret", secret));
        }
        let response = self
            .http
            .post(&self.config.endpoints.token)
            .form(&form)
            .send()
            .await?;
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ConnectError::Denied(oauth_error(&body)));
        }
        Ok(response.json().await?)
    }

    /// Uses a refresh token to mint a new access token. `invalid_grant` becomes
    /// [`ConnectError::AuthExpired`] carrying `account`.
    pub async fn refresh(&self, refresh_token: &str, account: &str) -> ConnectResult<TokenResponse> {
        let mut form = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", self.config.client_id.as_str()),
        ];
        if let Some(secret) = &self.config.client_secret {
            form.push(("client_secret", secret));
        }
        let response = self
            .http
            .post(&self.config.endpoints.token)
            .form(&form)
            .send()
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response.json().await?);
        }
        let body = response.text().await.unwrap_or_default();
        if oauth_error(&body).starts_with("invalid_grant") {
            return Err(ConnectError::AuthExpired(account.to_string()));
        }
        if status.is_server_error() || status.as_u16() == 429 {
            return Err(ConnectError::Network(format!("token endpoint: HTTP {status}")));
        }
        Err(ConnectError::Protocol(format!(
            "token refresh failed: {}",
            oauth_error(&body)
        )))
    }

    /// Revokes a token at Google (refresh tokens revoke their access tokens too).
    pub async fn revoke(&self, token: &str) -> ConnectResult<()> {
        let response = self
            .http
            .post(&self.config.endpoints.revoke)
            .form(&[("token", token)])
            .send()
            .await?;
        let status = response.status();
        // 400 invalid_token means it was already revoked/expired: the goal is met.
        if status.is_success() || status.as_u16() == 400 {
            Ok(())
        } else {
            Err(ConnectError::Network(format!("revoke: HTTP {status}")))
        }
    }

    /// Reads the profile from the ID token, falling back to the userinfo endpoint.
    async fn profile(&self, tokens: &TokenResponse) -> ConnectResult<GoogleProfile> {
        if let Some(profile) = tokens.id_token.as_deref().and_then(profile_from_id_token) {
            return Ok(profile);
        }
        let response = self
            .http
            .get(&self.config.endpoints.userinfo)
            .bearer_auth(&tokens.access_token)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(ConnectError::Protocol(format!(
                "userinfo: HTTP {}",
                response.status()
            )));
        }
        let profile: GoogleProfile = response.json().await?;
        if profile.email.is_empty() {
            return Err(ConnectError::Protocol("userinfo has no e-mail".into()));
        }
        Ok(profile)
    }
}

/// Decodes the claims of an ID token received directly from Google's token endpoint over TLS
/// (the signature need not be verified in that case, per Google's guidance).
pub fn profile_from_id_token(id_token: &str) -> Option<GoogleProfile> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let profile: GoogleProfile = serde_json::from_slice(&bytes).ok()?;
    (!profile.email.is_empty()).then_some(profile)
}

/// `error` (+ description) of an OAuth error body, or a trimmed copy of the body.
fn oauth_error(body: &str) -> String {
    #[derive(Deserialize)]
    struct E {
        error: Option<String>,
        error_description: Option<String>,
    }
    match serde_json::from_str::<E>(body) {
        Ok(E {
            error: Some(e),
            error_description,
        }) => match error_description {
            Some(d) => format!("{e}: {d}"),
            None => e,
        },
        _ => body.chars().take(200).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::google::config::Endpoints;

    fn oauth() -> GoogleOAuth {
        GoogleOAuth::new(GoogleConfig::with_endpoints("cid", None, Endpoints::default())).unwrap()
    }

    #[test]
    fn authorization_url_has_required_parameters() {
        let pkce = Pkce::generate();
        let first = oauth()
            .authorization_url("http://127.0.0.1:5/callback", &pkce, "st", None)
            .unwrap();
        let q: std::collections::HashMap<_, _> = first.query_pairs().into_owned().collect();
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["code_challenge"], pkce.challenge);
        assert_eq!(q["access_type"], "offline");
        assert_eq!(q["prompt"], "consent");
        assert_eq!(q["scope"], SCOPES);
        assert_eq!(q["state"], "st");
        assert!(!q.contains_key("login_hint"));

        let re = oauth()
            .authorization_url("http://127.0.0.1:5/callback", &pkce, "st", Some("a@b.c"))
            .unwrap();
        let q: std::collections::HashMap<_, _> = re.query_pairs().into_owned().collect();
        assert_eq!(q["login_hint"], "a@b.c");
        assert!(!q.contains_key("prompt"));
    }

    #[test]
    fn id_token_claims() {
        let payload = URL_SAFE_NO_PAD
            .encode(r#"{"email":"a@corp.com","name":"Анна","picture":"http://p","hd":"corp.com","sub":"1"}"#);
        let p = profile_from_id_token(&format!("h.{payload}.s")).unwrap();
        assert_eq!(p.email, "a@corp.com");
        assert_eq!(p.hd.as_deref(), Some("corp.com"));
        assert!(profile_from_id_token("garbage").is_none());
    }

    #[test]
    fn error_body_parsing() {
        assert_eq!(
            oauth_error(r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#),
            "invalid_grant: Token has been expired or revoked."
        );
    }
}
