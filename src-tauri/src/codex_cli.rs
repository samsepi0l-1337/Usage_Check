use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::cli_auth::ProviderAdapter;
use crate::terminal::TerminalCommand;
use serde_json::Value;
use usage_core::account::{Account, AuthSource, ProfileOwnership};
use usage_core::fetch::codex::{
    parse_app_server_account, parse_app_server_rate_limits, AppServerAccount,
};

/// Probe result from Codex app-server
#[derive(Debug, Clone)]
pub struct CodexProbe {
    pub account: AppServerAccount,
    pub primary: Option<usage_core::models::QuotaUsage>,
    pub secondary: Option<usage_core::models::QuotaUsage>,
}

/// Resolve the Codex executable on PATH (no `which` crate; use env::split_paths)
fn which_codex() -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(if cfg!(windows) { "codex.exe" } else { "codex" });
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Extract JSONL exchange into a testable function that consumes an async reader.
/// Sends requests and reads responses, matching responses by id.
/// FIX: pass WHOLE line object to parsers (they unwrap "result" once).
pub async fn probe_codex_exchange<R, W>(mut reader: R, mut writer: W) -> Result<CodexProbe, String>
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    // Send initialization requests
    let requests = [
        r#"{"method":"initialize","id":1,"params":{"clientInfo":{"name":"usagecheck","title":"UsageCheck","version":"0.1.4"},"capabilities":null}}"#,
        r#"{"method":"initialized"}"#,
        r#"{"method":"account/read","id":2,"params":{"refreshToken":true}}"#,
        r#"{"method":"account/rateLimits/read","id":3}"#,
    ];

    for req in &requests {
        writer
            .write_all(req.as_bytes())
            .await
            .map_err(|e| format!("failed to write request: {}", e))?;
        writer
            .write_all(b"\n")
            .await
            .map_err(|e| format!("failed to write newline: {}", e))?;
    }
    drop(writer);

    // FIX BUG 2: Replace `while reader.read_line(&mut line).await.ok() == Some(1)` with proper loop.
    // read_line() returns byte count: Ok(0) = EOF, Ok(n>0) = line.
    // The old code == Some(1) only matched 1-byte lines, causing it to exit on any real line.
    let mut account: Option<AppServerAccount> = None;
    let mut rate_limits: Option<(
        Option<usage_core::models::QuotaUsage>,
        Option<usage_core::models::QuotaUsage>,
    )> = None;
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break, // EOF
            Ok(_) => {
                // Got a line (>0 bytes)
                if line.trim().is_empty() {
                    continue;
                }

                if let Ok(obj) = serde_json::from_str::<Value>(&line) {
                    if let Some(id) = obj.get("id").and_then(|v| v.as_i64()) {
                        match id {
                            2 => {
                                // FIX BUG 1: Pass WHOLE line object; parser unwraps "result" once.
                                // Old code: obj.get("result") then passed inner object to parser.
                                // Parser also called .get("result") → double-nest failure.
                                match parse_app_server_account(&obj) {
                                    Ok(acc) => account = Some(acc),
                                    Err(_) => {
                                        // FIX BUG 4: Propagate rejection (e.g., null identity, API-key).
                                        // Old code: .ok() swallowed rejection as None.
                                        return Err("identity rejected in response".to_string());
                                    }
                                }
                            }
                            3 => {
                                // FIX BUG 1: Pass WHOLE line object to parser.
                                // Rate-limit absence is OK (None result), but parse errors still propagate.
                                match parse_app_server_rate_limits(&obj) {
                                    Ok((prim, sec)) => rate_limits = Some((prim, sec)),
                                    Err(e) => {
                                        return Err(format!("rate limits parse error: {}", e));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }

                // Break once both responses received
                if account.is_some() && rate_limits.is_some() {
                    break;
                }
            }
            Err(_) => break, // I/O error
        }
    }

    let account = account.ok_or_else(|| "no account found in response".to_string())?;
    let (primary, secondary) =
        rate_limits.ok_or_else(|| "no rate limits found in response".to_string())?;

    Ok(CodexProbe {
        account,
        primary,
        secondary,
    })
}

/// Probe Codex at a given profile root by launching `codex app-server --stdio`
/// with a 10-second timeout. Returns CodexProbe with account and rate limits.
pub async fn probe_codex(profile_root: &Path) -> Result<CodexProbe, String> {
    let codex_exe = which_codex().ok_or_else(|| "codex not found on PATH".to_string())?;

    let mut child = tokio::process::Command::new(&codex_exe)
        .arg("app-server")
        .arg("--stdio")
        .env("CODEX_HOME", profile_root)
        .env_remove("CODEX_ACCESS_TOKEN")
        .env_remove("OPENAI_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn codex app-server: {}", e))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "failed to open stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to open stdout".to_string())?;

    // Read responses with 10-second timeout
    let future = probe_codex_exchange(BufReader::new(stdout), stdin);

    tokio::time::timeout(std::time::Duration::from_secs(10), future)
        .await
        .map_err(|_| "codex app-server timed out (10s)".to_string())?
}

