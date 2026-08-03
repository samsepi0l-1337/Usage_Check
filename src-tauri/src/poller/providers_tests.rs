use super::*;
use serde_json::json;
use std::ffi::OsString;
use std::path::Path;
use tempfile::TempDir;
use usage_core::fetch::claude::ClaudeQuota;
use usage_core::models::QuotaUsage;

struct ClaudeConfigDirGuard(Option<OsString>);

impl ClaudeConfigDirGuard {
    fn set(path: &Path) -> Self {
        let previous = std::env::var_os("CLAUDE_CONFIG_DIR");
        std::env::set_var("CLAUDE_CONFIG_DIR", path);
        Self(previous)
    }

    fn unset() -> Self {
        let previous = std::env::var_os("CLAUDE_CONFIG_DIR");
        std::env::remove_var("CLAUDE_CONFIG_DIR");
        Self(previous)
    }
}

impl Drop for ClaudeConfigDirGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(previous) => std::env::set_var("CLAUDE_CONFIG_DIR", previous),
            None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
        }
    }
}

#[test]
fn cli_profile_rate_limited_failure_is_assembled_as_throttled() {
    let account = usage_core::account::Account {
        id: "claude-cli".into(),
        provider: usage_core::account::Provider::Claude,
        label: "user@example.com".into(),
        auth_source: usage_core::account::AuthSource::CliProfile {
            profile_root: std::path::PathBuf::from("/profile"),
            ownership: usage_core::account::ProfileOwnership::External,
            expected_identity: "user@example.com".into(),
        },
    };
    let local = LocalUsage::none(usage_core::models::LocalProvenance::NoLocalProfile);

    let usage = assemble_cli_profile_usage(
        &account,
        CliProfileOutcome::Live(FetchOutcome::Failed { status: Some(429) }),
        local,
    );

    assert_eq!(usage.status, "throttled");
    assert_eq!(usage.five_hour, None);
    assert_eq!(usage.week, None);
}

#[test]
fn auth_source_claude_snapshot_missing_is_waiting() {
    use std::path::Path;
    assert!(matches!(
        read_claude_snapshot_outcome(Path::new("/nonexistent"), "id", true),
        CliProfileOutcome::WaitingForUsage
    ));
}

