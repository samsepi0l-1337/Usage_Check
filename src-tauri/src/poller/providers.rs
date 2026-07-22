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

/// Identity-checked keychain read, bounded so a stalled auth prompt degrades
/// gracefully (keychain read runs off the async worker, 5s ceiling).
async fn seed_claude_creds_from_keychain(
    profile_root: &Path,
    expected_identity: &str,
) -> Option<Credentials> {
    let root = profile_root.to_path_buf();
    let ident = expected_identity.to_string();
    match tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || {
            import::load_claude_profile_credentials(&root, &ident)
        }),
    )
    .await
    {
        Ok(Ok(creds)) => creds.filter(|c| !c.access_token.is_empty()),
        _ => None,
    }
}

/// Identity-checked live-default-login read, bounded so a stalled auth prompt
/// degrades gracefully. Returned credentials retain live-keychain provenance
/// and must never be passed to the refresh flow.
async fn load_live_claude_creds_from_keychain(expected_identity: &str) -> Option<Credentials> {
    let ident = expected_identity.to_string();
    match tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || import::load_claude_default_login_credentials(&ident)),
    )
    .await
    {
        Ok(Ok(creds)) => creds.filter(|c| !c.access_token.is_empty()),
        _ => None,
    }
}

/// Refreshes near/at expiry (v0.1.0 `maybe_refresh`) persisting the rotated copy
/// to the app-owned cache, then fetches live quota. Keychain never written.
/// `Ok(Live)` on 2xx; `Err(status)` on fetch failure (caller maps to snapshot).
async fn refresh_and_fetch_claude(
    store: &AccountStore,
    account_id: &str,
    client: &reqwest::Client,
    mut creds: Credentials,
    expected_identity: &str,
    force_refresh: bool,
) -> Result<CliProfileOutcome, Option<u16>> {
    if (force_refresh && creds.refresh_token.is_some())
        || crate::oauth::should_refresh(creds.expires_at, Utc::now(), REFRESH_THRESHOLD)
    {
        if let Ok(refreshed) =
            crate::oauth::refresh_access_token(Provider::Claude, &creds).await
        {
            match store.set_cli_profile_credentials(account_id, &refreshed) {
                Ok(()) => {}
                Err(error) => {
                    eprintln!("failed to persist cli-token-cache/{account_id}.json: {error}")
                }
            }
            creds = refreshed;
        }
    }
    match fetch_claude_quota(client, &creds).await {
        Ok(quota) => {
            let email = (!expected_identity.is_empty()).then_some(expected_identity);
            Ok(CliProfileOutcome::Live(claude_fetch_outcome(quota, email)))
        }
        Err(status) => Err(status),
    }
}

fn claude_snapshot_after_fetch_failure(
    snapshot_path: &Path,
    expected_identity: &str,
    status: Option<u16>,
) -> CliProfileOutcome {
    match read_claude_snapshot_outcome(snapshot_path, expected_identity) {
        CliProfileOutcome::WaitingForUsage => {
            CliProfileOutcome::Live(FetchOutcome::Failed { status })
        }
        other => other,
    }
}

/// Polls a Claude CliProfile account by first riding an identity-matching live
/// Claude Code login read-only, then falling back to the app-owned refreshable
/// copy and snapshot paths.
pub(super) async fn poll_claude_cli_profile(
    store: &AccountStore,
    account_id: &str,
    client: &reqwest::Client,
    profile_root: &Path,
    expected_identity: &str,
    snapshot_path: &Path,
) -> CliProfileOutcome {
    // Step A: ride the identity-matching Claude Code login read-only. This
    // credential is never sent through `refresh_and_fetch_claude`.
    let live_creds = load_live_claude_creds_from_keychain(expected_identity).await;
    let mut live_fetch_status = None;
    if let Some(live) = live_creds.as_ref().filter(|credentials| {
        credentials
            .expires_at
            .as_ref()
            .is_none_or(|expires_at| expires_at > &Utc::now())
    }) {
        // Cache the live token WITHOUT its refresh_token so the app-owned path can never rotate a
        // grant Claude Code CLI owns (read-only-ride invariant, structural).
        let cached_live = Credentials {
            refresh_token: None,
            ..live.clone()
        };
        match store.set_cli_profile_credentials(account_id, &cached_live) {
            Ok(()) => {}
            Err(error) => {
                eprintln!("failed to persist cli-token-cache/{account_id}.json: {error}")
            }
        }
        match fetch_claude_quota(client, live).await {
            Ok(quota) => {
                let email = (!expected_identity.is_empty()).then_some(expected_identity);
                return CliProfileOutcome::Live(claude_fetch_outcome(quota, email));
            }
            Err(status) => live_fetch_status = Some(status),
        }
    }

    // Step B: retain the app-owned cache/seed/refresh flow. When a matching
    // live login exists, do not reinterpret the just-cached live token as an
    // app-owned refreshable copy. A distinct per-profile credential may still
    // seed the existing app-owned path.
    let cached = if live_creds.is_some() {
        None
    } else {
        store
            .cli_profile_credentials(account_id)
            .filter(|c| !c.access_token.is_empty())
    };
    let creds = match cached {
        Some(cached) => Some(cached),
        None => {
            let seeded = seed_claude_creds_from_keychain(profile_root, expected_identity)
                .await
                .filter(|seed| {
                    live_creds
                        .as_ref()
                        .is_none_or(|live| seed.access_token.as_str() != live.access_token.as_str())
                });
            if let Some(seed) = &seeded {
                match store.set_cli_profile_credentials(account_id, seed) {
                    Ok(()) => {}
                    Err(error) => {
                        eprintln!("failed to persist cli-token-cache/{account_id}.json: {error}")
                    }
                }
            }
            seeded
        }
    };
    if let Some(creds) = creds {
        match refresh_and_fetch_claude(store, account_id, client, creds, expected_identity, false)
            .await
        {
            Ok(outcome) => return outcome,
            Err(status) => {
                if matches!(status, Some(401) | Some(403)) {
                    if let Some(reseed) =
                        seed_claude_creds_from_keychain(profile_root, expected_identity)
                            .await
                            .filter(|seed| {
                                live_creds.as_ref().is_none_or(|live| {
                                    seed.access_token.as_str() != live.access_token.as_str()
                                })
                            })
                    {
                        match store.set_cli_profile_credentials(account_id, &reseed) {
                            Ok(()) => {}
                            Err(error) => eprintln!(
                                "failed to persist cli-token-cache/{account_id}.json: {error}"
                            ),
                        }
                        if let Ok(outcome) = refresh_and_fetch_claude(
                            store,
                            account_id,
                            client,
                            reseed,
                            expected_identity,
                            true,
                        )
                        .await
                        {
                            return outcome;
                        }
                    }
                }
                return claude_snapshot_after_fetch_failure(
                    snapshot_path,
                    expected_identity,
                    status,
                );
            }
        }
    }
    match live_fetch_status {
        Some(status) => {
            claude_snapshot_after_fetch_failure(snapshot_path, expected_identity, status)
        }
        None => read_claude_snapshot_outcome(snapshot_path, expected_identity),
    }
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