/// Build account from probe result
fn account_from_probe(
    probe: CodexProbe,
    profile_root: std::path::PathBuf,
    ownership: ProfileOwnership,
) -> Account {
    Account {
        id: probe.account.id.clone(),
        provider: usage_core::account::Provider::Codex,
        label: probe.account.id.clone(),
        auth_source: AuthSource::CliProfile {
            profile_root,
            ownership,
            expected_identity: probe.account.id,
        },
    }
}

/// Codex CLI adapter for provider-based authentication
pub struct CodexCliAdapter;

impl ProviderAdapter for CodexCliAdapter {
    fn probe(&self) -> Result<Option<Account>, String> {
        // FIX BUG 3: Use block_in_place to safely call block_on from within a Tokio runtime.
        // The app runs in Tauri's multi-threaded Tokio runtime; block_in_place temporarily
        // yields the thread to prevent a "cannot start a runtime from within a runtime" panic.
        // This is safe on multi-threaded runtimes and required for sync adapter methods.

        // Try default CODEX_HOME first
        if let Some(default_home) = crate::paths::codex_default_home() {
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(probe_codex(&default_home))
            });
            if let Ok(probe) = result {
                return Ok(Some(account_from_probe(
                    probe,
                    default_home,
                    ProfileOwnership::External,
                )));
            }
        }

        // Try managed root
        if let Some(managed_root) = crate::paths::codex_managed_root() {
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(probe_codex(&managed_root))
            });
            if let Ok(probe) = result {
                return Ok(Some(account_from_probe(
                    probe,
                    managed_root,
                    ProfileOwnership::Managed,
                )));
            }
        }

        Ok(None)
    }

    fn managed_profile_root(&self) -> Result<std::path::PathBuf, String> {
        crate::paths::codex_managed_root()
            .ok_or_else(|| "codex managed root unavailable".to_string())
    }

    fn login_command(&self, profile_root: &Path) -> TerminalCommand {
        TerminalCommand {
            executable: std::path::PathBuf::from("codex"),
            args: vec![OsString::from("login")],
            env: vec![(
                OsString::from("CODEX_HOME"),
                OsString::from(profile_root.to_string_lossy().to_string()),
            )],
            env_remove: vec![
                OsString::from("CODEX_ACCESS_TOKEN"),
                OsString::from("OPENAI_API_KEY"),
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
                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(probe_codex(&profile_root))
                });
                let probe = result?;
                Ok(Account {
                    id: probe.account.id.clone(),
                    provider: usage_core::account::Provider::Codex,
                    label: probe.account.id.clone(),
                    auth_source: AuthSource::CliProfile {
                        profile_root,
                        ownership,
                        expected_identity,
                    },
                })
            }
            _ => Err("unsupported auth source for codex".to_string()),
        }
    }
}

#[cfg(test)]
#[path = "codex_cli_tests.rs"]
mod tests;
