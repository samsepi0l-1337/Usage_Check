use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use usage_core::account::{Account, AuthSource, Credentials, Provider};
use usage_core::fetch::agy::AgyQuota;
use usage_core::fetch::claude::parse_claude_usage;
use usage_core::models::LocalUsage;

use crate::agy_local;
use crate::claude_statusline;
use crate::import;
use crate::store::AccountStore;

use super::http::{
    fetch_agy_quota_remote, fetch_claude_quota, fetch_codex_quota_at, CODEX_USAGE_URL,
};
use super::usage_model::{
    account_usage_from_agy, assemble_account_usage, claude_fetch_outcome, claude_identity_status,
    codex_fetch_outcome, codex_identity_status, looks_like_email, status_for_failure, AccountUsage,
    FetchOutcome,
};

/// Refresh proactively when the access token expires within this window.
const REFRESH_THRESHOLD: Duration = Duration::from_secs(60);
/// If the token is expired or empty (see `creds_need_refresh`), attempts a
/// refresh via `oauth::refresh_access_token` and persists the result.
/// On any failure the original `creds` are returned unchanged. Codex OAuth
/// inspects refresh errors itself so a still-valid identity is not
/// collapsed into a silent 401 `needs_login`.
pub(crate) async fn maybe_refresh(
    store: &AccountStore,
    credential_id: &str,
    provider: Provider,
    creds: Credentials,
) -> Credentials {
    if !crate::oauth::creds_need_refresh(&creds, Utc::now(), REFRESH_THRESHOLD) {
        return creds;
    }

    match crate::oauth::refresh_access_token(provider, &creds).await {
        Ok(refreshed) => {
            let _ = store.update_credentials(credential_id, &refreshed);
            refreshed
        }
        Err(_) => creds,
    }
}

async fn refresh_now(
    provider: Provider,
    creds: &Credentials,
    token_url: Option<&str>,
) -> Result<Credentials, crate::oauth::TokenRefreshError> {
    match token_url {
        Some(url) => crate::oauth::refresh_access_token_at(url, provider, creds).await,
        None => crate::oauth::refresh_access_token(provider, creds).await,
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
        let _ = store.update_credentials(AccountStore::credential_key(account), creds);
    }
    if needs_label {
        if let Some(email) = identity.email.filter(|s| !s.is_empty()) {
            store.update_label(&account.id, &email);
        }
    }
}

/// Whether a machine-local Antigravity quota may be attributed to this account.
/// Fails closed: an unverifiable identity is only accepted when the account is the
/// only agy account, so one desktop login is never shown as two different accounts.
fn agy_local_quota_matches_account(
    local_email: Option<&str>,
    account_label: &str,
    agy_account_count: usize,
) -> bool {
    match local_email {
        Some(local_email) => {
            if account_label.contains('@') {
                // Account identity is known — only attribute the local quota when it
                // actually belongs to this account.
                account_label.eq_ignore_ascii_case(local_email)
            } else {
                // Account identity is unverified (e.g. freshly added). Only unambiguous
                // when it is the sole agy account in the store.
                agy_account_count <= 1
            }
        }
        // Local quota carries no identity at all — cannot verify, so only accept it
        // when there is no other agy account it could be confused with.
        None => agy_account_count <= 1,
    }
}

pub(super) async fn poll_agy(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
    poll_agy_with_local_quota(store, client, account, agy_local::fetch_local_quota().await).await
}

