//! Import credentials from CLI config files.
//!
//! Codex: `~/.codex/auth.json` (or `$CODEX_HOME/auth.json`)
//! Claude: macOS Keychain / Windows Credential Manager service
//!   `Claude Code-credentials` (preferred), then
//!   `~/.claude/.credentials.json` (or `$CLAUDE_CONFIG_DIR/...`)
//! Agy: not imported from CLI token DBs — use browser OAuth (`add-agy-oauth`).
//!
//! SECURITY: never log/print access_token or refresh_token values.

use std::path::Path;
#[cfg(any(target_os = "macos", feature = "edition-pro"))]
use std::process::Command;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{TimeZone, Utc};
use usage_core::account::{Credentials, Provider};

use crate::oauth::chatgpt_account_id_from_id_token;
use crate::paths;

/// Result of a CLI import: credentials plus a human-readable label
/// (email when available).
#[derive(Clone, Debug)]
pub struct ImportedAccount {
    pub credentials: Credentials,
    pub label: String,
}

/// Extracts `email` from a JWT payload (Codex id_token). Pure — never logs.
pub fn email_from_jwt(jwt: &str) -> Option<String> {
    let payload_b64 = jwt.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let root: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    root.get("email")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn default_label(provider: Provider) -> String {
    provider.display_name().to_string()
}

/// Reads `~/.claude.json` `oauthAccount` (email + accountUuid). Pure file I/O
/// helper used to label imports and give Claude an upsert identity.
fn claude_oauth_account() -> Option<(Option<String>, Option<String>)> {
    let home = paths::home_dir()?;
    claude_oauth_account_in(&home)
}

fn claude_oauth_account_in(root_dir: &Path) -> Option<(Option<String>, Option<String>)> {
    let path = root_dir.join(".claude.json");
    let data = std::fs::read_to_string(path).ok()?;
    let root: serde_json::Value = serde_json::from_str(&data).ok()?;
    let oauth = root.get("oauthAccount")?;
    let email = oauth
        .get("emailAddress")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let account_id = oauth
        .get("accountUuid")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if email.is_none() && account_id.is_none() {
        return None;
    }
    Some((email, account_id))
}

/// Reads (email, accountUuid, organizationUuid) from `<root_dir>/.claude.json`
/// `oauthAccount`. Pure file I/O; returns an empty identity set on any read or
/// parse failure.
pub(crate) fn claude_oauth_identity_set_in(
    root_dir: &Path,
) -> (Option<String>, Option<String>, Option<String>) {
    let path = root_dir.join(".claude.json");
    let Ok(data) = std::fs::read_to_string(path) else {
        return (None, None, None);
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&data) else {
        return (None, None, None);
    };
    let Some(oauth) = root.get("oauthAccount") else {
        return (None, None, None);
    };
    let email = oauth
        .get("emailAddress")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let account_id = oauth
        .get("accountUuid")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let org_uuid = oauth
        .get("organizationUuid")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    (email, account_id, org_uuid)
}

/// True iff `profile_root` is the default/unmanaged Claude profile (one of
/// `paths::claude_config_roots()`). Only the default profile may fall back to the OS-wide
/// `"Claude Code-credentials"` keychain service; managed per-account profiles must not, or they could
/// adopt a different identity's token. Compares canonicalized paths, falling back to raw equality.
fn claude_profile_is_default(profile_root: &std::path::Path) -> bool {
    let target = std::fs::canonicalize(profile_root).unwrap_or_else(|_| profile_root.to_path_buf());
    paths::claude_config_roots().into_iter().any(|root| {
        let root_canon = std::fs::canonicalize(&root).unwrap_or(root);
        root_canon == target
    })
}

/// Loads Claude credentials from Keychain / `.credentials.json`, attaching
/// `accountUuid` from `~/.claude.json` when present.
pub fn load_claude_cli_auth() -> Result<ImportedAccount, String> {
    // Claude Code stores OAuth in the OS keychain first; the on-disk
    // `.credentials.json` is only a fallback (often absent on macOS).
    let mut credentials = if let Some(creds) = read_claude_from_keychain() {
        creds
    } else {
        let files = paths::claude_credential_files();
        let mut found = None;
        for path in &files {
            let Ok(data) = std::fs::read_to_string(path) else {
                continue;
            };
            let Ok(root) = serde_json::from_str::<serde_json::Value>(&data) else {
                continue;
            };
            if let Some(creds) = parse_claude_credentials_json(&root) {
                found = Some(creds);
                break;
            }
        }
        found.ok_or_else(|| {
            "Claude credentials not found — run `claude` login first \
             (macOS: Keychain item \"Claude Code-credentials\", or ~/.claude/.credentials.json)"
                .to_string()
        })?
    };
    // Keychain blob has no account id; attach accountUuid from
    // ~/.claude.json so store upserts survive CLI account switches.
    let (email, account_id) = claude_oauth_account().unwrap_or((None, None));
    if credentials.account_id.is_none() {
        credentials.account_id = account_id;
    }
    Ok(ImportedAccount {
        label: email.unwrap_or_else(|| default_label(Provider::Claude)),
        credentials,
    })
}

/// Reads the credentials for a SPECIFIC Claude profile directory and returns them ONLY when the
/// profile's identity matches `expected_identity` (email, accountUuid, or organizationUuid).
/// Identity-safe: never adopts a different account's credentials. Returns None on identity mismatch
/// or when no creds are found. SECURITY: never log/print access_token or refresh_token.
pub fn load_claude_profile_credentials(
    profile_root: &Path,
    expected_identity: &str,
) -> Option<Credentials> {
    let (email, account_id, org_uuid) = claude_oauth_identity_set_in(profile_root);
    let expected = expected_identity.trim();
    if !expected.is_empty()
        && email.as_deref() != Some(expected)
        && account_id.as_deref() != Some(expected)
        && org_uuid.as_deref() != Some(expected)
    {
        return None;
    }

    let profile_service = paths::claude_keychain_service_name_for(profile_root);
    let mut credentials = read_claude_from_keychain_service(&profile_service)
        .or_else(|| {
            if claude_profile_is_default(profile_root) {
                read_claude_from_keychain_service("Claude Code-credentials")
            } else {
                None
            }
        })
        .or_else(|| read_claude_credentials_file(&profile_root.join(".credentials.json")))?;
    if credentials.account_id.is_none() {
        credentials.account_id = account_id;
    }
    Some(credentials)
}
/// Pure parser for Codex `auth.json` body. Returns `None` when no usable
/// token is present. Also returns an optional display email from `id_token`.
pub fn parse_codex_auth_json(root: &serde_json::Value) -> Option<(Credentials, Option<String>)> {
    if let Some(tokens) = root.get("tokens") {
        let access = tokens.get("access_token")?.as_str()?.to_string();
        if access.is_empty() {
            return None;
        }
        let account_id = tokens
            .get("account_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                tokens
                    .get("id_token")
                    .and_then(|v| v.as_str())
                    .and_then(chatgpt_account_id_from_id_token)
            });
        let refresh_token = tokens
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let expires_at = tokens
            .get("expires_at")
            .and_then(parse_expires_at)
            .or_else(|| tokens.get("expires").and_then(parse_expires_at));
        let email = tokens
            .get("id_token")
            .and_then(|v| v.as_str())
            .and_then(email_from_jwt);
        return Some((
            Credentials {
                access_token: access,
                refresh_token,
                account_id,
                expires_at,
            },
            email,
        ));
    }

    // Fallback: OPENAI_API_KEY style (API key as bearer — may not work for
    // wham/usage, but matches the Swift reader's behavior).
    let access = root.get("OPENAI_API_KEY")?.as_str()?.to_string();
    if access.is_empty() {
        return None;
    }
    Some((
        Credentials {
            access_token: access,
            refresh_token: None,
            account_id: None,
            expires_at: None,
        },
        None,
    ))
}

