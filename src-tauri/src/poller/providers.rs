use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use usage_core::account::{Account, Credentials, Provider};
use usage_core::fetch::agy::AgyQuota;
use usage_core::fetch::claude::parse_claude_usage;
use usage_core::models::LocalUsage;

use crate::agy_local;
use crate::import;
use crate::store::AccountStore;

use super::http::{fetch_agy_quota_remote, fetch_claude_quota, fetch_codex_quota};
use super::usage_model::{
    account_usage_from_agy, assemble_account_usage, claude_fetch_outcome, claude_identity_status,
    codex_fetch_outcome, codex_identity_status, status_for_failure, AccountUsage, FetchOutcome,
};

/// Refresh proactively when the access token expires within this window.
const REFRESH_THRESHOLD: Duration = Duration::from_secs(60);
/// If `creds.expires_at` is within `REFRESH_THRESHOLD` of now (or already
/// past), attempts a proactive refresh via `oauth::refresh_access_token` and
/// persists the result via `store.update_credentials`. On any failure
/// (no refresh_token, network error, non-200), the original `creds` are
/// returned unchanged — the existing 401/403 fallback in `poll_all` still
/// applies to a stale token that couldn't be refreshed.
async fn maybe_refresh(
    store: &AccountStore,
    id: &str,
    provider: Provider,
    creds: Credentials,
) -> Credentials {
    if !crate::oauth::should_refresh(creds.expires_at, Utc::now(), REFRESH_THRESHOLD) {
        return creds;
    }

    match crate::oauth::refresh_access_token(provider, &creds).await {
        Ok(refreshed) => {
            let _ = store.update_credentials(id, &refreshed);
            refreshed
        }
        Err(_) => creds,
    }
}

/// Backfills Google `account_id` / email label for legacy agy rows that were
/// saved before identity was persisted (opaque `ya29` tokens, no `id_token`).
async fn enrich_agy_identity(store: &AccountStore, account: &Account, creds: &mut Credentials) {
    let needs_id = creds
        .account_id
        .as_deref()
        .map(|s| s.is_empty())
        .unwrap_or(true);
    let needs_label = !account.label.contains('@');
    if !needs_id && !needs_label {
        return;
    }
    let Some(identity) = crate::oauth::agy_identity_from_access_token(&creds.access_token).await
    else {
        return;
    };
    let mut changed = false;
    if needs_id {
        if let Some(id) = identity.account_id.filter(|s| !s.is_empty()) {
            creds.account_id = Some(id);
            changed = true;
        }
    }
    if changed {
        let _ = store.update_credentials(&account.id, creds);
    }
    if needs_label {
        if let Some(email) = identity.email.filter(|s| !s.is_empty()) {
            store.update_label(&account.id, &email);
        }
    }
}

pub(super) async fn poll_agy(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
    // 1) Prefer live Antigravity.app language_server (same as Model Quota UI).
    if let Some(quota) = agy_local::fetch_local_quota().await {
        if let Some(email) = quota.email.as_deref() {
            store.update_label(&account.id, email);
        }
        return account_usage_from_agy(account, &quota, "ok");
    }

    // 2) OAuth remote Cloud Code fallback.
    match store.credentials(&account.id) {
        Some(creds) if !creds.access_token.is_empty() => {
            let mut creds = maybe_refresh(store, &account.id, Provider::Agy, creds).await;
            enrich_agy_identity(store, account, &mut creds).await;
            match fetch_agy_quota_remote(client, &creds).await {
                Ok(mut quota) => {
                    // Remote summary has no email; use stored label / userinfo.
                    if quota.email.is_none() {
                        let label = store
                            .list()
                            .into_iter()
                            .find(|a| a.id == account.id)
                            .map(|a| a.label)
                            .unwrap_or_else(|| account.label.clone());
                        if label.contains('@') {
                            quota.email = Some(label);
                        }
                    }
                    if let Some(email) = quota.email.as_deref() {
                        store.update_label(&account.id, email);
                    }
                    account_usage_from_agy(account, &quota, "ok")
                }
                Err(status) => account_usage_from_agy(
                    account,
                    &AgyQuota {
                        email: None,
                        plan: None,
                        pools: Vec::new(),
                    },
                    status_for_failure(status),
                ),
            }
        }
        _ => account_usage_from_agy(
            account,
            &AgyQuota {
                email: None,
                plan: None,
                pools: Vec::new(),
            },
            "needs_login",
        ),
    }
}

/// Outcome of reading a CLI-profile provider (Claude snapshot / Codex probe).
/// Shared by both CLI providers so an identity change is never collapsed into a
/// generic `needs_login`. `WaitingForUsage` is Claude-snapshot-specific.
pub(super) enum CliProfileOutcome {
    Live(FetchOutcome),
    WaitingForUsage,
    IdentityChanged,
}

