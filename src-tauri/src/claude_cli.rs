use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::cli_auth::ProviderAdapter;
use crate::terminal::TerminalCommand;
use serde_json::Value;
use usage_core::account::{Account, AuthSource, ProfileOwnership, Provider};

/// Directories to search for a provider CLI. A GUI process launched by launchd /
/// Finder inherits a minimal PATH (`/usr/bin:/bin:/usr/sbin:/sbin`) that omits the
/// Homebrew / user-local dirs where `claude` is typically installed, so we augment
/// the process PATH with the common locations.
fn candidate_bin_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(path) = std::env::var("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    #[cfg(not(windows))]
    for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        dirs.push(PathBuf::from(extra));
    }
    if let Some(home) = crate::paths::home_dir() {
        for sub in [
            ".local/bin",
            ".claude/local",
            ".bun/bin",
            ".deno/bin",
            ".volta/bin",
            ".npm-global/bin",
            "bin",
        ] {
            dirs.push(home.join(sub));
        }
    }
    dirs
}

/// Resolve the Claude executable to an absolute path, searching a PATH superset so it
/// is found even from a GUI process with a minimal PATH.
fn which_claude() -> Option<PathBuf> {
    let bin = if cfg!(windows) { "claude.exe" } else { "claude" };
    for dir in candidate_bin_dirs() {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            return Some(candidate);
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

/// Reads Claude identity directly from `<profile_dir>/.claude.json` (no `claude` binary, no runtime).
/// Preserves the historical precedence: organizationUuid (the CLI's `orgId`) preferred, else the
/// lowercased/trimmed email. Returns `(identity, plan)` with an unknown plan because subscription
/// type is not present in `oauthAccount`.
fn read_claude_identity_from_json(profile_dir: &Path) -> Option<(String, String)> {
    let (email, _account_uuid, org_uuid) = crate::import::claude_oauth_identity_set_in(profile_dir);
    let identity = org_uuid.filter(|s| !s.is_empty()).or_else(|| {
        email
            .map(|email| email.trim().to_lowercase())
            .filter(|s| !s.is_empty())
    })?;
    Some((identity, "unknown".to_string()))
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
        let executable = which_claude().unwrap_or_else(|| PathBuf::from("claude"));

        TerminalCommand {
            executable,
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

    fn resolve_account(&self, auth_source: AuthSource) -> Result<Account, String> {
        match auth_source {
            AuthSource::CliProfile {
                profile_root,
                ownership,
                expected_identity,
            } => {
                let (identity, _plan) = probe_claude_dir(&profile_root)?;
                Ok(Account {
                    id: format!("claude-{identity}"),
                    provider: Provider::Claude,
                    label: identity.clone(),
                    auth_source: AuthSource::CliProfile {
                        profile_root,
                        ownership,
                        expected_identity,
                    },
                })
            }
            _ => Err("unsupported auth source for claude".to_string()),
        }
    }

    fn managed_profile_root(&self) -> Result<std::path::PathBuf, String> {
        crate::paths::claude_managed_root("default").map_err(|e| e.to_string())
    }
}

/// Probe a Claude directory (sync wrapper around async probe)
fn probe_claude_dir(profile_dir: &Path) -> Result<(String, String), String> {
    if let Some(found) = read_claude_identity_from_json(profile_dir) {
        return Ok(found);
    }

    let profile_str = profile_dir.to_string_lossy().to_string();

    let claude_exe =
        which_claude().ok_or_else(|| "claude executable not found on PATH".to_string())?;

    // Use block_in_place to call async from sync context
    let result = tokio::task::block_in_place(|| {
        let handle = tokio::runtime::Handle::current();
        handle.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                let mut child = tokio::process::Command::new(&claude_exe)
                    .args(["auth", "status", "--json"])
                    .env("CLAUDE_CONFIG_DIR", &profile_str)
                    .env_remove("ANTHROPIC_API_KEY")
                    .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true)
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
            .await
            .map_err(|_| "claude auth status timed out after 10s".to_string())?
        })
    })?;

    parse_claude_status(&result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_claude_identity_from_json_prefers_organization_uuid() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"User@Example.COM","organizationUuid":"org-abc","accountUuid":"acc-1"}}"#,
        )
        .unwrap();

        assert_eq!(
            read_claude_identity_from_json(dir.path()),
            Some(("org-abc".to_string(), "unknown".to_string()))
        );
    }

    #[test]
    fn test_read_claude_identity_from_json_falls_back_to_normalized_email() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"  Foo@Bar.COM "}}"#,
        )
        .unwrap();

        assert_eq!(
            read_claude_identity_from_json(dir.path()),
            Some(("foo@bar.com".to_string(), "unknown".to_string()))
        );
    }

    #[test]
    fn test_read_claude_identity_from_json_returns_none_without_file() {
        let dir = tempfile::tempdir().unwrap();

        assert_eq!(read_claude_identity_from_json(dir.path()), None);
    }

    #[test]
    fn test_read_claude_identity_from_json_returns_none_without_oauth_account() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".claude.json"), r#"{}"#).unwrap();

        assert_eq!(read_claude_identity_from_json(dir.path()), None);
    }

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