/// Pure parser for Claude credentials JSON (file or Keychain password blob).
/// Accepts either `{ "claudeAiOauth": { ... } }` or a flat `{ "accessToken": ... }`
/// object (Claude Code uses both shapes).
pub fn parse_claude_credentials_json(root: &serde_json::Value) -> Option<Credentials> {
    let oauth = root
        .get("claudeAiOauth")
        .filter(|v| v.is_object())
        .unwrap_or(root);

    let access = oauth.get("accessToken")?.as_str()?.to_string();
    if access.is_empty() {
        return None;
    }

    let refresh_token = oauth
        .get("refreshToken")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let expires_at = oauth.get("expiresAt").and_then(parse_expires_at);
    // Skip clearly-expired tokens unless a refresh_token can renew them.
    if let Some(exp) = expires_at {
        if exp < Utc::now() && refresh_token.is_none() {
            return None;
        }
    }

    Some(Credentials {
        access_token: access,
        refresh_token,
        // Filled by `import_from_cli` from `~/.claude.json` when available.
        account_id: None,
        expires_at,
    })
}

/// Candidate Keychain account names for Claude Code (username first, then
/// account-less lookup — same order as Claude Code CLI).
fn claude_keychain_accounts() -> Vec<Option<String>> {
    let mut accounts: Vec<Option<String>> = Vec::new();
    for key in ["USER", "USERNAME", "LOGNAME"] {
        if let Ok(v) = std::env::var(key) {
            let trimmed = v.trim();
            if !trimmed.is_empty() && !accounts.iter().any(|a| a.as_deref() == Some(trimmed)) {
                accounts.push(Some(trimmed.to_string()));
            }
        }
    }
    // Claude Code also tries `security ...` without `-a`.
    accounts.push(None);
    accounts
}

