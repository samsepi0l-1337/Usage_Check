use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::cli_auth::ProviderAdapter;
use crate::terminal::TerminalCommand;
use serde_json::Value;
use usage_core::account::{Account, AuthSource, ProfileOwnership, Provider};

/// Resolve the Claude executable on PATH
fn which_claude() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(if cfg!(windows) {
                "claude.exe"
            } else {
                "claude"
            });
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Parse Claude auth status --json output to extract identity and plan
fn parse_claude_status(text: &str) -> Result<(String, String), String> {
    let obj: Value = serde_json::from_str(text).map_err(|e| format!("JSON parse error: {}", e))?;

    // Must have loggedIn: true
    let logged_in = obj
        .get("loggedIn")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| "missing or non-bool loggedIn".to_string())?;

    if !logged_in {
        return Err("not logged in".to_string());
    }

    // Identity: prefer orgId, fallback to normalized email
    let identity = obj
        .get("orgId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            obj.get("email")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_lowercase())
        })
        .ok_or_else(|| "missing identity (orgId or email)".to_string())?;

    // Plan: subscriptionType
    let plan = obj
        .get("subscriptionType")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    Ok((identity, plan))
}

/// Claude CLI adapter
#[derive(Debug, Clone)]
pub struct ClaudeCliAdapter;

impl ProviderAdapter for ClaudeCliAdapter {
    fn probe(&self) -> Result<Option<Account>, String> {
        // Try default CLAUDE_CONFIG_DIR first
        if let Some(default_dir) = crate::paths::default_claude_config_dir_checked() {
            if let Ok((identity, _plan)) = probe_claude_dir(&default_dir) {
                return Ok(Some(Account {
                    id: format!("claude-{}", identity),
                    provider: Provider::Claude,
                    label: identity.clone(),
                    auth_source: AuthSource::CliProfile {
                        profile_root: default_dir,
                        ownership: ProfileOwnership::External,
                        expected_identity: identity,
                    },
                }));
            }
        }

        // Try managed root
        if let Ok(managed_dir) = crate::paths::claude_managed_root("default") {
            if let Ok((identity, _plan)) = probe_claude_dir(&managed_dir) {
                return Ok(Some(Account {
                    id: format!("claude-{}", identity),
                    provider: Provider::Claude,
                    label: identity.clone(),
                    auth_source: AuthSource::CliProfile {
                        profile_root: managed_dir,
                        ownership: ProfileOwnership::Managed,
                        expected_identity: identity,
                    },
                }));
            }
        }

        Ok(None)
    }

    fn login_command(&self, profile_root: &Path) -> TerminalCommand {
        let profile_str = profile_root.to_string_lossy().to_string();

        TerminalCommand {
            executable: PathBuf::from("claude"),
            args: vec![
                OsString::from("auth"),
                OsString::from("login"),
                OsString::from("--claudeai"),
            ],
            env: vec![(
                OsString::from("CLAUDE_CONFIG_DIR"),
                OsString::from(&profile_str),
            )],
            env_remove: vec![
                OsString::from("ANTHROPIC_API_KEY"),
                OsString::from("CLAUDE_CODE_OAUTH_TOKEN"),
            ],
        }
    }

    fn resolve_account(&self, _auth_source: AuthSource) -> Result<Account, String> {
        Err("Claude resolve_account not yet implemented".to_string())
    }
}

/// Probe a Claude directory (sync wrapper around async probe)
fn probe_claude_dir(profile_dir: &Path) -> Result<(String, String), String> {
    let profile_str = profile_dir.to_string_lossy().to_string();

    let claude_exe =
        which_claude().ok_or_else(|| "claude executable not found on PATH".to_string())?;

    // Use block_in_place to call async from sync context
    let result = tokio::task::block_in_place(|| {
        let handle = tokio::runtime::Handle::current();
        handle.block_on(async {
            let mut child = tokio::process::Command::new(&claude_exe)
                .args(["auth", "status", "--json"])
                .env("CLAUDE_CONFIG_DIR", &profile_str)
                .env_remove("ANTHROPIC_API_KEY")
                .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("failed to spawn claude: {}", e))?;

            let stdout = child.stdout.take().ok_or_else(|| "no stdout".to_string())?;
            let mut reader = tokio::io::BufReader::new(stdout);
            let mut output = String::new();

            use tokio::io::AsyncReadExt;
            reader
                .read_to_string(&mut output)
                .await
                .map_err(|e| format!("read error: {}", e))?;

            let status = child
                .wait()
                .await
                .map_err(|e| format!("wait error: {}", e))?;

            if !status.success() {
                return Err(format!(
                    "claude auth status exited with: {}",
                    status.code().unwrap_or(-1)
                ));
            }

            Ok::<String, String>(output)
        })
    })?;

    parse_claude_status(&result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_claude_status_with_org_id() {
        let json = r#"{"loggedIn":true,"orgId":"org-123","email":"user@example.com","subscriptionType":"pro"}"#;
        let (identity, plan) = parse_claude_status(json).unwrap();
        assert_eq!(identity, "org-123");
        assert_eq!(plan, "pro");
    }

    #[test]
    fn test_parse_claude_status_fallback_email() {
        let json = r#"{"loggedIn":true,"email":"  User@Example.COM  ","subscriptionType":"free"}"#;
        let (identity, plan) = parse_claude_status(json).unwrap();
        assert_eq!(identity, "user@example.com");
        assert_eq!(plan, "free");
    }

    #[test]
    fn test_parse_claude_status_not_logged_in() {
        let json = r#"{"loggedIn":false}"#;
        assert!(parse_claude_status(json).is_err());
    }

    #[test]
    fn test_parse_claude_status_missing_identity() {
        let json = r#"{"loggedIn":true,"subscriptionType":"pro"}"#;
        assert!(parse_claude_status(json).is_err());
    }

    #[test]
    fn test_which_claude() {
        let _ = which_claude();
    }
}
