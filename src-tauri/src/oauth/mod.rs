//! PKCE OAuth manager: pure helpers (PKCE generation, authorize-URL assembly,
//! per-provider config) plus a live `begin_login` flow that opens the system
//! browser, runs a localhost callback server, and exchanges the code for
//! `Credentials`.
//!
//! SECURITY: never log/print the verifier, authorization code, access_token,
//! or refresh_token. Error strings must not embed token/secret values.

use std::time::Duration;

use chrono::Utc;
use tiny_http::Response;
use usage_core::account::{Credentials, Provider};

#[cfg(test)]
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

mod google_secret;
mod flow;
mod identity;

#[allow(unused_imports)]
pub(crate) use google_secret::{resolve_agy_oauth_client, extract_google_oauth_pair};
#[allow(unused_imports)]
pub(crate) use flow::{make_pkce, build_authorize_url, parse_callback_query};
#[allow(unused_imports)]
pub(crate) use identity::{account_id_from_token_response, chatgpt_account_id_from_id_token, google_sub_from_id_token, agy_identity_from_access_token, agy_email_from_access_token, TokenResponse};

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
const GOOGLE_CLIENT_SUFFIX: &[u8] = b".apps.googleusercontent.com";
const GOCSPX_PREFIX: &[u8] = b"GOCSPX-";
/// Google Cloud Console registration used by Antigravity-Manager / agy tools.
const AGY_CALLBACK_PORT: u16 = 8080;
const AGY_CALLBACK_PATH: &str = "/callback";

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
        // Scopes must include `user:sessions:claude_code` (same as Claude Code
        // Keychain tokens) or the oauth/usage endpoint rejects the token.
        Provider::Claude => Ok(ProviderOAuth {
            client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e".to_string(),
            client_secret: None,
            auth_url: "https://console.anthropic.com/oauth/authorize".to_string(),
            token_url: "https://console.anthropic.com/v1/oauth/token".to_string(),
            scopes: "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload"
                .to_string(),
            fixed_redirect: None,
            extra_authorize_params: Vec::new(),
            use_pkce: true,
        }),
        // Google OAuth for Antigravity — credentials from env or local install.
        // Redirect must be the registered loopback URI
        // `http://localhost:8080/callback` (same as Antigravity-Manager).
        Provider::Agy => {
            let (client_id, client_secret) = resolve_agy_oauth_client()?;
            Ok(ProviderOAuth {
                client_id,
                client_secret: Some(client_secret),
                auth_url: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
                token_url: "https://oauth2.googleapis.com/token".to_string(),
                scopes: AGY_SCOPES.to_string(),
                fixed_redirect: Some((AGY_CALLBACK_PORT, AGY_CALLBACK_PATH)),
                extra_authorize_params: vec![
                    ("access_type".into(), "offline".into()),
                    ("prompt".into(), "consent".into()),
                    ("include_granted_scopes".into(), "true".into()),
                ],
                use_pkce: false,
            })
        }
        #[cfg(feature = "edition-pro")]
        Provider::Cursor | Provider::Grok | Provider::Higgsfield => Err(format!(
            "{} uses local import — choose Import from the tray Add Account menu",
            provider.display_name()
        )),
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

    let (server, redirect_uri) = flow::bind_callback_server(&cfg)?;

    let (verifier, challenge) = make_pkce();
    let state = flow::make_state();
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

    let mut account_id = account_id_from_token_response(&body);
    // Google access tokens are opaque (`ya29…`); when `id_token` is absent,
    // resolve a stable `sub`/`id` via userinfo so re-login can upsert by
    // identity after a terminal account switch.
    if account_id.is_none() && provider == Provider::Agy {
        if let Some(identity) = agy_identity_from_access_token(&body.access_token).await {
            account_id = identity.account_id;
        }
    }

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

    let account_id = account_id_from_token_response(&body).or_else(|| creds.account_id.clone());

    Ok(Credentials {
        access_token: body.access_token,
        refresh_token: body.refresh_token.or_else(|| creds.refresh_token.clone()),
        account_id,
        expires_at,
    })
}

#[cfg(test)]
#[path = "../oauth_tests.rs"]
mod tests;