/// Parses a Keychain / file secret blob into credentials. Never logs `secret`.
fn credentials_from_secret_blob(secret: &str) -> Option<Credentials> {
    let root: serde_json::Value = serde_json::from_str(secret.trim()).ok()?;
    parse_claude_credentials_json(&root)
}

/// macOS: read via `/usr/bin/security` (same path Claude Code CLI uses).
/// The Security.framework/`keyring` crate path often fails for third-party
/// apps until the user grants Keychain access; `security` already works for
/// the logged-in user session.
#[cfg(target_os = "macos")]
fn read_claude_from_macos_security(service: &str, account: Option<&str>) -> Option<Credentials> {
    let mut cmd = Command::new("/usr/bin/security");
    cmd.arg("find-generic-password").arg("-s").arg(service);
    if let Some(account) = account {
        cmd.arg("-a").arg(account);
    }
    cmd.arg("-w");
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let secret = String::from_utf8(output.stdout).ok()?;
    credentials_from_secret_blob(&secret)
}

#[cfg(not(target_os = "macos"))]
fn read_claude_from_macos_security(_service: &str, _account: Option<&str>) -> Option<Credentials> {
    None
}

/// Windows / fallback: `keyring` crate (Credential Manager / Keychain API).
fn read_claude_from_keyring_crate(service: &str, account: &str) -> Option<Credentials> {
    let entry = keyring::Entry::new(service, account).ok()?;
    let secret = entry.get_password().ok()?;
    credentials_from_secret_blob(&secret)
}

fn read_claude_credentials_file(path: &Path) -> Option<Credentials> {
    let data = std::fs::read_to_string(path).ok()?;
    let root: serde_json::Value = serde_json::from_str(&data).ok()?;
    parse_claude_credentials_json(&root)
}

fn read_claude_from_keychain_service(service: &str) -> Option<Credentials> {
    for account in claude_keychain_accounts() {
        if let Some(creds) = read_claude_from_macos_security(service, account.as_deref()) {
            return Some(creds);
        }
        if let Some(account) = account.as_deref() {
            if let Some(creds) = read_claude_from_keyring_crate(service, account) {
                return Some(creds);
            }
        }
    }
    None
}

/// Reads Claude credentials from the OS credential store. Prefer macOS
/// `security` CLI (Claude Code's own path), then `keyring`, then files.
/// Never logs the secret payload.
fn read_claude_from_keychain() -> Option<Credentials> {
    let service = paths::claude_keychain_service_name();
    read_claude_from_keychain_service(&service)
}

fn parse_expires_at(v: &serde_json::Value) -> Option<chrono::DateTime<Utc>> {
    if let Some(n) = v.as_f64() {
        let secs = if n > 10_000_000_000.0 { n / 1000.0 } else { n };
        return Utc.timestamp_opt(secs as i64, 0).single();
    }
    if let Some(s) = v.as_str() {
        return chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.with_timezone(&Utc));
    }
    None
}
/// Loads Codex credentials from `~/.codex/auth.json` (or `$CODEX_HOME`).
pub fn load_codex_cli_auth() -> Result<ImportedAccount, String> {
    let path =
        paths::codex_auth_file().ok_or_else(|| "could not resolve home directory".to_string())?;
    let data = std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "Codex auth not found at {} — run `codex login` first",
            path.display()
        )
    })?;
    let root: serde_json::Value =
        serde_json::from_str(&data).map_err(|_| "Codex auth.json is not valid JSON".to_string())?;
    let (credentials, email) = parse_codex_auth_json(&root)
        .ok_or_else(|| "Codex auth.json has no usable access_token".to_string())?;
    Ok(ImportedAccount {
        label: email.unwrap_or_else(|| default_label(Provider::Codex)),
        credentials,
    })
}

/// Loads credentials for `provider` from the local CLI config.
/// Agy has no CLI auth import — use browser OAuth (`add-agy-oauth`).
pub fn import_from_cli(provider: Provider) -> Result<ImportedAccount, String> {
    match provider {
        Provider::Agy => Err(
            "Antigravity is not imported from local CLI token DBs — use Login Antigravity (browser)"
                .into(),
        ),
        Provider::Codex => load_codex_cli_auth(),
        Provider::Claude => load_claude_cli_auth(),
        #[cfg(feature = "edition-pro")]
        Provider::Cursor => crate::cursor_local::load_cursor_local_auth(),
        #[cfg(feature = "edition-pro")]
        Provider::Grok => load_grok_env_auth(),
        #[cfg(feature = "edition-pro")]
        Provider::Higgsfield => load_higgsfield_cli_auth(),
    }
}

