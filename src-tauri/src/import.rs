//! Import credentials (or local-only accounts) from CLI config files.
//!
//! Codex: `~/.codex/auth.json` (or `$CODEX_HOME/auth.json`)
//! Claude: `~/.claude/.credentials.json` (or `$CLAUDE_CONFIG_DIR/...`)
//! Agy: no auth file — creates a local-log-only account with empty credentials.
//!
//! SECURITY: never log/print access_token or refresh_token values.

use chrono::{TimeZone, Utc};
use usage_core::account::{Credentials, Provider};

use crate::paths;

/// Pure parser for Codex `auth.json` body. Returns `None` when no usable
/// token is present.
pub fn parse_codex_auth_json(root: &serde_json::Value) -> Option<Credentials> {
    if let Some(tokens) = root.get("tokens") {
        let access = tokens.get("access_token")?.as_str()?.to_string();
        if access.is_empty() {
            return None;
        }
        let account_id = tokens
            .get("account_id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let refresh_token = tokens
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let expires_at = tokens
            .get("expires_at")
            .and_then(parse_expires_at)
            .or_else(|| tokens.get("expires").and_then(parse_expires_at));
        return Some(Credentials {
            access_token: access,
            refresh_token,
            account_id,
            expires_at,
        });
    }

    // Fallback: OPENAI_API_KEY style (API key as bearer — may not work for
    // wham/usage, but matches the Swift reader's behavior).
    let access = root.get("OPENAI_API_KEY")?.as_str()?.to_string();
    if access.is_empty() {
        return None;
    }
    Some(Credentials {
        access_token: access,
        refresh_token: None,
        account_id: None,
        expires_at: None,
    })
}

/// Pure parser for Claude `.credentials.json` body. Looks under
/// `claudeAiOauth` for `accessToken` / `refreshToken` / `expiresAt`.
pub fn parse_claude_credentials_json(root: &serde_json::Value) -> Option<Credentials> {
    let oauth = root.get("claudeAiOauth")?;
    let access = oauth.get("accessToken")?.as_str()?.to_string();
    if access.is_empty() {
        return None;
    }

    let expires_at = oauth.get("expiresAt").and_then(parse_expires_at);
    // Skip clearly-expired tokens (same as Swift).
    if let Some(exp) = expires_at {
        if exp < Utc::now() {
            return None;
        }
    }

    let refresh_token = oauth
        .get("refreshToken")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    Some(Credentials {
        access_token: access,
        refresh_token,
        account_id: None,
        expires_at,
    })
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

/// Empty placeholder credentials for local-log-only accounts (agy).
pub fn local_only_credentials() -> Credentials {
    Credentials {
        access_token: String::new(),
        refresh_token: None,
        account_id: None,
        expires_at: None,
    }
}

/// Loads credentials for `provider` from the local CLI config, or returns
/// empty credentials for agy. Errors when the expected auth file is missing
/// or unreadable (Codex/Claude).
pub fn import_from_cli(provider: Provider) -> Result<Credentials, String> {
    match provider {
        Provider::Agy => Ok(local_only_credentials()),
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
            parse_codex_auth_json(&root)
                .ok_or_else(|| "Codex auth.json has no usable access_token".to_string())
        }
        Provider::Claude => {
            let files = paths::claude_credential_files();
            if files.is_empty() {
                return Err("could not resolve home directory".to_string());
            }
            for path in &files {
                let Ok(data) = std::fs::read_to_string(path) else {
                    continue;
                };
                let Ok(root) = serde_json::from_str::<serde_json::Value>(&data) else {
                    continue;
                };
                if let Some(creds) = parse_claude_credentials_json(&root) {
                    return Ok(creds);
                }
            }
            Err(
                "Claude credentials not found — run `claude` login first, or set CLAUDE_CONFIG_DIR"
                    .to_string(),
            )
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
        let c = parse_codex_auth_json(&root).unwrap();
        assert_eq!(c.access_token, "at-1");
        assert_eq!(c.refresh_token.as_deref(), Some("rt-1"));
        assert_eq!(c.account_id.as_deref(), Some("acct-9"));
    }

    #[test]
    fn parses_codex_openai_api_key_fallback() {
        let root = json!({ "OPENAI_API_KEY": "sk-test" });
        let c = parse_codex_auth_json(&root).unwrap();
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
    fn rejects_expired_claude_token() {
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
    fn agy_import_returns_empty_creds() {
        let c = import_from_cli(Provider::Agy).unwrap();
        assert!(c.access_token.is_empty());
    }
}
