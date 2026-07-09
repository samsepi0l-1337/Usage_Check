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

/// Codex CLI registers this exact loopback redirect with OpenAI Hydra.
/// Using an ephemeral port or `/callback` (instead of `/auth/callback`)
/// yields `authorize_hydra_invalid_request`.
const CODEX_CALLBACK_PORT: u16 = 1455;
const CODEX_CALLBACK_PATH: &str = "/auth/callback";
const CODEX_ORIGINATOR: &str = "usagecheck";

/// Per-provider OAuth configuration.
///
/// Codex/Claude use public PKCE clients. Antigravity uses Google's confidential
/// client (client_secret, no PKCE) — same client as Antigravity.app / agy.
#[derive(Clone, Debug)]
pub struct ProviderOAuth {
    pub client_id: String,
    /// Present for confidential clients (Antigravity Google OAuth).
    pub client_secret: Option<String>,
    pub auth_url: String,
    pub token_url: String,
    pub scopes: String,
    /// When set, bind this exact port and use `http://localhost:{port}{path}`
    /// as `redirect_uri` (required for Codex's registered client).
    pub fixed_redirect: Option<(u16, &'static str)>,
    /// Extra authorize-query params required by the provider (Codex CLI flags).
    pub extra_authorize_params: Vec<(String, String)>,
    /// Public clients (Codex/Claude) require PKCE; Antigravity does not.
    pub use_pkce: bool,
}

const AGY_SCOPES: &str = "openid \
https://www.googleapis.com/auth/cloud-platform \
https://www.googleapis.com/auth/userinfo.email \
https://www.googleapis.com/auth/userinfo.profile \
https://www.googleapis.com/auth/cclog \
https://www.googleapis.com/auth/experimentsandconfigs";

/// Prefer the Antigravity enterprise-style Google client when multiple
/// `*.apps.googleusercontent.com` IDs appear in a binary (numeric prefix).
const AGY_PREFERRED_CLIENT_PREFIX: &str = "1071006060591-";

/// Resolves Antigravity Google OAuth client_id + client_secret without
/// embedding them in source (GitHub push protection blocks that).
///
/// Order:
/// 1. `ANTIGRAVITY_OAUTH_CLIENT_ID` + `ANTIGRAVITY_OAUTH_CLIENT_SECRET`
/// 2. Scan a local `agy` / Antigravity.app binary for the embedded pair
pub fn resolve_agy_oauth_client() -> Result<(String, String), String> {
    if let (Ok(id), Ok(secret)) = (
        std::env::var("ANTIGRAVITY_OAUTH_CLIENT_ID"),
        std::env::var("ANTIGRAVITY_OAUTH_CLIENT_SECRET"),
    ) {
        let id = id.trim().to_string();
        let secret = secret.trim().to_string();
        if !id.is_empty() && !secret.is_empty() {
            return Ok((id, secret));
        }
    }

    for path in agy_oauth_binary_candidates() {
        if let Ok(data) = std::fs::read(&path) {
            if let Some(pair) = extract_google_oauth_pair(&data) {
                return Ok(pair);
            }
        }
    }

    Err(
        "Antigravity OAuth credentials not found — set ANTIGRAVITY_OAUTH_CLIENT_ID and \
         ANTIGRAVITY_OAUTH_CLIENT_SECRET, or install Antigravity.app / agy so UsageCheck can \
         read the embedded Google OAuth client"
            .into(),
    )
}

fn agy_oauth_binary_candidates() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("ANTIGRAVITY_CLI_PATH") {
        out.push(std::path::PathBuf::from(p));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::PathBuf::from(home);
        out.push(home.join(".local/bin/agy"));
    }
    out.push(std::path::PathBuf::from("/opt/homebrew/bin/agy"));
    out.push(std::path::PathBuf::from("/usr/local/bin/agy"));
    out.push(std::path::PathBuf::from(
        "/Applications/Antigravity.app/Contents/Resources/bin/language_server",
    ));
    out.push(std::path::PathBuf::from(
        "/Applications/Antigravity.app/Contents/MacOS/Antigravity",
    ));
    out
}