pub(super) fn read_claude_snapshot_outcome(
    snapshot_path: &Path,
    expected_identity: &str,
) -> CliProfileOutcome {
    let Ok(bytes) = std::fs::read(snapshot_path) else {
        return CliProfileOutcome::WaitingForUsage;
    };
    let Ok(root) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return CliProfileOutcome::WaitingForUsage;
    };
    let identity = root
        .get("identity")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if claude_identity_status(identity, expected_identity).is_some() {
        return CliProfileOutcome::IdentityChanged;
    }
    let rate_limits = root
        .get("rate_limits")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let quota = parse_claude_usage(&rate_limits);
    if quota.five_hour.is_none() && quota.week.is_none() {
        return CliProfileOutcome::WaitingForUsage;
    }
    CliProfileOutcome::Live(FetchOutcome::Live {
        five_hour: quota.five_hour,
        week: quota.week,
        plan: None,
        email: (!identity.is_empty()).then(|| identity.to_string()),
    })
}

/// Polls a Claude CliProfile account: PRIMARY = identity-safe local credentials → live HTTP quota;
/// FALLBACK = status-line snapshot. Restores pre-30f525d "read local claude values → show usage".
pub(super) async fn poll_claude_cli_profile(
    client: &reqwest::Client,
    profile_root: &Path,
    expected_identity: &str,
    snapshot_path: &Path,
) -> CliProfileOutcome {
    if let Some(creds) = import::load_claude_profile_credentials(profile_root, expected_identity) {
        if let Ok(quota) = fetch_claude_quota(client, &creds).await {
            let email = (!expected_identity.is_empty()).then_some(expected_identity);
            return CliProfileOutcome::Live(claude_fetch_outcome(quota, email));
        }
    }
    read_claude_snapshot_outcome(snapshot_path, expected_identity)
}

/// Maps a shared CLI-profile outcome to an `AccountUsage`, stamping the
/// non-`Live` statuses. Used by both the Claude and Codex CLI-profile arms.
pub(super) fn assemble_cli_profile_usage(
    account: &Account,
    outcome: CliProfileOutcome,
    local: LocalUsage,
) -> AccountUsage {
    match outcome {
        CliProfileOutcome::Live(fetch) => assemble_account_usage(account, fetch, local),
        CliProfileOutcome::WaitingForUsage => {
            let mut usage =
                assemble_account_usage(account, FetchOutcome::Failed { status: None }, local);
            usage.status = "waiting_for_usage".to_string();
            usage
        }
        CliProfileOutcome::IdentityChanged => {
            let mut usage =
                assemble_account_usage(account, FetchOutcome::Failed { status: None }, local);
            usage.status = "identity_changed".to_string();
            usage
        }
    }
}

pub(super) async fn poll_codex_cli_profile(
    profile_root: &std::path::Path,
    expected_identity: &str,
) -> CliProfileOutcome {
    match crate::codex_cli::probe_codex(profile_root).await {
        Ok(probe) => {
            // A re-logged profile now owning a different identity is a distinct
            // state from an expired credential — surface `identity_changed`, not
            // `needs_login` (parity with the Claude CLI path).
            if codex_identity_status(&probe.account.id, expected_identity).is_some() {
                return CliProfileOutcome::IdentityChanged;
            }
            CliProfileOutcome::Live(FetchOutcome::Live {
                five_hour: probe.primary,
                week: probe.secondary,
                plan: None,
                email: probe.account.email.clone(),
            })
        }
        Err(_) => CliProfileOutcome::Live(FetchOutcome::Failed { status: None }),
    }
}

pub(super) async fn poll_codex_oauth(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> FetchOutcome {
    match store.credentials(&account.id) {
        Some(creds) if !creds.access_token.is_empty() => {
            let creds = maybe_refresh(store, &account.id, Provider::Codex, creds).await;
            match fetch_codex_quota(client, &creds).await {
                Ok(quota) => {
                    if let Some(email) = quota.email.as_deref() {
                        store.update_label(&account.id, email);
                    }
                    codex_fetch_outcome(quota)
                }
                Err(status) => FetchOutcome::Failed { status },
            }
        }
        _ => FetchOutcome::Failed { status: Some(401) },
    }
}

pub(super) async fn poll_claude_oauth(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> FetchOutcome {
    match store.credentials(&account.id) {
        Some(creds) if !creds.access_token.is_empty() => {
            let creds = maybe_refresh(store, &account.id, Provider::Claude, creds).await;
            match fetch_claude_quota(client, &creds).await {
                Ok(quota) => {
                    let email = if account.label.contains('@') {
                        Some(account.label.as_str())
                    } else {
                        None
                    };
                    claude_fetch_outcome(quota, email)
                }
                Err(status) => FetchOutcome::Failed { status },
            }
        }
        _ => FetchOutcome::Failed { status: Some(401) },
    }
}

#[cfg(test)]
#[path = "providers_tests.rs"]
mod tests;
