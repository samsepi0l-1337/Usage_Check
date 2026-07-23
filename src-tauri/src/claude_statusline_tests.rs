use super::*;
use chrono::{TimeZone, Utc};
use std::ffi::OsString;
use std::sync::Mutex;
use tempfile::TempDir;
use usage_core::account::{Account, ProfileOwnership, Provider};
use usage_core::fetch::claude::ClaudeQuota;
use usage_core::models::QuotaUsage;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct HomeGuard(Option<OsString>);

impl HomeGuard {
    fn set(path: &Path) -> Self {
        let previous = std::env::var_os("HOME");
        std::env::set_var("HOME", path);
        Self(previous)
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(previous) => std::env::set_var("HOME", previous),
            None => std::env::remove_var("HOME"),
        }
    }
}

fn read_json(path: &Path) -> Value {
    let body = fs::read_to_string(path).expect("test JSON should be readable");
    serde_json::from_str(&body).expect("test JSON should parse")
}

fn quota_usage(percent: f64, timestamp: i64) -> QuotaUsage {
    QuotaUsage {
        percent,
        resets_at: Some(
            Utc.timestamp_opt(timestamp, 0)
                .single()
                .expect("valid timestamp"),
        ),
        window_seconds: None,
    }
}

#[test]
fn write_usage_snapshot_emits_exact_round_trip_shape() {
    let temp = TempDir::new().expect("temp directory");
    let snapshot_path = temp.path().join("statusline_snapshot.json");
    let five_hour = quota_usage(33.25, 1_784_774_400);
    let week = quota_usage(22.5, 1_785_292_800);
    let quota = ClaudeQuota {
        five_hour: Some(five_hour.clone()),
        week: Some(week.clone()),
    };

    write_usage_snapshot_to_path(&snapshot_path, "user@example.com", &quota)
        .expect("write usage snapshot");

    let written = read_json(&snapshot_path);
    assert_eq!(
        written,
        json!({
            "identity": "user@example.com",
            "rate_limits": {
                "five_hour": {
                    "utilization": 33.25,
                    "resets_at": five_hour.resets_at.expect("five-hour reset").to_rfc3339()
                },
                "seven_day": {
                    "utilization": 22.5,
                    "resets_at": week.resets_at.expect("weekly reset").to_rfc3339()
                }
            }
        })
    );
    let serialized = serde_json::to_string(&written).expect("serialize written snapshot");
    assert!(!serialized.contains("access_token"));
    assert!(!serialized.contains("refresh_token"));

    let parsed = usage_core::fetch::claude::parse_claude_usage(&written["rate_limits"]);
    assert_eq!(parsed.five_hour, quota.five_hour);
    assert_eq!(parsed.week, quota.week);
    assert_eq!(written["identity"], "user@example.com");
}

#[test]
fn write_usage_snapshot_skips_empty_identity_and_empty_quota() {
    let temp = TempDir::new().expect("temp directory");
    let empty_identity_path = temp.path().join("empty-identity.json");
    let populated_quota = ClaudeQuota {
        five_hour: Some(quota_usage(33.0, 1_784_774_400)),
        week: None,
    };

    write_usage_snapshot_to_path(&empty_identity_path, "", &populated_quota)
        .expect("skip empty identity");
    assert!(!empty_identity_path.exists());

    let empty_quota_path = temp.path().join("empty-quota.json");
    let empty_quota = ClaudeQuota {
        five_hour: None,
        week: None,
    };
    write_usage_snapshot_to_path(&empty_quota_path, "user@example.com", &empty_quota)
        .expect("skip empty quota");
    assert!(!empty_quota_path.exists());
}

#[test]
fn bridge_settings_path_uses_cli_profile_root() {
    let profile_root = std::path::PathBuf::from("claude-profile");
    let account = Account {
        id: "claude-account".to_string(),
        provider: Provider::Claude,
        label: "user@example.com".to_string(),
        auth_source: AuthSource::CliProfile {
            profile_root: profile_root.clone(),
            ownership: ProfileOwnership::External,
            expected_identity: "user@example.com".to_string(),
        },
    };

    assert_eq!(
        resolve_bridge_settings_path(Some(&account), &account.id),
        profile_root.join("settings.json")
    );
}

