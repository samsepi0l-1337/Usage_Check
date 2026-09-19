use super::claude::{resolve_claude_identity_decision, ClaudeLoginDecision};
use super::*;
use serde_json::json;
use std::ffi::OsString;
use std::path::Path;
use tempfile::TempDir;

struct ClaudeConfigDirGuard(Option<OsString>);

impl ClaudeConfigDirGuard {
    fn set(path: &Path) -> Self {
        let previous = std::env::var_os("CLAUDE_CONFIG_DIR");
        std::env::set_var("CLAUDE_CONFIG_DIR", path);
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

fn write_claude_default_login(root: &Path) {
    std::fs::write(
        root.join(".claude.json"),
        serde_json::to_string(&json!({
            "oauthAccount": {
                "emailAddress": "live@example.test",
                "accountUuid": "live-account",
                "organizationUuid": "live-organization"
            }
        }))
        .unwrap(),
    )
    .expect("write default Claude identity");
    std::fs::write(
        root.join(".credentials.json"),
        serde_json::to_string(&json!({
            "claudeAiOauth": {
                "accessToken": "live-access-token",
                "refreshToken": "live-refresh-token"
            }
        }))
        .unwrap(),
    )
    .expect("write default Claude credentials");
}

#[test]
fn parses_codex_tokens_block() {
    let root = json!({
        "tokens": {
            "access_token": "at-1",
            "refresh_token": "rt-1",
            "account_id": "acct-9"
        }
    });
    let (c, email) = parse_codex_auth_json(&root).unwrap();
    assert_eq!(c.access_token, "at-1");
    assert_eq!(c.refresh_token.as_deref(), Some("rt-1"));
    assert_eq!(c.account_id.as_deref(), Some("acct-9"));
    assert!(email.is_none());
}

#[test]
fn parses_codex_openai_api_key_fallback() {
    let root = json!({ "OPENAI_API_KEY": "sk-test" });
    let (c, _) = parse_codex_auth_json(&root).unwrap();
    assert_eq!(c.access_token, "sk-test");
}

#[test]
fn rejects_empty_codex_token() {
    let root = json!({ "tokens": { "access_token": "" } });
    assert!(parse_codex_auth_json(&root).is_none());
}

#[test]
fn parses_claude_oauth_block() {
    let future_ms = (Utc::now().timestamp() + 3600) * 1000;
    let root = json!({
        "claudeAiOauth": {
            "accessToken": "claude-at",
            "refreshToken": "claude-rt",
            "expiresAt": future_ms
        }
    });
    let c = parse_claude_credentials_json(&root).unwrap();
    assert_eq!(c.access_token, "claude-at");
    assert_eq!(c.refresh_token.as_deref(), Some("claude-rt"));
    assert!(c.expires_at.is_some());
}

#[test]
fn rejects_expired_claude_token_without_refresh() {
    let past_ms = (Utc::now().timestamp() - 3600) * 1000;
    let root = json!({
        "claudeAiOauth": {
            "accessToken": "claude-at",
            "expiresAt": past_ms
        }
    });
    assert!(parse_claude_credentials_json(&root).is_none());
}

#[test]
fn accepts_expired_claude_token_when_refresh_present() {
    let past_ms = (Utc::now().timestamp() - 3600) * 1000;
    let root = json!({
        "claudeAiOauth": {
            "accessToken": "claude-at",
            "refreshToken": "claude-rt",
            "expiresAt": past_ms
        }
    });
    let c = parse_claude_credentials_json(&root).unwrap();
    assert_eq!(c.refresh_token.as_deref(), Some("claude-rt"));
}

#[test]
fn parses_flat_claude_oauth_object() {
    let root = json!({
        "accessToken": "flat-at",
        "refreshToken": "flat-rt"
    });
    let c = parse_claude_credentials_json(&root).unwrap();
    assert_eq!(c.access_token, "flat-at");
}

#[test]
fn claude_profile_is_not_default_for_managed_style_directory() {
    let profile = TempDir::new().expect("create profile directory");

    assert!(!claude_profile_is_default(profile.path()));
}

#[test]
fn claude_profile_credentials_do_not_fall_back_to_default_keychain() {
    let profile = TempDir::new().expect("create profile directory");
    std::fs::write(
        profile.path().join(".claude.json"),
        serde_json::to_string(&json!({
            "oauthAccount": {
                "emailAddress": "managed@example.test",
                "accountUuid": "managed-account"
            }
        }))
        .unwrap(),
    )
    .expect("write profile identity");

    assert!(load_claude_profile_credentials(profile.path(), "managed@example.test").is_none());
}

#[test]
fn claude_profile_credentials_reject_identity_mismatch() {
    let profile = TempDir::new().expect("create profile directory");
    std::fs::write(
        profile.path().join(".claude.json"),
        serde_json::to_string(&json!({
            "oauthAccount": {
                "emailAddress": "other@example.test",
                "accountUuid": "other-account",
                "organizationUuid": "other-organization"
            }
        }))
        .unwrap(),
    )
    .expect("write profile identity");
    std::fs::write(
        profile.path().join(".credentials.json"),
        serde_json::to_string(&json!({
            "claudeAiOauth": { "accessToken": "test-access-token" }
        }))
        .unwrap(),
    )
    .expect("write profile credentials");

    assert!(load_claude_profile_credentials(profile.path(), "expected@example.test").is_none());
}

#[test]
fn claude_profile_credentials_accept_matching_identity_from_file() {
    let profile = TempDir::new().expect("create profile directory");
    std::fs::write(
        profile.path().join(".claude.json"),
        serde_json::to_string(&json!({
            "oauthAccount": {
                "emailAddress": "match@example.test",
                "accountUuid": "profile-account"
            }
        }))
        .unwrap(),
    )
    .expect("write profile identity");
    std::fs::write(
        profile.path().join(".credentials.json"),
        serde_json::to_string(&json!({
            "claudeAiOauth": { "accessToken": "test-access-token" }
        }))
        .unwrap(),
    )
    .expect("write profile credentials");

    let credentials = load_claude_profile_credentials(profile.path(), "match@example.test")
        .expect("matching profile credentials");
    assert!(!credentials.access_token.is_empty());
    assert_eq!(credentials.account_id.as_deref(), Some("profile-account"));
}

#[test]
fn claude_profile_credentials_accept_matching_organization_uuid_from_file() {
    let profile = TempDir::new().expect("create profile directory");
    std::fs::write(
        profile.path().join(".claude.json"),
        serde_json::to_string(&json!({
            "oauthAccount": {
                "emailAddress": "org-match@example.test",
                "accountUuid": "profile-account",
                "organizationUuid": "profile-organization"
            }
        }))
        .unwrap(),
    )
    .expect("write profile identity");
    std::fs::write(
        profile.path().join(".credentials.json"),
        serde_json::to_string(&json!({
            "claudeAiOauth": { "accessToken": "test-access-token" }
        }))
        .unwrap(),
    )
    .expect("write profile credentials");

    let credentials = load_claude_profile_credentials(profile.path(), "profile-organization")
        .expect("organization-matching profile credentials");
    assert!(!credentials.access_token.is_empty());
    assert_eq!(credentials.account_id.as_deref(), Some("profile-account"));
}

#[test]
fn claude_default_login_credentials_accept_each_matching_identity_from_file() {
    let _lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .expect("environment lock");
    let config_root = TempDir::new().expect("create default Claude config directory");
    let _config = ClaudeConfigDirGuard::set(config_root.path());
    write_claude_default_login(config_root.path());

    for expected_identity in ["live@example.test", "live-account", "live-organization"] {
        let credentials = load_claude_default_login_credentials(expected_identity, true)
            .expect("identity-matching default Claude credentials");
        assert_eq!(credentials.access_token, "live-access-token");
        assert_eq!(
            credentials.refresh_token.as_deref(),
            Some("live-refresh-token")
        );
        assert_eq!(credentials.account_id.as_deref(), Some("live-account"));
    }
}

#[test]
fn claude_default_login_credentials_reject_identity_mismatch() {
    let _lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .expect("environment lock");
    let config_root = TempDir::new().expect("create default Claude config directory");
    let _config = ClaudeConfigDirGuard::set(config_root.path());
    write_claude_default_login(config_root.path());

    assert!(load_claude_default_login_credentials("different-account", true).is_none());
}

#[test]
fn claude_default_login_credentials_reject_empty_expected_identity() {
    let _lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .expect("environment lock");
    let config_root = TempDir::new().expect("create default Claude config directory");
    let _config = ClaudeConfigDirGuard::set(config_root.path());
    write_claude_default_login(config_root.path());

    assert!(load_claude_default_login_credentials("  ", true).is_none());
}

#[test]
fn claude_default_login_credentials_ride_requires_allow_unverified_flag() {
    let _lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .expect("environment lock");
    let config_root = TempDir::new().expect("create default Claude config directory");
    let _config = ClaudeConfigDirGuard::set(config_root.path());
    // No `oauthAccount` present — identity is unverifiable, not mismatched — so
    // `resolve_claude_identity_decision` returns `RideUnverifiedLive` (covered by
    // `claude_default_login_credentials_rides_live_login_when_no_identity_present`).
    std::fs::write(
        config_root.path().join(".claude.json"),
        serde_json::to_string(&json!({ "machineID": "test-machine" })).unwrap(),
    )
    .expect("write default Claude config without identity");

    // Ambiguous with 2+ Claude CliProfile accounts: the caller must pass
    // `allow_unverified_ride = false`, and the loader must fail closed — deterministically
    // None, WITHOUT even attempting the live keychain read — rather than let this account
    // ride the same live login as every other account.
    assert!(load_claude_default_login_credentials("expected-account", false).is_none());

    // Sole account (today's working behavior): `allow_unverified_ride = true` still reaches
    // the real ride attempt (no test keychain entry exists in this sandbox, so it also
    // resolves to None here — the assertion above is what actually guards the fail-closed
    // gate; this call just proves `true` does not short-circuit the same way).
    assert!(load_claude_default_login_credentials("expected-account", true).is_none());
}

#[test]
fn claude_default_login_credentials_rides_live_login_when_no_identity_present() {
    let _lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .expect("environment lock");
    let config_root = TempDir::new().expect("create default Claude config directory");
    let _config = ClaudeConfigDirGuard::set(config_root.path());
    std::fs::write(
        config_root.path().join(".claude.json"),
        serde_json::to_string(&json!({ "machineID": "test-machine" })).unwrap(),
    )
    .expect("write default Claude config without identity");

    assert_eq!(
        resolve_claude_identity_decision("expected-account", &crate::paths::claude_config_roots(),),
        ClaudeLoginDecision::RideUnverifiedLive
    );
}

#[test]
fn claude_default_login_credentials_refuses_when_identity_present_but_mismatched() {
    let _lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .expect("environment lock");
    let config_root = TempDir::new().expect("create default Claude config directory");
    let _config = ClaudeConfigDirGuard::set(config_root.path());
    std::fs::write(
        config_root.path().join(".claude.json"),
        serde_json::to_string(&json!({
            "oauthAccount": { "accountUuid": "different-account" }
        }))
        .unwrap(),
    )
    .expect("write mismatching default Claude identity");

    assert_eq!(
        resolve_claude_identity_decision("expected-account", &crate::paths::claude_config_roots(),),
        ClaudeLoginDecision::Refuse
    );
}

#[test]
fn agy_import_is_rejected() {
    let err = import_from_cli(Provider::Agy).unwrap_err();
    assert!(err.contains("Antigravity"), "{err}");
}

/// Live Keychain smoke (macOS). Ignored by default so CI without Claude
/// login still passes. Run with: `cargo test --bins -- --ignored`
#[test]
#[ignore]
fn imports_claude_from_local_keychain_when_present() {
    let imported = import_from_cli(Provider::Claude).expect("claude import");
    assert!(!imported.credentials.access_token.is_empty());
    // Never assert on token contents — only shape.
    assert!(
        imported.credentials.access_token.starts_with("sk-ant-")
            || imported.credentials.access_token.len() > 20
    );
}

#[test]
fn xai_env_parse_reads_mgmt_key_and_team() {
    use std::env;

    // Set env vars
    env::set_var("XAI_MGMT_KEY", "test-mgmt-key-123");
    env::set_var("XAI_TEAM_ID", "test-team-456");

    // Load should succeed
    let result = load_grok_env_auth();
    assert!(
        result.is_ok(),
        "load_grok_env_auth should succeed with env vars set"
    );

    let imported = result.unwrap();
    assert_eq!(imported.credentials.access_token, "test-mgmt-key-123");
    assert_eq!(
        imported.credentials.account_id,
        Some("test-team-456".to_string())
    );

    // Clean up
    env::remove_var("XAI_MGMT_KEY");
    env::remove_var("XAI_TEAM_ID");

    // Test with empty/missing env
    let result_empty = load_grok_env_auth();
    assert!(
        result_empty.is_err(),
        "load_grok_env_auth should fail without env vars"
    );
}

#[test]
fn xai_paste_dedupes_team_line() {
    use usage_core::fetch::grok::parse_grok_paste;

    // Test with key + team on separate lines
    let (key, team) = parse_grok_paste("KEY123\nTEAM456");
    assert_eq!(key, "KEY123");
    assert_eq!(team, Some("TEAM456".to_string()));

    // Test with single line (key only)
    let (key_only, team_none) = parse_grok_paste("KEY789");
    assert_eq!(key_only, "KEY789");
    assert!(team_none.is_none());
}

#[test]
fn grok_imported_account_accepts_valid_team_id() {
    let imported = grok_imported_account("  test-mgmt-key  ", "team-abc").unwrap();
    assert_eq!(imported.label, "Grok · team team-abc");
    assert_eq!(imported.credentials.account_id.as_deref(), Some("team-abc"));
    assert_eq!(imported.credentials.access_token, "test-mgmt-key");
}

#[test]
fn grok_imported_account_rejects_invalid_team_id() {
    let err = grok_imported_account("test-mgmt-key", "Translated Report (Full Report Below)")
        .unwrap_err();
    assert!(err.contains("team id"), "{err}");
    assert!(grok_imported_account("test-mgmt-key", "").is_err());
    assert!(grok_imported_account("test-mgmt-key", "  ").is_err());
}

#[test]
fn xai_stored_as_management_reference() {
    use crate::store::AccountStore;
    use tempfile::TempDir;
    use usage_core::account::{AuthSource, Credentials, Provider};

    let root = TempDir::new().unwrap();
    let store = AccountStore::new_at(root.path().to_path_buf());

    let raw_key = "xai-management-key-test-value";
    let account = store
        .add_with(
            Provider::Grok,
            "xAI API credits".into(),
            Credentials {
                access_token: raw_key.into(),
                refresh_token: None,
                account_id: Some("test-team".into()),
                expires_at: None,
            },
            || true,
        )
        .expect("store xAI account");

    // Verify the account was stored with XaiManagement auth source
    assert!(matches!(
        account.auth_source,
        AuthSource::XaiManagement { ref team_id, .. } if team_id == "test-team"
    ));

    // Verify the raw key is NOT in the serialized account index
    let index_path = root.path().join("accounts-v2.json");
    if index_path.exists() {
        let index = std::fs::read_to_string(&index_path).expect("read account index");
        assert!(
            !index.contains(raw_key),
            "raw key must not leak into account index"
        );
    }
}

#[test]
fn kimi_import_reads_first_usable_credentials_file() {
    let dir = TempDir::new().unwrap();
    let creds_dir = dir.path().join("credentials");
    std::fs::create_dir_all(&creds_dir).unwrap();
    std::fs::write(
        creds_dir.join("kimi-code.json"),
        r#"{"access_token":"kimi-at","refresh_token":"kimi-rt","expires_at":1769861835.261056,"scope":"kimi-code","token_type":"Bearer"}"#,
    )
    .unwrap();
    let imported = super::kimi::load_kimi_cli_auth_from_files(&[creds_dir.join("kimi-code.json")])
        .expect("kimi import");
    assert_eq!(imported.credentials.access_token, "kimi-at");
    assert_eq!(
        imported.credentials.refresh_token.as_deref(),
        Some("kimi-rt")
    );
    assert!(imported.credentials.expires_at.is_some());
}

#[test]
fn kimi_import_skips_expired_without_refresh() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("expired.json");
    std::fs::write(&path, r#"{"access_token":"old","expires_at":1000}"#).unwrap();
    let err = super::kimi::load_kimi_cli_auth_from_files(&[path]).unwrap_err();
    assert!(err.contains("Kimi Code credentials not found"), "{err}");
}

#[test]
fn opencode_import_reads_only_go_key() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("auth.json");
    std::fs::write(
        &path,
        r#"{
            "opencode": { "type": "api", "key": "zen-should-be-ignored" },
            "opencode-go": { "type": "api", "key": "oc_go_key" },
            "openrouter": { "type": "api", "key": "sk-or-should-not-go-here" }
        }"#,
    )
    .unwrap();
    let imported = super::opencode::load_opencode_cli_auth_from(&path).expect("opencode-go import");
    assert_eq!(imported.credentials.access_token, "oc_go_key");
    assert_eq!(imported.label, "OpenCode Go");
}