/// Scans binary bytes for a Google OAuth client id + `GOCSPX-…` secret pair.
/// Prefers the Antigravity enterprise client id prefix when several exist.
pub fn extract_google_oauth_pair(data: &[u8]) -> Option<(String, String)> {
    let clients = extract_ascii_matches(data, |s| {
        s.ends_with(".apps.googleusercontent.com")
            && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
            && s.contains('-')
            && s.len() > 40
    });
    let secrets = extract_ascii_matches(data, |s| {
        s.starts_with("GOCSPX-")
            && s.len() > 10
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    });
    if clients.is_empty() || secrets.is_empty() {
        return None;
    }

    let client = clients
        .iter()
        .find(|c| c.starts_with(AGY_PREFERRED_CLIENT_PREFIX))
        .cloned()
        .unwrap_or_else(|| clients[0].clone());

    // Prefer the secret that appears nearest to the chosen client id in the file.
    let client_pos = data
        .windows(client.len())
        .position(|w| w == client.as_bytes())
        .unwrap_or(0);
    let mut best: Option<(usize, String)> = None;
    for secret in &secrets {
        let Some(pos) = data
            .windows(secret.len())
            .position(|w| w == secret.as_bytes())
        else {
            continue;
        };
        let dist = client_pos.abs_diff(pos);
        if best.as_ref().map(|(d, _)| dist < *d).unwrap_or(true) {
            best = Some((dist, secret.clone()));
        }
    }
    let secret = best?.1;
    Some((client, secret))
}

fn extract_ascii_matches(data: &[u8], pred: impl Fn(&str) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, &b) in data.iter().enumerate() {
        let ok = b.is_ascii_graphic() && b != b'"' && b != b'\'' && b != b'\\' && b != b'<';
        if ok {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start.take() {
            if let Ok(text) = std::str::from_utf8(&data[s..i]) {
                if pred(text) && !out.iter().any(|x| x == text) {
                    out.push(text.to_string());
                }
            }
        }
    }
    if let Some(s) = start {
        if let Ok(text) = std::str::from_utf8(&data[s..]) {
            if pred(text) && !out.iter().any(|x| x == text) {
                out.push(text.to_string());
            }
        }
    }
    out
}