#[test]
fn auth_source_claude_usage_snapshot_round_trips_through_snapshot_reader() {
    let temp = TempDir::new().expect("create temp directory");
    let snapshot = temp.path().join("snapshot.json");
    let source_five_hour = QuotaUsage {
        percent: 30.0,
        resets_at: None,
        window_seconds: None,
    };
    let source_week = QuotaUsage {
        percent: 55.0,
        resets_at: None,
        window_seconds: None,
    };
    crate::claude_statusline::write_usage_snapshot_to_path(
        &snapshot,
        "id",
        &ClaudeQuota {
            five_hour: Some(source_five_hour.clone()),
            week: Some(source_week.clone()),
            breakdown: Vec::new(),
        },
    )
    .expect("write usage snapshot");

    let CliProfileOutcome::Live(FetchOutcome::Live {
        five_hour,
        week,
        plan,
        email,
        ..
    }) = read_claude_snapshot_outcome(&snapshot, "id", true)
    else {
        panic!("expected live Claude snapshot outcome");
    };

    assert_eq!(five_hour, Some(source_five_hour));
    assert_eq!(week, Some(source_week));
    assert_eq!(plan, None);
    assert_eq!(email.as_deref(), Some("id"));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn claude_cli_profile_falls_back_to_snapshot_without_profile_credentials() {
    let _lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _retain_unset_helper_without_invoking_it = ClaudeConfigDirGuard::unset;
    let temp = TempDir::new().expect("create temp directory");
    let config_root = temp.path().join("default-claude");
    std::fs::create_dir(&config_root).expect("create default Claude config directory");
    let _config = ClaudeConfigDirGuard::set(&config_root);
    std::fs::write(
        config_root.join(".claude.json"),
        serde_json::to_string(&json!({
            "oauthAccount": {
                "accountUuid": "other-account"
            }
        }))
        .unwrap(),
    )
    .expect("write mismatched default Claude identity");
    let store = crate::store::AccountStore::new_at(temp.path().join("store"));
    let account_id = uuid::Uuid::new_v4().to_string();
    let profile_root = temp.path().join("profile");
    std::fs::create_dir(&profile_root).expect("create empty profile directory");
    let snapshot = temp.path().join("snapshot.json");
    std::fs::write(
        &snapshot,
        r#"{"identity":"id","rate_limits":{"five_hour":{"utilization":30.0},"seven_day":{"utilization":55.0}}}"#,
    )
    .expect("write snapshot");

    let client = reqwest::Client::new();
    assert!(matches!(
        poll_claude_cli_profile(&store, &account_id, &client, &profile_root, "id", &snapshot,)
            .await,
        CliProfileOutcome::Live(FetchOutcome::Live { .. })
    ));
    assert!(matches!(
        poll_claude_cli_profile(
            &store,
            &account_id,
            &client,
            &profile_root,
            "id",
            &temp.path().join("missing.json"),
        )
        .await,
        CliProfileOutcome::WaitingForUsage
    ));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn claude_cli_profile_caches_matching_live_credentials_unchanged_before_snapshot_fallback() {
    let _lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = TempDir::new().expect("create temp directory");
    let config_root = temp.path().join("default-claude");
    std::fs::create_dir(&config_root).expect("create default Claude config directory");
    let _config = ClaudeConfigDirGuard::set(&config_root);
    let expected_identity = "live-account";
    let live_access_token = "live-access-token";
    let live_refresh_token = "live-refresh-token";
    let expires_at_ms = (Utc::now().timestamp() + 3_600) * 1_000;
    std::fs::write(
        config_root.join(".claude.json"),
        serde_json::to_string(&json!({
            "oauthAccount": {
                "emailAddress": "live@example.test",
                "accountUuid": expected_identity,
                "organizationUuid": "live-organization"
            }
        }))
        .unwrap(),
    )
    .expect("write default Claude identity");
    std::fs::write(
        config_root.join(".credentials.json"),
        serde_json::to_string(&json!({
            "claudeAiOauth": {
                "accessToken": live_access_token,
                "refreshToken": live_refresh_token,
                "expiresAt": expires_at_ms
            }
        }))
        .unwrap(),
    )
    .expect("write default Claude credentials");

    let store = crate::store::AccountStore::new_at(temp.path().join("store"));
    let account_id = uuid::Uuid::new_v4().to_string();
    let profile_root = temp.path().join("managed-profile");
    std::fs::create_dir(&profile_root).expect("create managed profile directory");
    let snapshot = temp.path().join("snapshot.json");
    std::fs::write(
        &snapshot,
        serde_json::to_string(&json!({
            "identity": expected_identity,
            "rate_limits": {
                "five_hour": { "utilization": 30.0 },
                "seven_day": { "utilization": 55.0 }
            }
        }))
        .unwrap(),
    )
    .expect("write snapshot");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(1))
        .build()
        .expect("build bounded test client");

    let CliProfileOutcome::Live(FetchOutcome::Live {
        five_hour, week, ..
    }) = poll_claude_cli_profile(
        &store,
        &account_id,
        &client,
        &profile_root,
        expected_identity,
        &snapshot,
    )
    .await
    else {
        panic!("expected snapshot fallback after the fake live-token fetch fails");
    };

    assert_eq!(five_hour.map(|quota| quota.percent), Some(30.0));
    assert_eq!(week.map(|quota| quota.percent), Some(55.0));
    let cached = store
        .cli_profile_credentials(&account_id)
        .expect("live credentials cached before fetch fallback");
    assert_eq!(cached.access_token, live_access_token);
    assert_eq!(cached.refresh_token, None);
    assert_eq!(cached.account_id.as_deref(), Some(expected_identity));
    assert_eq!(
        cached.expires_at.map(|expiry| expiry.timestamp_millis()),
        Some(expires_at_ms)
    );
}

#[test]
fn cli_profile_token_cache_round_trips() {
    let temp = TempDir::new().expect("create temp directory");
    let store = crate::store::AccountStore::new_at(temp.path().join("store"));
    let account_id = uuid::Uuid::new_v4().to_string();
    let credentials = Credentials {
        access_token: "access-token".to_string(),
        refresh_token: Some("refresh-token".to_string()),
        account_id: Some("claude-account".to_string()),
        expires_at: None,
    };

    assert!(store.cli_profile_credentials(&account_id).is_none());
    assert!(store.cli_profile_credentials("not-a-uuid").is_none());
    assert!(store
        .set_cli_profile_credentials("not-a-uuid", &credentials)
        .is_err());

    store
        .set_cli_profile_credentials(&account_id, &credentials)
        .expect("persist CLI-profile credentials");
    let loaded = store
        .cli_profile_credentials(&account_id)
        .expect("read CLI-profile credentials");

    assert!(loaded.access_token == credentials.access_token);
    assert!(loaded.refresh_token == credentials.refresh_token);
}

#[test]
fn agy_local_quota_matches_account_accepts_matching_email() {
    assert!(agy_local_quota_matches_account(
        Some("user@example.test"),
        "user@example.test",
        2,
    ));
}

#[test]
fn agy_local_quota_matches_account_is_case_insensitive() {
    assert!(agy_local_quota_matches_account(
        Some("User@Example.Test"),
        "user@example.test",
        2,
    ));
}

#[test]
fn agy_local_quota_matches_account_rejects_mismatched_email() {
    assert!(!agy_local_quota_matches_account(
        Some("other@example.test"),
        "user@example.test",
        2,
    ));
}

#[test]
fn agy_local_quota_matches_account_allows_unverified_label_when_sole_account() {
    assert!(agy_local_quota_matches_account(
        Some("user@example.test"),
        "Antigravity",
        1,
    ));
}

#[test]
fn agy_local_quota_matches_account_rejects_unverified_label_with_multiple_accounts() {
    assert!(!agy_local_quota_matches_account(
        Some("user@example.test"),
        "Antigravity",
        2,
    ));
}

#[test]
fn agy_local_quota_matches_account_allows_no_email_when_sole_account() {
    assert!(agy_local_quota_matches_account(None, "Antigravity", 1));
}

#[test]
fn agy_local_quota_matches_account_rejects_no_email_with_multiple_accounts() {
    assert!(!agy_local_quota_matches_account(None, "Antigravity", 2));
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
        read_claude_snapshot_outcome(&snapshot, "id", true),
        CliProfileOutcome::IdentityChanged
    ));
}