#[test]
fn opencode_import_missing_go_entry_mentions_login() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("auth.json");
    std::fs::write(&path, r#"{"opencode":{"type":"api","key":"zen"}}"#).unwrap();
    let err = super::opencode::load_opencode_cli_auth_from(&path).unwrap_err();
    assert!(err.contains("opencode auth login"), "{err}");
    assert!(err.contains("OpenCode Go"), "{err}");
}

#[test]
fn deepseek_yaml_refs_and_flat_and_env() {
    assert_eq!(
        parse_deepseek_api_key("version: 1\nrefs:\n  DEEPSEEK_API_KEY: sk-from-refs\n").as_deref(),
        Some("sk-from-refs")
    );
    assert_eq!(
        parse_deepseek_api_key("DEEPSEEK_API_KEY: sk-flat\n").as_deref(),
        Some("sk-flat")
    );
    assert_eq!(
        parse_deepseek_api_key("export DEEPSEEK_API_KEY=\"sk-env\"\n").as_deref(),
        Some("sk-env")
    );

    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join(".credentials.yaml"),
        "DEEPSEEK_API_KEY: sk-yaml\n",
    )
    .unwrap();
    let imported = super::deepseek::load_deepseek_cli_auth_from(dir.path()).unwrap();
    assert_eq!(imported.credentials.access_token, "sk-yaml");

    let env_only = TempDir::new().unwrap();
    std::fs::write(env_only.path().join(".env"), "DEEPSEEK_API_KEY=sk-dotenv\n").unwrap();
    let imported = super::deepseek::load_deepseek_cli_auth_from(env_only.path()).unwrap();
    assert_eq!(imported.credentials.access_token, "sk-dotenv");
}

