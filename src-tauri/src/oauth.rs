//! PKCE OAuth manager: pure helpers (PKCE generation, authorize-URL assembly,
//! per-provider config) plus a live `begin_login` flow that opens the system
//! browser, runs a localhost callback server, and exchanges the code for
//! `Credentials`.
//!
//! SECURITY: never log/print the verifier, authorization code, access_token,
//! or refresh_token. Error strings must not embed token/secret values.

use std::time::Duration;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::Utc;
use rand::RngCore;
use sha2::{Digest, Sha256};
use tiny_http::{Response, Server};
use usage_core::account::{Credentials, Provider};

/// Per-provider OAuth (PKCE, public client) configuration.
#[derive(Clone, Debug)]
pub struct ProviderOAuth {
    pub client_id: String,
    pub auth_url: String,
    pub token_url: String,
    pub scopes: String,
}

/// Returns the PKCE OAuth configuration for a provider, or an `Err` when the
/// provider has no reproducible OAuth flow (the UI should route those to a
/// fallback/manual-import path instead).
pub fn config(provider: Provider) -> Result<ProviderOAuth, String> {
    match provider {
        // Codex CLI (ChatGPT) login uses a public PKCE client against the
        // ChatGPT auth endpoints. client_id is the well-known public id used
        // by the open-source `codex` CLI's login flow.
        // TODO: verify against Codex CLI login flow (client_id/scopes may
        // drift with future CLI releases).
        Provider::Codex => Ok(ProviderOAuth {
            client_id: "app_EMoamEEZ73f0CkXaXp7hrann".to_string(),
            auth_url: "https://auth.openai.com/oauth/authorize".to_string(),
            token_url: "https://auth.openai.com/oauth/token".to_string(),
            scopes: "openid profile email offline_access".to_string(),
        }),
        // Claude Code CLI login uses a public PKCE client against Anthropic's
        // console OAuth endpoints.
        // TODO: verify against Claude Code CLI login flow (client_id/scopes
        // may drift with future CLI releases).
        Provider::Claude => Ok(ProviderOAuth {
            client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e".to_string(),
            auth_url: "https://console.anthropic.com/oauth/authorize".to_string(),
            token_url: "https://console.anthropic.com/v1/oauth/token".to_string(),
            scopes: "org:create_api_key user:profile user:inference".to_string(),
        }),
        // Per Task 9's investigation: agy/Gemini auth lives behind an
        // Electron-style Keychain-encrypted blob with no discoverable public
        // OAuth endpoint. Do not invent a fake flow — route the UI to the
        // fallback import path instead.
        Provider::Agy => Err("agy OAuth unavailable — use fallback import".to_string()),
    }
}

/// Generates a PKCE verifier (43-128 chars, unreserved base64url charset) and
/// its S256 code challenge. Pure function — no I/O, no logging.
pub fn make_pkce() -> (String, String) {
    let mut bytes = [0u8; 64];
    rand::thread_rng().fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let digest = hasher.finalize();
    let challenge = URL_SAFE_NO_PAD.encode(digest);

    (verifier, challenge)
}

/// Generates a random opaque `state` value for CSRF protection on the OAuth
/// callback. Pure function — no I/O.
pub fn make_state() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Assembles the provider authorize URL with all required PKCE + OAuth query
/// parameters. Pure function — no I/O.
pub fn build_authorize_url(cfg: &ProviderOAuth, challenge: &str, redirect: &str, state: &str) -> String {
    let mut url = String::with_capacity(256);
    url.push_str(&cfg.auth_url);
    url.push('?');
    url.push_str("response_type=code");
    url.push_str("&client_id=");
    url.push_str(&urlencoding::encode(&cfg.client_id));
    url.push_str("&redirect_uri=");
    url.push_str(&urlencoding::encode(redirect));
    url.push_str("&scope=");
    url.push_str(&urlencoding::encode(&cfg.scopes));
    url.push_str("&code_challenge=");
    url.push_str(&urlencoding::encode(challenge));
    url.push_str("&code_challenge_method=S256");
    url.push_str("&state=");
    url.push_str(&urlencoding::encode(state));
    url
}