#[test]
fn bridge_settings_path_falls_back_without_cli_profile() {
    let account_id = "claude-account";
    let expected = crate::paths::claude_settings_json(account_id);
    assert_eq!(resolve_bridge_settings_path(None, account_id), expected);

    let account = Account {
        id: account_id.to_string(),
        provider: Provider::Claude,
        label: "user@example.com".to_string(),
        auth_source: AuthSource::BrowserOAuth {
            credential_id: "credential-id".to_string(),
        },
    };
    assert_eq!(
        resolve_bridge_settings_path(Some(&account), account_id),
        expected
    );
}

#[test]
fn install_and_remove_use_object_form_and_preserve_complete_prior() {
    let temp = TempDir::new().expect("temp directory");
    let settings_path = temp.path().join("settings.json");
    let prior = json!({"type": "command", "command": "old-command", "padding": 3});
    fs::write(&settings_path, json!({"statusLine": prior}).to_string()).expect("write settings");

    install_statusline_bridge(&settings_path, "test-account").expect("install bridge");

    let installed = read_json(&settings_path);
    assert_eq!(installed["statusLine"]["type"], "command");
    let command = installed["statusLine"]["command"]
        .as_str()
        .expect("command string");
    assert!(
        command.contains("--claude-statusline-bridge test-account"),
        "{command}"
    );
    assert!(
        command.contains("usage-app") || command.contains("usage_app"),
        "{command}"
    );
    assert_eq!(
        read_json(&temp.path().join(".statusline_prior.json"))["prior_command"],
        prior
    );

    remove_statusline_bridge(&settings_path, "test-account").expect("remove bridge");
    assert_eq!(read_json(&settings_path)["statusLine"], prior);
}

#[test]
fn remove_preserves_later_user_edit() {
    let temp = TempDir::new().expect("temp directory");
    let settings_path = temp.path().join("settings.json");
    fs::write(
        &settings_path,
        json!({"statusLine": "old-command"}).to_string(),
    )
    .expect("write settings");
    install_statusline_bridge(&settings_path, "test-account").expect("install bridge");
    let mut settings = read_json(&settings_path);
    settings["statusLine"] = json!({"type": "command", "command": "user-edit"});
    fs::write(&settings_path, settings.to_string()).expect("update settings");

    remove_statusline_bridge(&settings_path, "test-account").expect("remove is no-op");

    assert_eq!(
        read_json(&settings_path)["statusLine"]["command"],
        "user-edit"
    );
}

#[test]
fn account_id_rejection_blocks_shell_metacharacters() {
    let temp = TempDir::new().expect("temp directory");
    let settings_path = temp.path().join("settings.json");
    fs::write(&settings_path, "{}").expect("write settings");
    for invalid in ["bad;id", "bad id", "$(bad)"] {
        assert!(
            validate_account_id(invalid).is_err(),
            "accepted {invalid:?}"
        );
        assert!(
            install_statusline_bridge(&settings_path, invalid).is_err(),
            "installed {invalid:?}"
        );
    }
    assert_eq!(read_json(&settings_path), json!({}));
}

#[test]
fn runtime_bridge_preserves_bytes_and_writes_minimal_snapshot() {
    let _lock = ENV_LOCK.lock().expect("environment lock");
    let temp = TempDir::new().expect("temp directory");
    let _home = HomeGuard::set(temp.path());
    let account_id = "runtime-test";
    let settings_path = crate::paths::claude_settings_json(account_id);
    fs::create_dir_all(settings_path.parent().expect("settings parent")).expect("create profile");
    #[cfg(unix)]
    let prior_command = "cat";
    #[cfg(windows)]
    let prior_command = "more";
    fs::write(
        &settings_path,
        json!({"statusLine": {"type": "command", "command": prior_command}}).to_string(),
    )
    .expect("write settings");
    install_statusline_bridge(&settings_path, account_id).expect("install bridge");
    let input = br#"{"model":{"id":"claude-opus-4-8"},"rate_limits":{"five_hour":{"used_percentage":23.5,"resets_at":1738425600},"seven_day":{"used_percentage":41.2,"resets_at":1738857600}}}"#;
    let mut output = Vec::new();
    run_bridge(
        account_id,
        "user@example.com",
        &settings_path,
        &input[..],
        &mut output,
    )
    .expect("run bridge");
    assert_eq!(output, input); // prior "cat" echoes stdin through
    let snap = read_json(&crate::paths::claude_statusline_snapshot(account_id));
    assert_eq!(snap["identity"], "user@example.com");
    assert_eq!(snap["rate_limits"]["five_hour"]["utilization"], 23.5);
    assert_eq!(snap["rate_limits"]["five_hour"]["resets_at"], 1738425600);
    let quota = usage_core::fetch::claude::parse_claude_usage(&snap["rate_limits"]);
    assert!(quota.five_hour.is_some() && quota.week.is_some());
    assert_eq!(quota.five_hour.unwrap().percent, 23.5);
}

