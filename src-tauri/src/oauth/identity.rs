use std::time::Duration;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

/// Token-endpoint response shape (Codex/Claude both return this standard
/// OAuth2 token response body).
#[derive(serde::Deserialize)]
pub(crate) struct TokenResponse {
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
    #[serde(default)]
    pub(crate) id_token: Option<String>,
    #[serde(default)]
    pub(crate) expires_in: Option<i64>,
    #[serde(default)]
    pub(crate) account_id: Option<String>,
}

/// Decodes a JWT payload object without verifying the signature. Pure — never
/// logs the token. Used only to read non-secret identity claims (`sub`, email,
/// ChatGPT account id).
fn jwt_payload_json(jwt: &str) -> Option<serde_json::Value> {
    let payload_b64 = jwt.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Extracts `chatgpt_account_id` from a ChatGPT id_token JWT payload
/// (`https://api.openai.com/auth` claim). Pure — no I/O, never logs the token.
pub fn chatgpt_account_id_from_id_token(id_token: &str) -> Option<String> {
    jwt_payload_json(id_token)?
        .get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Extracts Google account `sub` from an OpenID `id_token`. Pure — never logs.
pub fn google_sub_from_id_token(id_token: &str) -> Option<String> {
    jwt_payload_json(id_token)?
        .get("sub")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}
/// Resolves a stable provider account id from a token response: ChatGPT
/// account id when present, otherwise Google `sub` (Antigravity).
pub(crate) fn account_id_from_token_response(body: &TokenResponse) -> Option<String> {
    body.account_id
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            body.id_token
                .as_deref()
                .and_then(chatgpt_account_id_from_id_token)
        })
        .or_else(|| body.id_token.as_deref().and_then(google_sub_from_id_token))
}

/// Google account identity from `userinfo` (email + numeric `id` / `sub`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgyGoogleIdentity {
    pub email: Option<String>,
    pub account_id: Option<String>,
}

/// Fetches Google identity for an Antigravity access token via `userinfo`.
/// Best-effort — returns `None` on any failure. Never logs the token.
pub async fn agy_identity_from_access_token(access_token: &str) -> Option<AgyGoogleIdentity> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;
    let resp = client
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .bearer_auth(access_token)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    let email = body
        .get("email")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let account_id = body
        .get("id")
        .or_else(|| body.get("sub"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if email.is_none() && account_id.is_none() {
        return None;
    }
    Some(AgyGoogleIdentity { email, account_id })
}

/// Fetches the Google account email for an Antigravity access token via
/// `userinfo`. Best-effort — returns `None` on any failure. Never logs the token.
pub async fn agy_email_from_access_token(access_token: &str) -> Option<String> {
    agy_identity_from_access_token(access_token)
        .await
        .and_then(|id| id.email)
}