#[test]
fn deepseek_missing_files_mentions_dsh() {
    let dir = TempDir::new().unwrap();
    let err = super::deepseek::load_deepseek_cli_auth_from(dir.path()).unwrap_err();
    assert!(err.contains("npx @deepseek-ai/dsh"), "{err}");
    assert!(err.contains("~/.dsh/.env"), "{err}");
}

#[test]
fn openrouter_prefers_ori_config_over_opencode_auth() {
    let dir = TempDir::new().unwrap();
    let ori = dir.path().join("config.json");
    let oc = dir.path().join("auth.json");
    std::fs::write(&ori, r#"{"env":{"OPENROUTER_API_KEY":"sk-or-from-ori"}}"#).unwrap();
    std::fs::write(
        &oc,
        r#"{"openrouter":{"type":"api","key":"sk-or-from-opencode"},"opencode-go":{"type":"api","key":"oc_ignored"}}"#,
    )
    .unwrap();
    let imported = super::openrouter::load_openrouter_cli_auth_from(Some(&ori), Some(&oc)).unwrap();
    assert_eq!(imported.credentials.access_token, "sk-or-from-ori");
}

#[test]
fn openrouter_falls_back_to_opencode_auth_entry() {
    let dir = TempDir::new().unwrap();
    let oc = dir.path().join("auth.json");
    std::fs::write(
        &oc,
        r#"{"openrouter":{"type":"api","key":"sk-or-from-opencode"},"opencode-go":{"type":"api","key":"oc_ignored"}}"#,
    )
    .unwrap();
    let imported = super::openrouter::load_openrouter_cli_auth_from(None, Some(&oc)).unwrap();
    assert_eq!(imported.credentials.access_token, "sk-or-from-opencode");
}

#[test]
fn new_pro_providers_store_as_browser_oauth_secrets() {
    use crate::store::AccountStore;
    use usage_core::account::AuthSource;

    let root = TempDir::new().unwrap();
    let store = AccountStore::new_at(root.path().to_path_buf());
    for provider in [
        Provider::Kimi,
        Provider::OpenCode,
        Provider::DeepSeek,
        Provider::OpenRouter,
        Provider::Copilot,
    ] {
        let account = store
            .add_with(
                provider,
                provider.display_name().into(),
                Credentials {
                    access_token: format!("{provider:?}-token"),
                    refresh_token: None,
                    account_id: None,
                    expires_at: None,
                },
                || true,
            )
            .expect("store secret");
        assert!(
            matches!(account.auth_source, AuthSource::BrowserOAuth { .. }),
            "{provider:?} should persist as BrowserOAuth"
        );
    }
}

#[test]
fn copilot_import_walks_apps_json_for_oauth_token() {
    let dir = TempDir::new().unwrap();
    let apps = dir.path().join("apps.json");
    std::fs::write(
        &apps,
        r#"{"github.com:device":{"user":"octocat","oauth_token":"gho_from_apps"}}"#,
    )
    .unwrap();
    let imported = super::copilot::load_copilot_cli_auth_from_files(&[apps], &[]).unwrap();
    assert_eq!(imported.credentials.access_token, "gho_from_apps");
    assert_eq!(imported.label, "octocat");
}

#[test]
fn copilot_import_falls_back_to_gh_hosts_yml() {
    let dir = TempDir::new().unwrap();
    let yml = dir.path().join("hosts.yml");
    std::fs::write(
        &yml,
        "github.com:\n    user: octocat\n    oauth_token: gho_from_gh\n",
    )
    .unwrap();
    let imported = super::copilot::load_copilot_cli_auth_from_files(&[], &[yml]).unwrap();
    assert_eq!(imported.credentials.access_token, "gho_from_gh");
}

#[test]
fn copilot_import_missing_files_does_not_need_env() {
    let err = super::copilot::load_copilot_cli_auth_from_files(&[], &[]).unwrap_err();
    assert!(err.contains("GitHub Copilot"), "{err}");
}

#[test]
fn windsurf_import_reads_sqlite_api_key() {
    use rusqlite::{params, Connection};

    let dir = TempDir::new().unwrap();
    let db = dir.path().join("state.vscdb");
    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "CREATE TABLE ItemTable (id INTEGER PRIMARY KEY, key TEXT, value TEXT)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
        params![
            "windsurfAuthStatus",
            r#"{"email":"ws@example.com","apiKey":"ws-secret"}"#
        ],
    )
    .unwrap();
    drop(conn);

    let session = crate::windsurf_local::read_windsurf_session(&db).unwrap();
    assert_eq!(session.api_key, "ws-secret");
    assert_eq!(session.identity, "ws@example.com");
}