#[test]
fn runtime_bridge_emits_fallback_without_prior_command() {
    let _lock = ENV_LOCK.lock().expect("environment lock");
    let temp = TempDir::new().expect("temp directory");
    let _home = HomeGuard::set(temp.path());
    let account_id = "fallback-test";
    let settings_path = crate::paths::claude_settings_json(account_id);
    fs::create_dir_all(settings_path.parent().expect("settings parent")).expect("create profile");
    fs::write(&settings_path, "{}").expect("write settings");
    let input = br#"{"model":{"id":"claude-opus-4-8"}}"#;
    let mut output = Vec::new();

    run_bridge(
        account_id,
        "ready@example.com",
        &settings_path,
        &input[..],
        &mut output,
    )
    .expect("run bridge");

    let rendered = String::from_utf8(output).expect("UTF-8 fallback");
    assert!(rendered.contains("ready@example.com"));
    assert!(rendered.contains("Usage check ready"));
}

#[test]
fn runtime_bridge_writes_empty_snapshot_without_rate_limits() {
    let _lock = ENV_LOCK.lock().expect("environment lock");
    let temp = TempDir::new().expect("temp directory");
    let _home = HomeGuard::set(temp.path());
    let account_id = "norates";
    let settings_path = crate::paths::claude_settings_json(account_id);
    fs::create_dir_all(settings_path.parent().expect("parent")).expect("create profile");
    fs::write(&settings_path, "{}").expect("write settings");
    let input = br#"{"model":{"id":"claude-opus-4-8"},"cost":{"total_cost_usd":0.01}}"#;
    let mut output = Vec::new();
    run_bridge(
        account_id,
        "u@e.com",
        &settings_path,
        &input[..],
        &mut output,
    )
    .expect("run bridge");
    let snap = read_json(&crate::paths::claude_statusline_snapshot(account_id));
    let quota = usage_core::fetch::claude::parse_claude_usage(&snap["rate_limits"]);
    assert!(quota.five_hour.is_none() && quota.week.is_none());
}

#[test]
fn install_rejects_non_object_root() {
    let temp = TempDir::new().expect("temp directory");
    let settings_path = temp.path().join("settings.json");
    fs::write(&settings_path, "42").expect("write settings");

    assert!(install_statusline_bridge(&settings_path, "test-account").is_err());
}

#[test]
fn remove_bridge_tears_down_sidecar_and_snapshot_on_non_bridge_statusline() {
    let _lock = ENV_LOCK.lock().expect("environment lock");
    let temp = TempDir::new().expect("temp directory");
    let _home = HomeGuard::set(temp.path());
    let account_id = "teardown-test";
    let settings_path = temp.path().join("settings.json");
    let sidecar_path = temp.path().join(".statusline_prior.json");
    fs::write(
        &settings_path,
        json!({"statusLine": {"type": "command", "command": "user-edit"}}).to_string(),
    )
    .expect("write settings");
    fs::write(
        &sidecar_path,
        json!({"prior_command": {"type": "command", "command": "noop"}}).to_string(),
    )
    .expect("write sidecar");
    let snapshot_path = crate::paths::claude_statusline_snapshot(account_id);
    fs::create_dir_all(snapshot_path.parent().expect("snapshot parent"))
        .expect("create snapshot dir");
    fs::write(
        &snapshot_path,
        json!({"identity":"teardown-test"}).to_string(),
    )
    .expect("write snapshot");

    remove_statusline_bridge(&settings_path, account_id).expect("remove no-op");

    assert!(!sidecar_path.exists());
    assert!(!snapshot_path.exists());
    assert_eq!(
        read_json(&settings_path)["statusLine"]["command"],
        "user-edit"
    );
}
