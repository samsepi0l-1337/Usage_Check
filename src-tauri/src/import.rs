//! Import credentials from CLI config files.
//!
//! Codex: `~/.codex/auth.json` (or `$CODEX_HOME/auth.json`)
//! Claude: macOS Keychain / Windows Credential Manager service
//!   `Claude Code-credentials` (preferred), then
//!   `~/.claude/.credentials.json` (or `$CLAUDE_CONFIG_DIR/...`)
//! Agy: not imported from CLI token DBs — use browser OAuth (`add-agy-oauth`).
//!
//! SECURITY: never log/print access_token or refresh_token values.

#[cfg(target_os = "macos")]
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
    match provider {
        Provider::Codex => "Codex".into(),
        Provider::Claude => "Claude".into(),
        Provider::Agy => "agy".into(),
    }
}

fn claude_email_from_config() -> Option<String> {
    let home = paths::home_dir()?;
    let path = home.join(".claude.json");
    let data = std::fs::read_to_string(path).ok()?;
    let root: serde_json::Value = serde_json::from_str(&data).ok()?;
    root.get("oauthAccount")?
        .get("emailAddress")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
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
            if !trimmed.is_empty()
                && !accounts
                    .iter()
                    .any(|a| a.as_deref() == Some(trimmed))
            {
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

/// Reads Claude credentials from the OS credential store. Prefer macOS
/// `security` CLI (Claude Code's own path), then `keyring`, then files.
/// Never logs the secret payload.
fn read_claude_from_keychain() -> Option<Credentials> {
    let service = paths::claude_keychain_service_name();
    for account in claude_keychain_accounts() {
        if let Some(creds) =
            read_claude_from_macos_security(&service, account.as_deref())
        {
            return Some(creds);
        }
        if let Some(account) = account.as_deref() {
            if let Some(creds) = read_claude_from_keyring_crate(&service, account) {
                return Some(creds);
            }
        }
    }
    None
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

/// Loads credentials for `provider` from the local CLI config.
/// Agy has no CLI auth import — use browser OAuth (`add-agy-oauth`).
pub fn import_from_cli(provider: Provider) -> Result<ImportedAccount, String> {
    match provider {
        Provider::Agy => Err(
            "Antigravity is not imported from local CLI token DBs — use Login Antigravity (browser)"
                .into(),
        ),
        Provider::Codex => {
            let path = paths::codex_auth_file()
                .ok_or_else(|| "could not resolve home directory".to_string())?;
            let data = std::fs::read_to_string(&path).map_err(|_| {
                format!(
                    "Codex auth not found at {} — run `codex login` first",
                    path.display()
                )
            })?;
            let root: serde_json::Value = serde_json::from_str(&data)
                .map_err(|_| "Codex auth.json is not valid JSON".to_string())?;
            let (credentials, email) = parse_codex_auth_json(&root)
                .ok_or_else(|| "Codex auth.json has no usable access_token".to_string())?;
            Ok(ImportedAccount {
                label: email.unwrap_or_else(|| default_label(Provider::Codex)),
                credentials,
            })
        }
        Provider::Claude => {
            // Claude Code stores OAuth in the OS keychain first; the on-disk
            // `.credentials.json` is only a fallback (often absent on macOS).
            let credentials = if let Some(creds) = read_claude_from_keychain() {
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
            Ok(ImportedAccount {
                label: claude_email_from_config()
                    .unwrap_or_else(|| default_label(Provider::Claude)),
                credentials,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