/// Returns the OAuth configuration for a provider.
pub fn config(provider: Provider) -> Result<ProviderOAuth, String> {
    match provider {
        // Verified against Codex CLI 0.143.0 (`codex-rs/login/src/server.rs`)
        // and local `~/.codex/log/codex-login.log` redirect_uri traces.
        Provider::Codex => Ok(ProviderOAuth {
            client_id: "app_EMoamEEZ73f0CkXaXp7hrann".to_string(),
            client_secret: None,
            auth_url: "https://auth.openai.com/oauth/authorize".to_string(),
            token_url: "https://auth.openai.com/oauth/token".to_string(),
            scopes: "openid profile email offline_access api.connectors.read api.connectors.invoke"
                .to_string(),
            fixed_redirect: Some((CODEX_CALLBACK_PORT, CODEX_CALLBACK_PATH)),
            extra_authorize_params: vec![
                ("id_token_add_organizations".into(), "true".into()),
                ("codex_cli_simplified_flow".into(), "true".into()),
                ("originator".into(), CODEX_ORIGINATOR.into()),
            ],
            use_pkce: true,
        }),
        // Claude Code CLI login uses a public PKCE client against Anthropic's
        // console OAuth endpoints. Ephemeral localhost ports are accepted.
        Provider::Claude => Ok(ProviderOAuth {
            client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e".to_string(),
            client_secret: None,
            auth_url: "https://console.anthropic.com/oauth/authorize".to_string(),
            token_url: "https://console.anthropic.com/v1/oauth/token".to_string(),
            scopes: "org:create_api_key user:profile user:inference".to_string(),
            fixed_redirect: None,
            extra_authorize_params: Vec::new(),
            use_pkce: true,
        }),
        // Google OAuth for Antigravity — credentials from env or local install.
        Provider::Agy => {
            let (client_id, client_secret) = resolve_agy_oauth_client()?;
            Ok(ProviderOAuth {
                client_id,
                client_secret: Some(client_secret),
                auth_url: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
                token_url: "https://oauth2.googleapis.com/token".to_string(),
                scopes: AGY_SCOPES.to_string(),
                fixed_redirect: None,
                extra_authorize_params: vec![
                    ("access_type".into(), "offline".into()),
                    ("prompt".into(), "consent".into()),
                    ("include_granted_scopes".into(), "true".into()),
                ],
                use_pkce: false,
            })
        }
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

/// Assembles the provider authorize URL with OAuth query parameters (plus any
/// provider-specific extras). When `cfg.use_pkce` is true, includes S256
/// `code_challenge`. Pure function — no I/O.
pub fn build_authorize_url(cfg: &ProviderOAuth, challenge: &str, redirect: &str, state: &str) -> String {
    let mut url = String::with_capacity(512);
    url.push_str(&cfg.auth_url);
    url.push('?');
    url.push_str("response_type=code");
    url.push_str("&client_id=");
    url.push_str(&urlencoding::encode(&cfg.client_id));
    url.push_str("&redirect_uri=");
    url.push_str(&urlencoding::encode(redirect));
    url.push_str("&scope=");
    url.push_str(&urlencoding::encode(&cfg.scopes));
    if cfg.use_pkce {
        url.push_str("&code_challenge=");
        url.push_str(&urlencoding::encode(challenge));
        url.push_str("&code_challenge_method=S256");
    }
    url.push_str("&state=");
    url.push_str(&urlencoding::encode(state));
    for (key, value) in &cfg.extra_authorize_params {
        url.push('&');
        url.push_str(&urlencoding::encode(key));
        url.push('=');
        url.push_str(&urlencoding::encode(value));
    }
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
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    account_id: Option<String>,
}

/// Extracts `chatgpt_account_id` from a ChatGPT id_token JWT payload
/// (`https://api.openai.com/auth` claim). Pure — no I/O, never logs the token.
pub fn chatgpt_account_id_from_id_token(id_token: &str) -> Option<String> {
    let payload_b64 = id_token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let root: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    root.get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .map(str::to_string)
}

fn bind_callback_server(cfg: &ProviderOAuth) -> Result<(Server, String), String> {
    match cfg.fixed_redirect {
        Some((port, path)) => {
            let server = Server::http(format!("127.0.0.1:{port}")).map_err(|e| {
                format!(
                    "failed to bind OAuth callback on 127.0.0.1:{port}{path}: {e} \
                     (is another Codex login already using this port?)"
                )
            })?;
            // OpenAI registers `localhost`, not `127.0.0.1` — must match exactly.
            Ok((server, format!("http://localhost:{port}{path}")))
        }
        None => {
            let server = Server::http("127.0.0.1:0")
                .map_err(|e| format!("failed to bind localhost callback server: {e}"))?;
            let port = server
                .server_addr()
                .to_ip()
                .map(|a| a.port())
                .ok_or_else(|| "failed to determine callback server port".to_string())?;
            Ok((server, format!("http://127.0.0.1:{port}/callback")))
        }
    }
}

/// Runs the full interactive login flow for `provider`:
/// 1. binds a localhost callback server (fixed port for Codex),
/// 2. opens the system browser at the provider's authorize URL,
/// 3. waits for the `?code=...&state=...` redirect (validating `state`),
/// 4. exchanges the code for tokens at the provider's token endpoint.
///
/// SECURITY: no verifier/code/token value is ever logged.
pub async fn begin_login(provider: Provider) -> Result<Credentials, String> {
    let cfg = config(provider)?;

    let (server, redirect_uri) = bind_callback_server(&cfg)?;

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
    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "authorization_code"),
        ("code", params.code.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("client_id", cfg.client_id.as_str()),
    ];
    if cfg.use_pkce {
        form.push(("code_verifier", verifier.as_str()));
    }
    if let Some(secret) = cfg.client_secret.as_deref() {
        form.push(("client_secret", secret));
    }
    let token_response = client
        .post(&cfg.token_url)
        .form(&form)
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

    let account_id = body.account_id.or_else(|| {
        body.id_token
            .as_deref()
            .and_then(chatgpt_account_id_from_id_token)
    });

    Ok(Credentials {
        access_token: body.access_token,
        refresh_token: body.refresh_token,
        account_id,
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

    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.as_str()),
        ("client_id", cfg.client_id.as_str()),
    ];
    if let Some(secret) = cfg.client_secret.as_deref() {
        form.push(("client_secret", secret));
    }
    let response = client
        .post(&cfg.token_url)
        .form(&form)
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

    let account_id = body
        .account_id
        .or_else(|| {
            body.id_token
                .as_deref()
                .and_then(chatgpt_account_id_from_id_token)
        })
        .or_else(|| creds.account_id.clone());

    Ok(Credentials {
        access_token: body.access_token,
        refresh_token: body.refresh_token.or_else(|| creds.refresh_token.clone()),
        account_id,
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
            client_secret: None,
            auth_url: "https://auth.example/authorize".into(),
            token_url: "https://auth.example/token".into(),
            scopes: "openid".into(),
            fixed_redirect: None,
            extra_authorize_params: vec![("id_token_add_organizations".into(), "true".into())],
            use_pkce: true,
        };
        let url = build_authorize_url(&cfg, "chal", "http://localhost:1455/auth/callback", "st8");
        assert!(url.contains("client_id=cid"));
        assert!(url.contains("code_challenge=chal"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=st8"));
        assert!(url.contains("id_token_add_organizations=true"));
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"));
    }

    #[test]
    fn authorize_url_skips_pkce_when_disabled() {
        let cfg = ProviderOAuth {
            client_id: "cid".into(),
            client_secret: Some("sec".into()),
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            scopes: "openid".into(),
            fixed_redirect: None,
            extra_authorize_params: vec![("access_type".into(), "offline".into())],
            use_pkce: false,
        };
        let url = build_authorize_url(&cfg, "chal", "http://127.0.0.1:9/callback", "st8");
        assert!(!url.contains("code_challenge"));
        assert!(url.contains("access_type=offline"));
    }

    #[test]
    fn codex_config_matches_cli_contract() {
        let cfg = config(Provider::Codex).unwrap();
        assert_eq!(cfg.client_id, "app_EMoamEEZ73f0CkXaXp7hrann");
        assert_eq!(cfg.fixed_redirect, Some((1455, "/auth/callback")));
        assert!(cfg.scopes.contains("api.connectors.read"));
        assert!(cfg.use_pkce);
        assert!(cfg
            .extra_authorize_params
            .iter()
            .any(|(k, v)| k == "codex_cli_simplified_flow" && v == "true"));
    }

    #[test]
    fn agy_config_resolves_from_env() {
        std::env::set_var(
            "ANTIGRAVITY_OAUTH_CLIENT_ID",
            "1071006060591-test.apps.googleusercontent.com",
        );
        std::env::set_var("ANTIGRAVITY_OAUTH_CLIENT_SECRET", "GOCSPX-test-secret-value");
        let cfg = config(Provider::Agy).unwrap();
        assert!(cfg.client_id.contains("apps.googleusercontent.com"));
        assert!(cfg.client_secret.is_some());
        assert!(!cfg.use_pkce);
        assert!(cfg.scopes.contains("cloud-platform"));
        std::env::remove_var("ANTIGRAVITY_OAUTH_CLIENT_ID");
        std::env::remove_var("ANTIGRAVITY_OAUTH_CLIENT_SECRET");
    }

    #[test]
    fn extract_google_oauth_pair_prefers_enterprise_prefix() {
        let blob = b"noise 884354919052-other.apps.googleusercontent.com xx \
GOCSPX-AAAA1111 BBB 1071006060591-exampleclientid000000000000000.apps.googleusercontent.com \
yy GOCSPX-BBBB2222 end";
        let (id, secret) = extract_google_oauth_pair(blob).unwrap();
        assert!(id.starts_with("1071006060591-"));
        assert_eq!(secret, "GOCSPX-BBBB2222");
    }

    #[test]
    fn codex_and_claude_config_present() {
        assert!(config(Provider::Codex).is_ok());
        assert!(config(Provider::Claude).is_ok());
    }

    #[test]
    fn parse_callback_query_extracts_code_and_state() {
        let params = parse_callback_query("/auth/callback?code=abc123&state=xyz").unwrap();
        assert_eq!(params.code, "abc123");
        assert_eq!(params.state, "xyz");
    }

    #[test]
    fn parse_callback_query_none_without_query() {
        assert!(parse_callback_query("/auth/callback").is_none());
    }

    #[test]
    fn chatgpt_account_id_from_synthetic_jwt() {
        // header.payload.sig — only payload matters; unsigned test fixture.
        let payload = URL_SAFE_NO_PAD.encode(
            br#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct-test-9"}}"#,
        );
        let jwt = format!("e30.{payload}.sig");
        assert_eq!(
            chatgpt_account_id_from_id_token(&jwt).as_deref(),
            Some("acct-test-9")
        );
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