/// Parsed `?code=...&state=...` callback query parameters.
struct CallbackParams {
    code: String,
    state: String,
}

/// Extracts `code` and `state` from a callback request path/query string.
/// Pure function — no I/O.
fn parse_callback_query(url: &str) -> Option<CallbackParams> {
    let query = url.split_once('?')?.1;
    let mut code: Option<String> = None;
    let mut state: Option<String> = None;

    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        // The callback query values are percent-decoded before use.
        let decoded = urlencoding::decode(value).ok()?.into_owned();
        match key {
            "code" => code = Some(decoded),
            "state" => state = Some(decoded),
            _ => {}
        }
    }

    Some(CallbackParams {
        code: code?,
        state: state?,
    })
}

/// Token-endpoint response shape (Codex/Claude both return this standard
/// OAuth2 token response body).
#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    account_id: Option<String>,
}

/// Runs the full interactive login flow for `provider`:
/// 1. binds a localhost callback server on an ephemeral port,
/// 2. opens the system browser at the provider's authorize URL,
/// 3. waits for the `?code=...&state=...` redirect (validating `state`),
/// 4. exchanges the code for tokens at the provider's token endpoint.
///
/// SECURITY: no verifier/code/token value is ever logged.
pub async fn begin_login(provider: Provider) -> Result<Credentials, String> {
    let cfg = config(provider)?;

    let server = Server::http("127.0.0.1:0")
        .map_err(|e| format!("failed to bind localhost callback server: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .ok_or_else(|| "failed to determine callback server port".to_string())?;
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    let (verifier, challenge) = make_pkce();
    let state = make_state();
    let authorize_url = build_authorize_url(&cfg, &challenge, &redirect_uri, &state);

    open::that(&authorize_url).map_err(|e| format!("failed to open browser: {e}"))?;

    // Block the calling (spawned) task while waiting for the single callback
    // request. tiny_http's `recv` blocks synchronously, so this must run on a
    // blocking-friendly context — callers should invoke this via
    // `tauri::async_runtime::spawn` / `spawn_blocking` as appropriate.
    let request = server
        .recv_timeout(Duration::from_secs(300))
        .map_err(|e| format!("callback server error: {e}"))?
        .ok_or_else(|| "login timed out waiting for browser callback".to_string())?;

    let params = parse_callback_query(request.url())
        .ok_or_else(|| "callback missing code/state parameters".to_string())?;

    if params.state != state {
        let _ = request.respond(Response::from_string("State mismatch. You may close this tab."));
        return Err("state mismatch on OAuth callback — rejecting".to_string());
    }

    let _ = request.respond(Response::from_string(
        "Login complete. You may close this tab and return to the app.",
    ));

    let client = reqwest::Client::new();
    let token_response = client
        .post(&cfg.token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", params.code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", cfg.client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .map_err(|e| format!("token exchange request failed: {e}"))?;

    if !token_response.status().is_success() {
        let status = token_response.status();
        return Err(format!("token exchange failed with status {status}"));
    }

    let body: TokenResponse = token_response
        .json()
        .await
        .map_err(|e| format!("failed to parse token response: {e}"))?;

    let expires_at = body
        .expires_in
        .map(|secs| Utc::now() + chrono::Duration::seconds(secs));

    Ok(Credentials {
        access_token: body.access_token,
        refresh_token: body.refresh_token,
        account_id: body.account_id,
        expires_at,
    })
}

/// Decides whether an access token should be proactively refreshed: true
/// when `expires_at` is within `threshold` of `now` (including already
/// expired), false when there is no known expiry (nothing to refresh
/// against) or expiry is comfortably in the future. Pure function — no I/O.
pub fn should_refresh(expires_at: Option<chrono::DateTime<Utc>>, now: chrono::DateTime<Utc>, threshold: Duration) -> bool {
    match expires_at {
        Some(exp) => exp - now <= chrono::Duration::from_std(threshold).unwrap_or(chrono::Duration::zero()),
        None => false,
    }
}

/// Proactively refreshes `creds` for `provider` using its stored
/// `refresh_token`. Requires the provider to have a reproducible OAuth
/// config (agy's `config` lookup fails, propagating here) and a present
/// `refresh_token`.
///
/// SECURITY: no verifier/code/token value is ever logged; error strings
/// carry only status codes / non-secret text.
pub async fn refresh_access_token(provider: Provider, creds: &Credentials) -> Result<Credentials, String> {
    let cfg = config(provider)?;

    let refresh_token = creds
        .refresh_token
        .as_ref()
        .ok_or_else(|| "no refresh_token".to_string())?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;

    let response = client
        .post(&cfg.token_url)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("client_id", cfg.client_id.as_str()),
        ])
        .send()
        .await
        .map_err(|e| format!("token refresh request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        return Err(format!("token refresh failed with status {status}"));
    }

    let body: TokenResponse = response
        .json()
        .await
        .map_err(|e| format!("failed to parse token refresh response: {e}"))?;

    let expires_at = body
        .expires_in
        .map(|secs| Utc::now() + chrono::Duration::seconds(secs));

    Ok(Credentials {
        access_token: body.access_token,
        refresh_token: body.refresh_token.or_else(|| creds.refresh_token.clone()),
        account_id: creds.account_id.clone(),
        expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_url_safe_no_padding() {
        let (verifier, challenge) = make_pkce();
        assert!(verifier.len() >= 43);
        assert!(!challenge.contains('=') && !challenge.contains('+') && !challenge.contains('/'));
    }

    #[test]
    fn authorize_url_contains_params() {
        let cfg = ProviderOAuth {
            client_id: "cid".into(),
            auth_url: "https://auth.example/authorize".into(),
            token_url: "https://auth.example/token".into(),
            scopes: "openid".into(),
        };
        let url = build_authorize_url(&cfg, "chal", "http://127.0.0.1:1455/cb", "st8");
        assert!(url.contains("client_id=cid"));
        assert!(url.contains("code_challenge=chal"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=st8"));
    }

    #[test]
    fn agy_config_is_err() {
        assert!(config(Provider::Agy).is_err());
    }

    #[test]
    fn codex_and_claude_config_present() {
        assert!(config(Provider::Codex).is_ok());
        assert!(config(Provider::Claude).is_ok());
    }

    #[test]
    fn parse_callback_query_extracts_code_and_state() {
        let params = parse_callback_query("/callback?code=abc123&state=xyz").unwrap();
        assert_eq!(params.code, "abc123");
        assert_eq!(params.state, "xyz");
    }

    #[test]
    fn parse_callback_query_none_without_query() {
        assert!(parse_callback_query("/callback").is_none());
    }

    #[test]
    fn should_refresh_true_when_already_expired() {
        let now = Utc::now();
        let expires_at = Some(now - chrono::Duration::seconds(5));
        assert!(should_refresh(expires_at, now, Duration::from_secs(60)));
    }

    #[test]
    fn should_refresh_true_when_within_threshold() {
        let now = Utc::now();
        let expires_at = Some(now + chrono::Duration::seconds(30));
        assert!(should_refresh(expires_at, now, Duration::from_secs(60)));
    }

    #[test]
    fn should_refresh_false_when_comfortably_in_future() {
        let now = Utc::now();
        let expires_at = Some(now + chrono::Duration::seconds(300));
        assert!(!should_refresh(expires_at, now, Duration::from_secs(60)));
    }

    #[test]
    fn should_refresh_false_when_no_expiry_known() {
        let now = Utc::now();
        assert!(!should_refresh(None, now, Duration::from_secs(60)));
    }

    #[test]
    fn should_refresh_true_at_exact_threshold_boundary() {
        let now = Utc::now();
        let expires_at = Some(now + chrono::Duration::seconds(60));
        assert!(should_refresh(expires_at, now, Duration::from_secs(60)));
    }
}