/// Core of `poll_agy`, parameterized on the local-quota probe result so the
/// GAP 3 reject→evict wiring below is unit-testable at the poller level
/// without driving the real `agy_local::fetch_local_quota()` probe (a `ps`
/// process scan plus a local loopback HTTPS call — not mockable in a unit
/// test).
async fn poll_agy_with_local_quota(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
    local_quota: Option<AgyQuota>,
) -> AccountUsage {
    // 1) Prefer live Antigravity.app language_server (same as Model Quota UI), but
    // only when the probed identity can be safely attributed to this account —
    // `fetch_local_quota` has no account argument, so with 2+ agy accounts a
    // mismatched or unverifiable local login must not be shown as this account's
    // usage (see `agy_local_quota_matches_account`).
    if let Some(quota) = local_quota {
        let agy_account_count = store
            .list()
            .iter()
            .filter(|a| a.provider == Provider::Agy)
            .count();
        if agy_local_quota_matches_account(
            quota.email.as_deref(),
            &account.label,
            agy_account_count,
        ) {
            if let Some(email) = quota.email.as_deref() {
                store.update_label(&account.id, email);
            }
            return account_usage_from_agy(account, &quota, "ok");
        }
        // GAP 3 fix: the local quota could not be safely attributed to this
        // account (ambiguous now that 2+ agy accounts exist). Any previously
        // remembered `last_success` value may have been cached while this
        // account was still the sole agy account — it is now equally
        // ambiguous, so evict it here rather than let a subsequent OAuth
        // failure re-serve it as `stale`. Do NOT evict when the local quota
        // was accepted above, and this branch is only reached when a local
        // quota was actually probed (never on `None`, i.e. no local path).
        crate::poller::evict_last_success(&account.id);
    }

    // 2) OAuth remote Cloud Code fallback.
    match store.credentials(AccountStore::credential_key(account)) {
        Some(creds) if !creds.access_token.is_empty() => {
            let mut creds = maybe_refresh(
                store,
                AccountStore::credential_key(account),
                Provider::Agy,
                creds,
            )
            .await;
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

/// Reads a Claude usage snapshot and maps it to a `CliProfileOutcome`.
///
/// GAP 2 fix: a snapshot's `source` field distinguishes provenance — `"bridge"`
/// (written by `claude_statusline::run_bridge`, i.e. Claude Code itself running
/// under this specific managed profile — genuinely per-account) from
/// `"live-ride"` (self-seeded from a possibly-unverified live-ride fetch, which
/// may hold another account's numbers under this account's identity) and legacy
/// snapshots with no `source` at all. `trust_unverified_source` gates this:
/// when `false` (2+ Claude CliProfile accounts), only `"bridge"` snapshots are
/// accepted — `"live-ride"` and source-less snapshots fail closed to
/// `WaitingForUsage` rather than risk showing a possibly-wrong number. When
/// `true` (sole account), behavior is unchanged from before this fix, including
/// legacy source-less snapshots.
pub(super) fn read_claude_snapshot_outcome(
    snapshot_path: &Path,
    expected_identity: &str,
    trust_unverified_source: bool,
) -> CliProfileOutcome {
    let Ok(bytes) = std::fs::read(snapshot_path) else {
        return CliProfileOutcome::WaitingForUsage;
    };
    let Ok(root) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return CliProfileOutcome::WaitingForUsage;
    };
    if !trust_unverified_source {
        let source = root.get("source").and_then(serde_json::Value::as_str);
        if source != Some("bridge") {
            return CliProfileOutcome::WaitingForUsage;
        }
    }
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
        // Snapshot identity is often the accountUuid anchor, not an email.
        email: looks_like_email(identity).then(|| identity.trim().to_lowercase()),
        breakdown: quota.breakdown,
    })
}

/// Email label for a Claude CLI account: stored email, else `.claude.json`
/// `oauthAccount.emailAddress` when that file's identity matches `expected`.
fn claude_email_for_display(profile_root: &Path, expected_identity: &str) -> Option<String> {
    let from_root = |root: &Path| -> Option<String> {
        let (email, account_uuid, org_uuid) = import::claude_oauth_identity_set_in(root);
        let email = email.filter(|s| looks_like_email(s))?;
        let email = email.trim().to_lowercase();
        let expected = expected_identity.trim();
        if expected.is_empty()
            || expected.eq_ignore_ascii_case(&email)
            || account_uuid.as_deref() == Some(expected)
            || org_uuid.as_deref() == Some(expected)
        {
            Some(email)
        } else {
            None
        }
    };
    if let Some(email) = from_root(profile_root) {
        return Some(email);
    }
    let expected = expected_identity.trim();
    if expected.is_empty() {
        return None;
    }
    crate::paths::claude_config_roots()
        .into_iter()
        .find_map(|root| from_root(&root))
        .or_else(|| looks_like_email(expected).then(|| expected.to_lowercase()))
}

fn claude_email_for_account(account: &Account) -> Option<String> {
    if looks_like_email(&account.label) {
        return Some(account.label.trim().to_lowercase());
    }
    match &account.auth_source {
        AuthSource::CliProfile {
            profile_root,
            expected_identity,
            ..
        } => claude_email_for_display(profile_root, expected_identity),
        _ => None,
    }
}

fn apply_claude_display_email(
    outcome: CliProfileOutcome,
    email: Option<&str>,
) -> CliProfileOutcome {
    match outcome {
        CliProfileOutcome::Live(FetchOutcome::Live {
            five_hour,
            week,
            plan,
            email: existing,
            breakdown,
        }) => {
            let email = existing
                .filter(|s| looks_like_email(s))
                .or_else(|| email.map(str::to_string));
            CliProfileOutcome::Live(FetchOutcome::Live {
                five_hour,
                week,
                plan,
                email,
                breakdown,
            })
        }
        other => other,
    }
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
        tokio::task::spawn_blocking(move || import::load_claude_profile_credentials(&root, &ident)),
    )
    .await
    {
        Ok(Ok(creds)) => creds.filter(|c| !c.access_token.is_empty()),
        _ => None,
    }
}

