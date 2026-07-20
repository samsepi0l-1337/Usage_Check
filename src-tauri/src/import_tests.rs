use super::*;
use serde_json::json;
use tempfile::TempDir;

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
#[cfg(feature = "edition-pro")]
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
#[cfg(feature = "edition-pro")]
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
#[cfg(feature = "edition-pro")]
fn grok_imported_account_accepts_valid_team_id() {
    let imported = grok_imported_account("  test-mgmt-key  ", "team-abc").unwrap();
    assert_eq!(imported.label, "Grok · team team-abc");
    assert_eq!(imported.credentials.account_id.as_deref(), Some("team-abc"));
    assert_eq!(imported.credentials.access_token, "test-mgmt-key");
}

#[test]
#[cfg(feature = "edition-pro")]
fn grok_imported_account_rejects_invalid_team_id() {
    let err = grok_imported_account(
        "test-mgmt-key",
        "Translated Report (Full Report Below)",
    )
    .unwrap_err();
    assert!(err.contains("team id"), "{err}");
    assert!(grok_imported_account("test-mgmt-key", "").is_err());
    assert!(grok_imported_account("test-mgmt-key", "  ").is_err());
}

#[test]
#[cfg(feature = "edition-pro")]
fn xai_stored_as_management_reference() {
    use crate::store::AccountStore;
    use tempfile::TempDir;
    use usage_core::account::{AuthSource, Credentials, Provider};

    let root = TempDir::new().unwrap();
    let store = AccountStore::new_at(root.path().to_path_buf());

    let raw_key = "xai-management-key-test-value";
    let account = store
        .add(
            Provider::Grok,
            "xAI API credits".into(),
            Credentials {
                access_token: raw_key.into(),
                refresh_token: None,
                account_id: Some("test-team".into()),
                expires_at: None,
            },
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
