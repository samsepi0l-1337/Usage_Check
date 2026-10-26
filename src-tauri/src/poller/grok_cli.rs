//! Grok CLI / OIDC consumer-quota polling (read-only session reference).
//!
//! SECURITY: the bearer token is re-read from the CLI's `auth.json` per poll,
//! never persisted by UsageCheck and never refreshed here (the CLI owns the
//! file). Endpoints are compile-time constants; no env/user value becomes a
//! URL in production builds.

use std::path::Path;

use chrono::{DateTime, Utc};
use usage_core::account::{Account, AuthSource, Credentials};
use usage_core::fetch::grok::{parse_grok_consumer_credits, GrokPrepaid};

use super::usage_model::{account_usage_from_grok, status_for_failure, AccountUsage};
use crate::import::read_grok_cli_credentials;

pub(super) const GROK_CONSUMER_BILLING_URLS: &[&str] = &[
    "https://cli-chat-proxy.grok.com/v1/billing?format=credits",
    "https://cli-chat-proxy.grok.com/v1/billing",
];

/// Consumer/SuperGrok quota for an OIDC access token. Never calls
/// `management-api.x.ai` (CLI tokens are rejected there). `Err` carries the
/// last HTTP status, or `None` for transport failures; 401/403 stop immediately.
pub(super) async fn fetch_grok_consumer_quota_at(
    client: &reqwest::Client,
    urls: &[&str],
    creds: &Credentials,
) -> Result<GrokPrepaid, Option<u16>> {
    let mut last_status: Option<u16> = None;
    for url in urls {
        let resp = client
            .get(*url)
            .header("Accept", "application/json")
            .header("User-Agent", "UsageCheck")
            .header("x-xai-token-auth", "xai-grok-cli")
            .bearer_auth(&creds.access_token)
            .send()
            .await
            .map_err(|_| None)?;
        let status = resp.status();
        if matches!(status.as_u16(), 401 | 403) {
            return Err(Some(status.as_u16()));
        }
        if !status.is_success() {
            last_status = Some(status.as_u16());
            continue;
        }
        let body: serde_json::Value = resp.json().await.map_err(|_| Some(status.as_u16()))?;
        return Ok(parse_grok_consumer_credits(&body));
    }
    Err(last_status)
}

fn grok_cli_failure(account: &Account, status: &str) -> AccountUsage {
    account_usage_from_grok(
        account,
        &GrokPrepaid {
            period: None,
            detail_suffix: Some("Grok quota unavailable".into()),
        },
        status,
    )
}

/// Polls one Grok CLI session reference. Any unusable local session (missing
/// or malformed file, other identity, expired token) is `needs_login` with no
/// network call; the user re-runs `grok login` and the CLI renews its file.
pub(super) async fn poll_grok_cli_at(
    client: &reqwest::Client,
    urls: &[&str],
    auth_path: Option<&Path>,
    account: &Account,
    now: DateTime<Utc>,
) -> AccountUsage {
    let AuthSource::GrokCli { expected_identity } = &account.auth_source else {
        return grok_cli_failure(account, "needs_login");
    };
    let Some(path) = auth_path else {
        return grok_cli_failure(account, "needs_login");
    };
    let creds = match read_grok_cli_credentials(path, expected_identity, now) {
        Ok(creds) => creds,
        Err(_) => return grok_cli_failure(account, "needs_login"),
    };
    match fetch_grok_consumer_quota_at(client, urls, &creds).await {
        Ok(prepaid) if prepaid.period.is_some() => account_usage_from_grok(account, &prepaid, "ok"),
        Ok(prepaid) => account_usage_from_grok(account, &prepaid, "needs_setup"),
        Err(status) => grok_cli_failure(account, status_for_failure(status)),
    }
}

pub(super) async fn poll_grok_cli(client: &reqwest::Client, account: &Account) -> AccountUsage {
    let path = crate::paths::grok_auth_file();
    poll_grok_cli_at(
        client,
        GROK_CONSUMER_BILLING_URLS,
        path.as_deref(),
        account,
        Utc::now(),
    )
    .await
}

#[cfg(test)]
#[path = "grok_cli_tests.rs"]
mod tests;