// ---------------------------------------------------------------------------
// GAP 1: app-owned CLI-profile token cache must not be trusted once the
// account is no longer unambiguously the sole Claude CliProfile account.
// ---------------------------------------------------------------------------

#[test]
fn claude_cli_profile_cache_is_trusted_for_sole_account_without_live_creds() {
    // Single account, no matching live login this poll: unchanged behavior —
    // the app-owned cache may still be read.
    assert!(claude_cli_profile_cache_is_trusted(false, true));
}

#[test]
fn claude_cli_profile_cache_is_trusted_rejects_multi_account() {
    // 2+ Claude CliProfile accounts: the cache may hold a live-ride token
    // cached while this account was still the sole account — no longer safe.
    assert!(!claude_cli_profile_cache_is_trusted(false, false));
}

#[test]
fn claude_cli_profile_cache_is_trusted_rejects_when_live_creds_present() {
    // Pre-existing behavior, both sole- and multi-account: a matching live
    // login this poll must not be reinterpreted as an app-owned refreshable
    // copy.
    assert!(!claude_cli_profile_cache_is_trusted(true, true));
    assert!(!claude_cli_profile_cache_is_trusted(true, false));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn claude_cli_profile_multi_account_ignores_cached_token_and_falls_through_to_snapshot() {
    let _lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = TempDir::new().expect("create temp directory");
    // A mismatched default identity makes Step A deterministically `Refuse` for
    // both accounts below, regardless of the sole-account ride flag — isolating
    // this test to Step B's cache-read gate.
    let config_root = temp.path().join("default-claude");
    std::fs::create_dir(&config_root).expect("create default Claude config directory");
    let _config = ClaudeConfigDirGuard::set(&config_root);
    std::fs::write(
        config_root.join(".claude.json"),
        serde_json::to_string(&json!({
            "oauthAccount": { "accountUuid": "other-account" }
        }))
        .unwrap(),
    )
    .expect("write mismatched default Claude identity");

    let store = crate::store::AccountStore::new_at(temp.path().join("store"));
    let profile_root_a = temp.path().join("profile-a");
    std::fs::create_dir(&profile_root_a).expect("create profile-a directory");
    let expected_identity = "account-a";
    let account_a = store
        .add_reference_with(
            usage_core::account::Provider::Claude,
            "account-a".to_string(),
            AuthSource::CliProfile {
                profile_root: profile_root_a.clone(),
                ownership: usage_core::account::ProfileOwnership::External,
                expected_identity: expected_identity.to_string(),
            },
            || true,
        )
        .expect("register first Claude CliProfile account");
    // A second Claude CliProfile account makes the store multi-account.
    store
        .add_reference_with(
            usage_core::account::Provider::Claude,
            "account-b".to_string(),
            AuthSource::CliProfile {
                profile_root: temp.path().join("profile-b"),
                ownership: usage_core::account::ProfileOwnership::External,
                expected_identity: "account-b".to_string(),
            },
            || true,
        )
        .expect("register second Claude CliProfile account");

    // Pre-populate the app-owned cache for account A, simulating a live-ride
    // token cached earlier while it may still have been the sole account.
    let cached_credentials = Credentials {
        access_token: "stale-cached-live-token".to_string(),
        refresh_token: None,
        account_id: Some(expected_identity.to_string()),
        expires_at: None,
    };
    store
        .set_cli_profile_credentials(&account_a.id, &cached_credentials)
        .expect("pre-populate app-owned cache for account A");

    // GAP 2 provenance is deliberately "live-ride" (untrusted-source) here, not
    // "bridge": with the GAP 1 fix, the stale cache is skipped and the
    // (also-empty) keychain seed fails too, so the flow reaches this snapshot
    // via `read_claude_snapshot_outcome` DIRECTLY, and GAP 2's untrusted-source
    // gate (multi-account ⇒ `trust_unverified_source = false`) then rejects a
    // "live-ride" snapshot outright — a bare `WaitingForUsage`. If GAP 1 were
    // NOT fixed, the stale cached token would instead be used for a real HTTP
    // fetch attempt, which would fail and route through
    // `claude_snapshot_after_fetch_failure` instead — wrapping the very same
    // rejected-snapshot read as `Live(Failed { .. })`. The two code paths are
    // therefore observably different outcomes, not just two routes to the same
    // assertion.
    let snapshot = temp.path().join("snapshot.json");
    std::fs::write(
        &snapshot,
        serde_json::to_string(&json!({
            "identity": expected_identity,
            "source": "live-ride",
            "rate_limits": {
                "five_hour": { "utilization": 30.0 },
                "seven_day": { "utilization": 55.0 }
            }
        }))
        .unwrap(),
    )
    .expect("write snapshot fixture");

    let client = reqwest::Client::new();
    let outcome = poll_claude_cli_profile(
        &store,
        &account_a.id,
        &client,
        &profile_root_a,
        expected_identity,
        &snapshot,
    )
    .await;

    assert!(
        matches!(outcome, CliProfileOutcome::WaitingForUsage),
        "expected a bare WaitingForUsage from the direct (non-cache) snapshot \
         read path, meaning the stale app-owned cache was never used for a \
         fetch attempt"
    );
}

// ---------------------------------------------------------------------------
// GAP 2: a Claude usage snapshot's `source` provenance gates acceptance for
// multi-account (untrusted) callers.
// ---------------------------------------------------------------------------

#[test]
fn read_claude_snapshot_outcome_rejects_live_ride_source_when_untrusted() {
    let temp = TempDir::new().expect("create temp directory");
    let snapshot = temp.path().join("snapshot.json");
    std::fs::write(
        &snapshot,
        serde_json::to_string(&json!({
            "identity": "id",
            "source": "live-ride",
            "rate_limits": { "five_hour": { "utilization": 30.0 } }
        }))
        .unwrap(),
    )
    .expect("write live-ride snapshot");

    assert!(matches!(
        read_claude_snapshot_outcome(&snapshot, "id", false),
        CliProfileOutcome::WaitingForUsage
    ));
}

#[test]
fn read_claude_snapshot_outcome_rejects_legacy_snapshot_without_source_when_untrusted() {
    let temp = TempDir::new().expect("create temp directory");
    let snapshot = temp.path().join("snapshot.json");
    std::fs::write(
        &snapshot,
        serde_json::to_string(&json!({
            "identity": "id",
            "rate_limits": { "five_hour": { "utilization": 30.0 } }
        }))
        .unwrap(),
    )
    .expect("write legacy snapshot without source");

    assert!(matches!(
        read_claude_snapshot_outcome(&snapshot, "id", false),
        CliProfileOutcome::WaitingForUsage
    ));
}

#[test]
fn read_claude_snapshot_outcome_accepts_bridge_source_when_untrusted() {
    let temp = TempDir::new().expect("create temp directory");
    let snapshot = temp.path().join("snapshot.json");
    std::fs::write(
        &snapshot,
        serde_json::to_string(&json!({
            "identity": "id",
            "source": "bridge",
            "rate_limits": { "five_hour": { "utilization": 30.0 } }
        }))
        .unwrap(),
    )
    .expect("write bridge snapshot");

    assert!(matches!(
        read_claude_snapshot_outcome(&snapshot, "id", false),
        CliProfileOutcome::Live(FetchOutcome::Live { .. })
    ));
}

#[test]
fn read_claude_snapshot_outcome_trusts_every_source_when_trust_unverified_source_true() {
    for source in [Some("live-ride"), Some("bridge"), None] {
        let temp = TempDir::new().expect("create temp directory");
        let snapshot = temp.path().join("snapshot.json");
        let mut body = json!({
            "identity": "id",
            "rate_limits": { "five_hour": { "utilization": 30.0 } }
        });
        if let Some(source) = source {
            body["source"] = json!(source);
        }
        std::fs::write(&snapshot, serde_json::to_string(&body).unwrap()).expect("write snapshot");

        assert!(
            matches!(
                read_claude_snapshot_outcome(&snapshot, "id", true),
                CliProfileOutcome::Live(FetchOutcome::Live { .. })
            ),
            "source {source:?} should be accepted when trust_unverified_source = true"
        );
    }
}

// ---------------------------------------------------------------------------
// The source gate must be evaluated before the identity check: an untrusted
// snapshot (source != "bridge") must fail closed to `WaitingForUsage` and
// must never be allowed to drive a user-visible `IdentityChanged` outcome,
// even when its stamped identity happens to disagree with the expected one.
// ---------------------------------------------------------------------------

#[test]
fn read_claude_snapshot_outcome_rejects_live_ride_source_with_mismatched_identity_when_untrusted() {
    let temp = TempDir::new().expect("create temp directory");
    let snapshot = temp.path().join("snapshot.json");
    std::fs::write(
        &snapshot,
        serde_json::to_string(&json!({
            "identity": "other",
            "source": "live-ride",
            "rate_limits": { "five_hour": { "utilization": 30.0 } }
        }))
        .unwrap(),
    )
    .expect("write live-ride snapshot with mismatched identity");

    assert!(matches!(
        read_claude_snapshot_outcome(&snapshot, "id", false),
        CliProfileOutcome::WaitingForUsage
    ));
}

#[test]
fn read_claude_snapshot_outcome_rejects_legacy_snapshot_without_source_and_mismatched_identity_when_untrusted(
) {
    let temp = TempDir::new().expect("create temp directory");
    let snapshot = temp.path().join("snapshot.json");
    std::fs::write(
        &snapshot,
        serde_json::to_string(&json!({
            "identity": "other",
            "rate_limits": { "five_hour": { "utilization": 30.0 } }
        }))
        .unwrap(),
    )
    .expect("write legacy snapshot without source and mismatched identity");

    assert!(matches!(
        read_claude_snapshot_outcome(&snapshot, "id", false),
        CliProfileOutcome::WaitingForUsage
    ));
}

#[test]
fn read_claude_snapshot_outcome_reports_identity_changed_for_bridge_source_when_untrusted() {
    let temp = TempDir::new().expect("create temp directory");
    let snapshot = temp.path().join("snapshot.json");
    std::fs::write(
        &snapshot,
        serde_json::to_string(&json!({
            "identity": "other",
            "source": "bridge",
            "rate_limits": { "five_hour": { "utilization": 30.0 } }
        }))
        .unwrap(),
    )
    .expect("write bridge snapshot with mismatched identity");

    // A trusted ("bridge") snapshot may still legitimately report an
    // identity change.
    assert!(matches!(
        read_claude_snapshot_outcome(&snapshot, "id", false),
        CliProfileOutcome::IdentityChanged
    ));
}

#[test]
fn read_claude_snapshot_outcome_reports_identity_changed_for_untrusted_source_when_trust_unverified_source_true(
) {
    for source in [Some("live-ride"), None] {
        let temp = TempDir::new().expect("create temp directory");
        let snapshot = temp.path().join("snapshot.json");
        let mut body = json!({
            "identity": "other",
            "rate_limits": { "five_hour": { "utilization": 30.0 } }
        });
        if let Some(source) = source {
            body["source"] = json!(source);
        }
        std::fs::write(&snapshot, serde_json::to_string(&body).unwrap()).expect("write snapshot");

        assert!(
            matches!(
                read_claude_snapshot_outcome(&snapshot, "id", true),
                CliProfileOutcome::IdentityChanged
            ),
            "source {source:?} should still report IdentityChanged when trust_unverified_source = true"
        );
    }
}

// ---------------------------------------------------------------------------
// GAP 3: a rejected agy local-quota probe must evict this account's
// remembered `last_success` value so a later OAuth failure never re-serves an
// now-ambiguous cached value as `stale`.
// ---------------------------------------------------------------------------

fn agy_account_for_eviction_test(store: &crate::store::AccountStore, label: &str) -> Account {
    store
        .add_with(
            Provider::Agy,
            label.to_string(),
            Credentials {
                // Empty access token so the OAuth fallback branch resolves to
                // `needs_login` without ever attempting a real network call.
                access_token: String::new(),
                refresh_token: None,
                account_id: None,
                expires_at: None,
            },
            || true,
        )
        .expect("add agy account")
}

fn seed_ok_last_success(account_id: &str) {
    let mut cache = crate::poller::last_success::last_success_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let ok_usage = AccountUsage {
        account: Account {
            id: account_id.to_string(),
            provider: Provider::Agy,
            label: "Antigravity".into(),
            auth_source: AuthSource::BrowserOAuth {
                credential_id: format!("credential-{account_id}"),
            },
        },
        display_name: "Antigravity".into(),
        plan: None,
        five_hour: None,
        week: None,
        totals: Default::default(),
        pool_breakdown: Vec::new(),
        breakdown: Vec::new(),
        detail_suffix: None,
        status: "ok".to_string(),
        local_status: None,
    };
    let stamped = crate::poller::last_success::apply_last_success(&mut cache, account_id, ok_usage);
    assert_eq!(stamped.status, "ok", "seed must actually cache as ok");
}

fn has_cached_last_success(account_id: &str) -> bool {
    crate::poller::last_success::last_success_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(account_id)
}

#[tokio::test]
async fn poll_agy_evicts_last_success_and_does_not_mutate_label_when_local_quota_is_rejected() {
    let temp = TempDir::new().expect("create temp directory");
    let store = crate::store::AccountStore::new_at(temp.path().join("store"));
    // Unverified label (no '@') + 2 agy accounts makes any local-quota match
    // ambiguous, so `agy_local_quota_matches_account` rejects it regardless of
    // the probed email.
    let account = agy_account_for_eviction_test(&store, "Antigravity");
    agy_account_for_eviction_test(&store, "Antigravity 2");

    seed_ok_last_success(&account.id);
    assert!(has_cached_last_success(&account.id));

    let client = reqwest::Client::new();
    let rejected_quota = AgyQuota {
        email: Some("other@example.test".to_string()),
        plan: None,
        pools: Vec::new(),
    };
    let usage = poll_agy_with_local_quota(&store, &client, &account, Some(rejected_quota)).await;

    assert!(
        !has_cached_last_success(&account.id),
        "rejected local quota must evict the remembered last_success value"
    );
    assert_eq!(usage.status, "needs_login");
    assert_eq!(
        store
            .account(&account.id)
            .expect("account still present")
            .label,
        "Antigravity",
        "rejection must not mutate the account label"
    );
}

#[tokio::test]
async fn poll_agy_does_not_evict_last_success_when_local_quota_matches() {
    let temp = TempDir::new().expect("create temp directory");
    let store = crate::store::AccountStore::new_at(temp.path().join("store"));
    let account = agy_account_for_eviction_test(&store, "user@example.test");

    seed_ok_last_success(&account.id);
    assert!(has_cached_last_success(&account.id));

    let client = reqwest::Client::new();
    let matching_quota = AgyQuota {
        email: Some("user@example.test".to_string()),
        plan: None,
        pools: Vec::new(),
    };
    let usage = poll_agy_with_local_quota(&store, &client, &account, Some(matching_quota)).await;

    assert_eq!(usage.status, "ok");
    assert!(
        has_cached_last_success(&account.id),
        "an accepted local quota must not evict last_success"
    );
}

#[tokio::test]
async fn poll_agy_does_not_evict_last_success_when_no_local_quota() {
    let temp = TempDir::new().expect("create temp directory");
    let store = crate::store::AccountStore::new_at(temp.path().join("store"));
    let account = agy_account_for_eviction_test(&store, "Antigravity");

    seed_ok_last_success(&account.id);
    assert!(has_cached_last_success(&account.id));

    let client = reqwest::Client::new();
    let usage = poll_agy_with_local_quota(&store, &client, &account, None).await;

    assert_eq!(usage.status, "needs_login");
    assert!(
        has_cached_last_success(&account.id),
        "no local-quota probe at all must not evict last_success"
    );
}
