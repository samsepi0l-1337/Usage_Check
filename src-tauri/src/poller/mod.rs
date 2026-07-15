//! Per-account usage poller: builds an `AccountUsage` snapshot for every
//! account in the `AccountStore`.
//!
//! Codex/Claude: live HTTP quota using the stored access token; on failure
//! falls back to local-log aggregation.
//! Agy: Antigravity Model Quota — prefer the running app's local
//! `RetrieveUserQuotaSummary`, else Cloud Code OAuth remote fetch. No local
//! SQLite token totals (those are not the UI quota %).
//!
//! SECURITY: never log/print an access token or other credential value.

use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use usage_core::account::{Account, AuthSource, Credentials, Provider};
use usage_core::fetch::agy::AgyQuota;
use usage_core::fetch::claude::parse_claude_usage;
use usage_core::models::{LocalProvenance, LocalUsage};

use crate::agy_local;
use crate::paths;
use crate::store::AccountStore;

mod http;
use http::{fetch_agy_quota_remote, fetch_claude_quota, fetch_codex_quota};

mod local_scan;
use local_scan::local_usage_for_provider;

mod usage_model;
#[cfg(feature = "edition-pro")]
mod providers_pro;
#[cfg(feature = "edition-pro")]
use providers_pro::{poll_cursor, poll_grok, poll_higgsfield};
pub use usage_model::{
    account_usage_from_agy,
    assemble_account_usage,
    AccountUsage,
    FetchOutcome,
};
use usage_model::{
    claude_fetch_outcome,
    claude_identity_status,
    codex_fetch_outcome,
    codex_identity_status,
    status_for_failure,
};

mod last_success;
pub use last_success::evict_last_success;
use last_success::{apply_last_success, last_success_cache};

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
    let Some(identity) = crate::oauth::agy_identity_from_access_token(&creds.access_token).await else {
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

async fn poll_agy(
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
enum CliProfileOutcome {
    Live(FetchOutcome),
    WaitingForUsage,
    IdentityChanged,
}

fn read_claude_snapshot_outcome(snapshot_path: &Path, expected_identity: &str) -> CliProfileOutcome {
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

/// Maps a shared CLI-profile outcome to an `AccountUsage`, stamping the
/// non-`Live` statuses. Used by both the Claude and Codex CLI-profile arms.
fn assemble_cli_profile_usage(
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

async fn poll_codex_cli_profile(
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

async fn poll_codex_oauth(
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

async fn poll_claude_oauth(
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


/// Builds the full per-account usage snapshot.
pub async fn poll_all(store: &AccountStore) -> Vec<AccountUsage> {
    let client = reqwest::Client::new();
    let accounts = store.list();
    let mut out = Vec::with_capacity(accounts.len());
    let now = Utc::now();
    let mut codex_local = local_usage_for_provider(store, &accounts, Provider::Codex, now).await;
    let mut claude_local = local_usage_for_provider(store, &accounts, Provider::Claude, now).await;

    for account in accounts {
        let usage = match account.provider {
            Provider::Agy => poll_agy(store, &client, &account).await,
            Provider::Codex => {
                let local = codex_local
                    .remove(&account.id)
                    .unwrap_or_else(|| LocalUsage::none(LocalProvenance::NoLocalProfile));
                match &account.auth_source {
                    AuthSource::CliProfile {
                        profile_root,
                        expected_identity,
                        ..
                    } => {
                        let outcome =
                            poll_codex_cli_profile(profile_root, expected_identity).await;
                        assemble_cli_profile_usage(&account, outcome, local)
                    }
                    _ => {
                        let outcome = poll_codex_oauth(store, &client, &account).await;
                        assemble_account_usage(&account, outcome, local)
                    }
                }
            }
            Provider::Claude => {
                let local = claude_local
                    .remove(&account.id)
                    .unwrap_or_else(|| LocalUsage::none(LocalProvenance::NoLocalProfile));
                match &account.auth_source {
                    AuthSource::CliProfile {
                        expected_identity, ..
                    } => {
                        let outcome = read_claude_snapshot_outcome(
                            &paths::claude_statusline_snapshot(&account.id),
                            expected_identity,
                        );
                        assemble_cli_profile_usage(&account, outcome, local)
                    }
                    _ => {
                        let outcome = poll_claude_oauth(store, &client, &account).await;
                        assemble_account_usage(&account, outcome, local)
                    }
                }
            }
            #[cfg(feature = "edition-pro")]
            Provider::Cursor => poll_cursor(store, &client, &account).await,
            #[cfg(feature = "edition-pro")]
            Provider::Grok => poll_grok(store, &client, &account).await,
            #[cfg(feature = "edition-pro")]
            Provider::Higgsfield => poll_higgsfield(store, &account).await,
        };
        let usage = {
            let mut cache = last_success_cache()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            apply_last_success(&mut cache, &account.id, usage)
        };
        out.push(usage);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    #[test]
    fn auth_source_claude_snapshot_missing_is_waiting() {
        use std::path::Path;
        assert!(matches!(
            read_claude_snapshot_outcome(Path::new("/nonexistent"), "id"),
            CliProfileOutcome::WaitingForUsage
        ));
    }

    #[test]
    fn auth_source_claude_snapshot_live() {
        let temp = TempDir::new().expect("create temp directory");
        let snapshot = temp.path().join("snapshot.json");
        std::fs::write(
            &snapshot,
            r#"{"identity":"id","rate_limits":{"five_hour":{"utilization":30.0},"seven_day":{"utilization":55.0}}}"#,
        )
        .expect("write snapshot");

        let CliProfileOutcome::Live(FetchOutcome::Live {
            five_hour,
            week,
            plan,
            email,
        }) = read_claude_snapshot_outcome(&snapshot, "id")
        else {
            panic!("expected live Claude snapshot outcome");
        };

        assert_eq!(five_hour.map(|quota| quota.percent), Some(30.0));
        assert_eq!(week.map(|quota| quota.percent), Some(55.0));
        assert_eq!(plan, None);
        assert_eq!(email.as_deref(), Some("id"));
    }

    #[test]
    fn auth_source_claude_snapshot_identity_mismatch() {
        let temp = TempDir::new().expect("create temp directory");
        let snapshot = temp.path().join("snapshot.json");
        std::fs::write(
            &snapshot,
            r#"{"identity":"other","rate_limits":{"five_hour":{"utilization":30.0}}}"#,
        )
        .expect("write snapshot");

        assert!(matches!(
            read_claude_snapshot_outcome(&snapshot, "id"),
            CliProfileOutcome::IdentityChanged
        ));
    }

}


