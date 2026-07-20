use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::Command;

use chrono::{TimeZone, Utc};
use usage_core::account::{Credentials, Provider};

use crate::paths;

use super::{default_label, ImportedAccount};

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
pub(crate) fn claude_profile_is_default(profile_root: &std::path::Path) -> bool {
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

pub(crate) fn parse_expires_at(v: &serde_json::Value) -> Option<chrono::DateTime<Utc>> {
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