/// Identity-checked live-default-login read, bounded so a stalled auth prompt
/// degrades gracefully. Returned credentials retain live-keychain provenance
/// and must never be passed to the refresh flow. `allow_unverified_ride` gates
/// the unverifiable-identity fallback — see
/// `import::load_claude_default_login_credentials`.
async fn load_live_claude_creds_from_keychain(
    expected_identity: &str,
    allow_unverified_ride: bool,
) -> Option<Credentials> {
    let ident = expected_identity.to_string();
    match tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || {
            import::load_claude_default_login_credentials(&ident, allow_unverified_ride)
        }),
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
        || crate::oauth::creds_need_refresh(&creds, Utc::now(), REFRESH_THRESHOLD)
    {
        if let Ok(refreshed) = crate::oauth::refresh_access_token(Provider::Claude, &creds).await {
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
            if let Err(error) =
                claude_statusline::write_usage_snapshot(account_id, expected_identity, &quota)
            {
                eprintln!("failed to persist Claude usage snapshot for {account_id}: {error}");
            }
            let email = looks_like_email(expected_identity).then_some(expected_identity);
            Ok(CliProfileOutcome::Live(claude_fetch_outcome(quota, email)))
        }
        Err(status) => Err(status),
    }
}

fn claude_snapshot_after_fetch_failure(
    snapshot_path: &Path,
    expected_identity: &str,
    status: Option<u16>,
    trust_unverified_source: bool,
) -> CliProfileOutcome {
    match read_claude_snapshot_outcome(snapshot_path, expected_identity, trust_unverified_source) {
        CliProfileOutcome::WaitingForUsage => {
            CliProfileOutcome::Live(FetchOutcome::Failed { status })
        }
        other => other,
    }
}

/// Whether Step B of `poll_claude_cli_profile` may read the app-owned
/// CLI-profile token cache. Pure decision, extracted so the GAP 1 fix is
/// unit-testable directly: with a matching live login already ridden this
/// step (`live_creds_present`), the cache must not be reinterpreted as an
/// app-owned refreshable copy (pre-existing behavior); with 2+ Claude
/// CliProfile accounts (`!sole_claude_cli_profile`), the cache may hold a
/// live-ride token cached earlier while this account was still the sole
/// account, which is no longer safe to trust — see the call site for the
/// full rationale.
fn claude_cli_profile_cache_is_trusted(
    live_creds_present: bool,
    sole_claude_cli_profile: bool,
) -> bool {
    !live_creds_present && sole_claude_cli_profile
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
    // credential is never sent through `refresh_and_fetch_claude`. The
    // unverified-identity ride (Claude Code CLI often persists no identity) is
    // only unambiguous when this is the sole Claude CliProfile account in the
    // store — otherwise every such account would ride the SAME live login.
    let sole_claude_cli_profile = store
        .list()
        .iter()
        .filter(|a| {
            a.provider == Provider::Claude && matches!(a.auth_source, AuthSource::CliProfile { .. })
        })
        .count()
        <= 1;
    if let Some(email) = claude_email_for_display(profile_root, expected_identity) {
        store.update_label(account_id, &email);
    }
    let live_creds =
        load_live_claude_creds_from_keychain(expected_identity, sole_claude_cli_profile).await;
    let mut live_fetch_status = None;
    // Ride even when `expires_at` is past: the usage API is the source of
    // truth, and a still-accepted token is how Fable's weekly cap is read.
    // 401/403 fall through to the app-owned cache / snapshot paths below.
    if let Some(live) = live_creds.as_ref() {
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
                if let Err(error) =
                    claude_statusline::write_usage_snapshot(account_id, expected_identity, &quota)
                {
                    eprintln!("failed to persist Claude usage snapshot for {account_id}: {error}");
                }
                let email = looks_like_email(expected_identity).then_some(expected_identity);
                return CliProfileOutcome::Live(claude_fetch_outcome(quota, email));
            }
            Err(status) => live_fetch_status = Some(status),
        }
    }

    // Step B: retain the app-owned cache/seed/refresh flow. When a matching
    // live login exists, do not reinterpret the just-cached live token as an
    // app-owned refreshable copy. A distinct per-profile credential may still
    // seed the existing app-owned path.
    //
    // GAP 1 fix: with 2+ Claude CliProfile accounts, the app-owned cache may
    // hold a live-ride token that Step A cached earlier — while this account
    // was still (unambiguously) the sole account. Once a second account exists
    // that ride is refused going forward, but the cached token would otherwise
    // keep silently serving the earlier live login's usage. Skip the cache
    // read entirely in that case and go straight to the identity-checked
    // per-profile keychain seed below, which verifies identity against the
    // profile's own `.claude.json`.
    let cached =
        if claude_cli_profile_cache_is_trusted(live_creds.is_some(), sole_claude_cli_profile) {
            store
                .cli_profile_credentials(account_id)
                .filter(|c| !c.access_token.is_empty())
        } else {
            None
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
                    sole_claude_cli_profile,
                );
            }
        }
    }
    match live_fetch_status {
        Some(status) => claude_snapshot_after_fetch_failure(
            snapshot_path,
            expected_identity,
            status,
            sole_claude_cli_profile,
        ),
        None => {
            read_claude_snapshot_outcome(snapshot_path, expected_identity, sole_claude_cli_profile)
        }
    }
}