#[test]
fn minimax_import_parses_fake_cli_quota_json() {
    let dir = TempDir::new().unwrap();
    let bin = super::minimax::write_fake_minimax_cli(
        dir.path(),
        r#"{
            "email": "mmx@example.com",
            "plan_name": "Token Plan",
            "model_remains": [{
                "model_name": "general",
                "current_interval_remaining_percent": 60,
                "current_weekly_remaining_percent": 90
            }]
        }"#,
    );
    let imported = super::minimax::load_minimax_cli_auth_from(&bin).unwrap();
    assert_eq!(imported.label, "mmx@example.com");
    assert!(imported.credentials.access_token.is_empty());
}

#[test]
fn minimax_import_falls_back_to_display_name_without_email() {
    let dir = TempDir::new().unwrap();
    let bin = super::minimax::write_fake_minimax_cli(
        dir.path(),
        r#"{
            "model_remains": [{
                "model_name": "general",
                "current_interval_remaining_percent": 100
            }]
        }"#,
    );
    let imported = super::minimax::load_minimax_cli_auth_from(&bin).unwrap();
    assert_eq!(imported.label, "MiniMax");
}

#[test]
fn minimax_import_missing_binary_tells_user_to_install_mmx() {
    let err = super::minimax::load_minimax_cli_auth_with(None).unwrap_err();
    assert!(err.contains("install mmx"), "{err}");
    assert!(err.contains("mmx auth login"), "{err}");
}

#[test]
fn augment_import_parses_fake_cli_status_json() {
    let dir = TempDir::new().unwrap();
    let bin = super::augment::write_fake_augment_cli(
        dir.path(),
        r#"{
            "email": "aug@example.com",
            "plan": "Developer",
            "credits_remaining": 25000,
            "included_credits": 100000
        }"#,
    );
    let imported = super::augment::load_augment_cli_auth_from(&bin).unwrap();
    assert_eq!(imported.label, "aug@example.com");
    assert!(imported.credentials.access_token.is_empty());
}

#[test]
fn augment_import_falls_back_to_display_name_without_email() {
    let dir = TempDir::new().unwrap();
    let bin = super::augment::write_fake_augment_cli(dir.path(), r#"{ "credits_remaining": 12 }"#);
    let imported = super::augment::load_augment_cli_auth_from(&bin).unwrap();
    assert_eq!(imported.label, "Augment");
}

#[test]
fn augment_import_missing_binary_tells_user_to_install_auggie() {
    let err = super::augment::load_augment_cli_auth_with(None).unwrap_err();
    assert!(err.contains("install auggie"), "{err}");
    assert!(err.contains("auggie login"), "{err}");
}
