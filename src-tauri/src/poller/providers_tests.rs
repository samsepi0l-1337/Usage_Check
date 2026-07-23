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
        read_claude_snapshot_outcome(Path::new("/nonexistent"), "id"),
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
    }) = read_claude_snapshot_outcome(&snapshot, "id")
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
        .expect("environment lock");
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
        .expect("environment lock");
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