/// Maps a shared CLI-profile outcome to an `AccountUsage`, stamping the
/// non-`Live` statuses. Used by both the Claude and Codex CLI-profile arms.
pub(super) fn assemble_cli_profile_usage(
    account: &Account,
    outcome: CliProfileOutcome,
    local: LocalUsage,
) -> AccountUsage {
    let email = (account.provider == Provider::Claude)
        .then(|| claude_email_for_account(account))
        .flatten();
    let outcome = if account.provider == Provider::Claude {
        apply_claude_display_email(outcome, email.as_deref())
    } else {
        outcome
    };
    let mut usage = match outcome {
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
    };
    if let Some(email) = email {
        usage.display_name = email.clone();
        usage.account.label = email;
    }
    usage
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
                breakdown: Vec::new(),
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
    poll_codex_oauth_at(store, client, account, None, CODEX_USAGE_URL).await
}

/// Codex browser-OAuth poll with injectable token/usage URLs (unit tests).
///
/// A stored account with a `refresh_token` is refreshed when the access
/// token is empty, expired, or rejected with 401/403. Permanent auth
/// failures (no/invalid grant) stay `needs_login`; transport/parse
/// failures on refresh surface as `error` so a still-valid identity is
/// not collapsed into a silent re-login prompt.
async fn poll_codex_oauth_at(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
    token_url: Option<&str>,
    usage_url: &str,
) -> FetchOutcome {
    let Some(creds) = store.credentials(AccountStore::credential_key(account)) else {
        return FetchOutcome::Failed { status: Some(401) };
    };
    let has_refresh = creds
        .refresh_token
        .as_deref()
        .is_some_and(|token| !token.is_empty());
    if creds.access_token.is_empty() && !has_refresh {
        return FetchOutcome::Failed { status: Some(401) };
    }

    let credential_id = AccountStore::credential_key(account);
    let creds = if creds.access_token.is_empty()
        || crate::oauth::creds_need_refresh(&creds, Utc::now(), REFRESH_THRESHOLD)
    {
        match persist_codex_refresh(store, credential_id, &creds, token_url).await {
            Ok(refreshed) => refreshed,
            Err(error) if creds.access_token.is_empty() => {
                return if error.is_permanent_auth_failure() {
                    FetchOutcome::Failed { status: Some(401) }
                } else {
                    FetchOutcome::Failed { status: None }
                };
            }
            Err(_) => creds,
        }
    } else {
        creds
    };

    match fetch_codex_quota_at(client, &creds, usage_url).await {
        Ok(quota) => live_codex_quota(store, account, quota),
        Err(status) if matches!(status, Some(401) | Some(403)) && has_refresh => {
            match persist_codex_refresh(store, credential_id, &creds, token_url).await {
                Ok(refreshed) => match fetch_codex_quota_at(client, &refreshed, usage_url).await {
                    Ok(quota) => live_codex_quota(store, account, quota),
                    Err(retry_status) => FetchOutcome::Failed {
                        status: retry_status,
                    },
                },
                Err(error) if error.is_permanent_auth_failure() => {
                    FetchOutcome::Failed { status: Some(401) }
                }
                Err(_) => FetchOutcome::Failed { status: None },
            }
        }
        Err(status) => FetchOutcome::Failed { status },
    }
}

async fn persist_codex_refresh(
    store: &AccountStore,
    credential_id: &str,
    creds: &Credentials,
    token_url: Option<&str>,
) -> Result<Credentials, crate::oauth::TokenRefreshError> {
    let refreshed = refresh_now(Provider::Codex, creds, token_url).await?;
    let _ = store.update_credentials(credential_id, &refreshed);
    Ok(refreshed)
}

fn live_codex_quota(
    store: &AccountStore,
    account: &Account,
    quota: usage_core::fetch::codex::CodexQuota,
) -> FetchOutcome {
    if let Some(email) = quota.email.as_deref() {
        store.update_label(&account.id, email);
    }
    codex_fetch_outcome(quota)
}

pub(super) async fn poll_claude_oauth(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> FetchOutcome {
    match store.credentials(AccountStore::credential_key(account)) {
        Some(creds) if !creds.access_token.is_empty() => {
            let creds = maybe_refresh(
                store,
                AccountStore::credential_key(account),
                Provider::Claude,
                creds,
            )
            .await;
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
