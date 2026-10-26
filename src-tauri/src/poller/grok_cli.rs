//! Grok CLI / OIDC consumer-quota polling (read-only session reference).
//!
//! SECURITY: the bearer token is re-read from the CLI's `auth.json` per poll,
//! never persisted by UsageCheck and never refreshed here (the CLI owns the
//! file). Endpoints are compile-time constants; no env/user value becomes a
//! URL in production builds.

use std::path::Path;

use chrono::{DateTime, Utc};
use usage_core::account::{Account, Credentials};
use usage_core::fetch::grok::GrokPrepaid;

use super::usage_model::AccountUsage;

pub(super) const GROK_CONSUMER_BILLING_URLS: &[&str] = &[
    "https://cli-chat-proxy.grok.com/v1/billing?format=credits",
    "https://cli-chat-proxy.grok.com/v1/billing",
];

pub(super) async fn fetch_grok_consumer_quota_at(
    _client: &reqwest::Client,
    _urls: &[&str],
    _creds: &Credentials,
) -> Result<GrokPrepaid, Option<u16>> {
    Err(None)
}

pub(super) async fn poll_grok_cli_at(
    _client: &reqwest::Client,
    _urls: &[&str],
    _auth_path: Option<&Path>,
    account: &Account,
    _now: DateTime<Utc>,
) -> AccountUsage {
    super::usage_model::account_usage_from_grok(
        account,
        &GrokPrepaid {
            period: None,
            detail_suffix: None,
        },
        "error",
    )
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