#[cfg(feature = "edition-pro")]
/// xAI Management API credentials from `XAI_MGMT_KEY` + `XAI_TEAM_ID`.
pub fn load_grok_env_auth() -> Result<ImportedAccount, String> {
    let key = std::env::var("XAI_MGMT_KEY")
        .or_else(|_| std::env::var("XAI_MANAGEMENT_KEY"))
        .map_err(|_| {
            "set XAI_MGMT_KEY (or XAI_MANAGEMENT_KEY) with your xAI Management Key".to_string()
        })?;
    if key.trim().is_empty() {
        return Err("XAI_MGMT_KEY is empty".into());
    }
    let team_id = std::env::var("XAI_TEAM_ID")
        .map_err(|_| "set XAI_TEAM_ID with your xAI team ID".to_string())?;
    if team_id.trim().is_empty() {
        return Err("XAI_TEAM_ID is empty".into());
    }
    Ok(grok_imported_account(&key, &team_id))
}

#[cfg(feature = "edition-pro")]
fn grok_imported_account(key: &str, team_id: &str) -> ImportedAccount {
    let team_id = team_id.trim();
    ImportedAccount {
        label: format!("Grok · team {team_id}"),
        credentials: Credentials {
            access_token: key.trim().to_string(),
            refresh_token: None,
            account_id: Some(team_id.to_string()),
            expires_at: None,
        },
    }
}

#[cfg(feature = "edition-pro")]
fn read_clipboard_text() -> Result<String, String> {
    arboard::Clipboard::new()
        .map_err(|e| format!("clipboard unavailable: {e}"))?
        .get_text()
        .map_err(|_| {
            "clipboard has no text — copy your xAI Management Key, then try again".to_string()
        })
}

#[cfg(feature = "edition-pro")]
/// Validates a Management Key via the official xAI endpoint and resolves team ID.
pub async fn validate_grok_management_key(key: &str) -> Result<String, String> {
    use usage_core::fetch::grok::team_id_from_validation;

    let client = reqwest::Client::new();
    let resp = client
        .get("https://management-api.x.ai/auth/management-keys/validation")
        .header("Accept", "application/json")
        .header("User-Agent", "UsageCheck")
        .bearer_auth(key.trim())
        .send()
        .await
        .map_err(|e| format!("management key validation request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "management key validation failed (HTTP {})",
            resp.status()
        ));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|_| "management key validation response is not valid JSON".to_string())?;
    team_id_from_validation(&body)
        .ok_or_else(|| "validation succeeded but response has no team/scope id".to_string())
}

#[cfg(feature = "edition-pro")]
/// Imports Grok from the system clipboard: validates the Management Key, or
/// falls back to a pasted team ID / `XAI_TEAM_ID` when validation cannot
/// resolve scope.
pub async fn import_grok_from_clipboard() -> Result<ImportedAccount, String> {
    use usage_core::fetch::grok::parse_grok_paste;

    let text = read_clipboard_text()?;
    let (key, pasted_team) = parse_grok_paste(&text);
    if key.is_empty() {
        return Err(
            "clipboard is empty — copy your xAI Management Key, then choose Import Grok (clipboard)"
                .into(),
        );
    }

    match validate_grok_management_key(&key).await {
        Ok(team_id) => Ok(grok_imported_account(&key, &team_id)),
        Err(validation_err) => {
            let team_id = pasted_team
                .or_else(|| {
                    std::env::var("XAI_TEAM_ID")
                        .ok()
                        .filter(|s| !s.trim().is_empty())
                })
                .ok_or_else(|| {
                    format!(
                        "{validation_err} — paste key and team ID on separate lines, \
                         set XAI_TEAM_ID, or use Import Grok (env vars)"
                    )
                })?;
            Ok(grok_imported_account(&key, &team_id))
        }
    }
}

#[cfg(feature = "edition-pro")]
/// Higgsfield CLI account reference from `higgsfield account status --json`.
pub fn load_higgsfield_cli_auth() -> Result<ImportedAccount, String> {
    use usage_core::fetch::higgsfield::parse_higgsfield_account;

    let output = Command::new("higgsfield")
        .args(["account", "status", "--json"])
        .output()
        .map_err(|_| {
            "Higgsfield CLI unavailable — run `higgsfield auth login` first".to_string()
        })?;
    if !output.status.success() {
        return Err(
            "Higgsfield CLI status command failed — run `higgsfield auth login` first".into(),
        );
    }
    let root: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "Higgsfield CLI status output is not valid JSON".to_string())?;
    let account = parse_higgsfield_account(&root);
    Ok(ImportedAccount {
        label: account
            .email
            .unwrap_or_else(|| default_label(Provider::Higgsfield)),
        credentials: Credentials {
            access_token: String::new(),
            refresh_token: None,
            account_id: None,
            expires_at: None,
        },
    })
}

#[cfg(test)]
mod tests {
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
}
