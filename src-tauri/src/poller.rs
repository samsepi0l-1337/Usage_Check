//! Per-account usage poller: builds an `AccountUsage` snapshot for every
//! account in the `AccountStore`.
//!
//! Codex/Claude: primary source is a live HTTP quota call using the stored
//! access token; on failure (network error, non-200, expired/invalid token)
//! falls back to local-log aggregation and reports a status describing why.
//! Agy: no discoverable quota API (Task 9) — always uses local-log
//! aggregation only; `five_hour`/`week` stay `None`.
//!
//! SECURITY: never log/print an access token or other credential value.

use std::time::Duration;

use chrono::Utc;
use serde::Serialize;

use usage_core::account::{Account, Credentials, Provider};
use usage_core::aggregate::aggregate;
use usage_core::fetch::claude::{parse_claude_usage, ClaudeQuota};
use usage_core::fetch::codex::{parse_codex_usage, CodexQuota};
use usage_core::models::{ModelTokenEvent, QuotaUsage, WindowTotals};
use usage_core::scanners::{claude as claude_scanner, codex as codex_scanner, gemini as gemini_scanner};

use crate::oauth;
use crate::store::AccountStore;

/// Refresh proactively when the access token expires within this window.
const REFRESH_THRESHOLD: Duration = Duration::from_secs(60);

/// If `creds.expires_at` is within `REFRESH_THRESHOLD` of now (or already
/// past), attempts a proactive refresh via `oauth::refresh_access_token` and
/// persists the result via `store.update_credentials`. On any failure
/// (no refresh_token, network error, non-200), the original `creds` are
/// returned unchanged — the existing 401/403 fallback in `poll_all` still
/// applies to a stale token that couldn't be refreshed.
async fn maybe_refresh(store: &AccountStore, id: &str, provider: Provider, creds: Credentials) -> Credentials {
    if !oauth::should_refresh(creds.expires_at, Utc::now(), REFRESH_THRESHOLD) {
        return creds;
    }

    match oauth::refresh_access_token(provider, &creds).await {
        Ok(refreshed) => {
            store.update_credentials(id, &refreshed);
            refreshed
        }
        Err(_) => creds,
    }
}

/// A single account's usage snapshot: live quota (when available) plus
/// local-log token totals, ready to render as a card.
#[derive(Clone, Debug, Serialize)]
pub struct AccountUsage {
    pub account: Account,
    pub five_hour: Option<QuotaUsage>,
    pub week: Option<QuotaUsage>,
    pub totals: WindowTotals,
    pub status: String,
}

/// Maps a parsed `CodexQuota` + local-log `totals` into an `AccountUsage`
/// with `status = "ok"`. Pure — no I/O.
pub fn account_usage_from_codex(account: &Account, quota: &CodexQuota, totals: WindowTotals) -> AccountUsage {
    AccountUsage {
        account: account.clone(),
        five_hour: quota.five_hour.clone(),
        week: quota.week.clone(),
        totals,
        status: "ok".to_string(),
    }
}

/// Maps a parsed `ClaudeQuota` + local-log `totals` into an `AccountUsage`
/// with `status = "ok"`. Pure — no I/O.
pub fn account_usage_from_claude(account: &Account, quota: &ClaudeQuota, totals: WindowTotals) -> AccountUsage {
    AccountUsage {
        account: account.clone(),
        five_hour: quota.five_hour.clone(),
        week: quota.week.clone(),
        totals,
        status: "ok".to_string(),
    }
}

/// Builds an `AccountUsage` from local-log aggregation only (no live quota).
/// Used for agy (no quota API — Task 9) and as the Codex/Claude fallback
/// path when the HTTP fetch fails.
fn account_usage_from_logs(account: &Account, totals: WindowTotals, status: &str) -> AccountUsage {
    AccountUsage {
        account: account.clone(),
        five_hour: None,
        week: None,
        totals,
        status: status.to_string(),
    }
}

/// Reads and aggregates local JSONL logs for a provider into `WindowTotals`.
/// Returns `Err` if the log directory could not be read at all (still
/// distinguished from "read fine, zero events").
fn aggregate_local_logs(provider: Provider) -> Result<WindowTotals, ()> {
    let home = match dirs_home() {
        Some(h) => h,
        None => return Err(()),
    };

    let (root, parse_line): (std::path::PathBuf, fn(&str) -> Option<ModelTokenEvent>) = match provider {
        Provider::Codex => (home.join(".codex/sessions"), codex_scanner::parse_codex_line),
        Provider::Claude => (home.join(".claude/projects"), claude_scanner::parse_claude_line),
        Provider::Agy => (home.join(".gemini"), gemini_scanner::parse_gemini_line),
    };

    if !root.exists() {
        // No logs yet is not an error — just zero totals.
        return Ok(WindowTotals::default());
    }

    let mut events = Vec::new();
    for path in walk_jsonl(&root) {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in content.lines() {
            if let Some(ev) = parse_line(line) {
                events.push(ev);
            }
        }
    }

    Ok(aggregate(&events, Utc::now()))
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

/// Recursively collects `.jsonl` file paths under `root`. Best-effort: read
/// errors on individual directories are skipped rather than propagated.
fn walk_jsonl(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()).map(|e| e.starts_with("jsonl")).unwrap_or(false)
                || path.file_name().and_then(|n| n.to_str()).map(|n| n.contains("transcript") && n.ends_with(".jsonl")).unwrap_or(false)
            {
                out.push(path);
            }
        }
    }
    out
}

/// Fetches live Codex quota via HTTP using `creds.access_token`. Returns
/// `Ok(CodexQuota)` on a 200 response, `Err(status_code)` otherwise
/// (`None` status = network/transport failure, not an HTTP error).
async fn fetch_codex_quota(client: &reqwest::Client, creds: &Credentials) -> Result<CodexQuota, Option<u16>> {
    let mut req = client
        .get("https://chatgpt.com/backend-api/wham/usage")
        .header("Accept", "application/json")
        .header("User-Agent", "UsageCheck")
        .bearer_auth(&creds.access_token);
    if let Some(account_id) = &creds.account_id {
        req = req.header("ChatGPT-Account-Id", account_id);
    }

    let resp = req.send().await.map_err(|_| None)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Some(status.as_u16()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|_| Some(status.as_u16()))?;
    Ok(parse_codex_usage(&body))
}

/// Fetches live Claude quota via HTTP using `creds.access_token`. Same
/// success/error shape as `fetch_codex_quota`.
async fn fetch_claude_quota(client: &reqwest::Client, creds: &Credentials) -> Result<ClaudeQuota, Option<u16>> {
    let req = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Accept", "application/json")
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", "UsageCheck")
        .bearer_auth(&creds.access_token);

    let resp = req.send().await.map_err(|_| None)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Some(status.as_u16()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|_| Some(status.as_u16()))?;
    Ok(parse_claude_usage(&body))
}

/// Maps an HTTP failure to a status string: 401/403 (expired/invalid token)
/// -> "needs_login", anything else (network error, 5xx, etc.) -> "error".
fn status_for_failure(status: Option<u16>) -> &'static str {
    match status {
        Some(401) | Some(403) => "needs_login",
        _ => "error",
    }
}

/// Builds the full per-account usage snapshot: for each account in `store`,
/// fetches live quota (Codex/Claude) or falls back to local-log aggregation
/// (always for agy, per Task 9's finding that it has no quota API).
pub async fn poll_all(store: &AccountStore) -> Vec<AccountUsage> {
    let client = reqwest::Client::new();
    let accounts = store.list();
    let mut out = Vec::with_capacity(accounts.len());

    for account in accounts {
        let usage = match account.provider {
            Provider::Agy => {
                let totals = aggregate_local_logs(Provider::Agy).unwrap_or_default();
                account_usage_from_logs(&account, totals, "ok")
            }
            Provider::Codex => {
                match store.credentials(&account.id) {
                    Some(creds) => {
                        let creds = maybe_refresh(store, &account.id, Provider::Codex, creds).await;
                        match fetch_codex_quota(&client, &creds).await {
                            Ok(quota) => {
                                let totals = aggregate_local_logs(Provider::Codex).unwrap_or_default();
                                account_usage_from_codex(&account, &quota, totals)
                            }
                            Err(status) => {
                                let totals = aggregate_local_logs(Provider::Codex).unwrap_or_default();
                                account_usage_from_logs(&account, totals, status_for_failure(status))
                            }
                        }
                    }
                    None => {
                        let totals = aggregate_local_logs(Provider::Codex).unwrap_or_default();
                        account_usage_from_logs(&account, totals, "needs_login")
                    }
                }
            }
            Provider::Claude => {
                match store.credentials(&account.id) {
                    Some(creds) => {
                        let creds = maybe_refresh(store, &account.id, Provider::Claude, creds).await;
                        match fetch_claude_quota(&client, &creds).await {
                            Ok(quota) => {
                                let totals = aggregate_local_logs(Provider::Claude).unwrap_or_default();
                                account_usage_from_claude(&account, &quota, totals)
                            }
                            Err(status) => {
                                let totals = aggregate_local_logs(Provider::Claude).unwrap_or_default();
                                account_usage_from_logs(&account, totals, status_for_failure(status))
                            }
                        }
                    }
                    None => {
                        let totals = aggregate_local_logs(Provider::Claude).unwrap_or_default();
                        account_usage_from_logs(&account, totals, "needs_login")
                    }
                }
            }
        };
        out.push(usage);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use usage_core::account::{Account, Provider};
    use usage_core::fetch::codex::CodexQuota;
    use usage_core::models::{QuotaUsage, WindowTotals};

    #[test]
    fn maps_codex_quota_to_account_usage() {
        let acct = Account { id: "1".into(), provider: Provider::Codex, label: "w".into() };
        let quota = CodexQuota {
            plan: None,
            five_hour: Some(QuotaUsage { percent: 12.0, resets_at: None, window_seconds: None }),
            week: None,
        };
        let au = account_usage_from_codex(&acct, &quota, WindowTotals::default());
        assert_eq!(au.status, "ok");
        assert_eq!(au.five_hour.as_ref().unwrap().percent, 12.0);
    }

    #[test]
    fn maps_claude_quota_to_account_usage() {
        let acct = Account { id: "2".into(), provider: Provider::Claude, label: "w".into() };
        let quota = ClaudeQuota {
            five_hour: Some(QuotaUsage { percent: 30.0, resets_at: None, window_seconds: None }),
            week: Some(QuotaUsage { percent: 55.5, resets_at: None, window_seconds: None }),
        };
        let au = account_usage_from_claude(&acct, &quota, WindowTotals::default());
        assert_eq!(au.status, "ok");
        assert_eq!(au.five_hour.as_ref().unwrap().percent, 30.0);
        assert_eq!(au.week.as_ref().unwrap().percent, 55.5);
    }

    #[test]
    fn status_for_failure_maps_auth_errors() {
        assert_eq!(status_for_failure(Some(401)), "needs_login");
        assert_eq!(status_for_failure(Some(403)), "needs_login");
        assert_eq!(status_for_failure(Some(500)), "error");
        assert_eq!(status_for_failure(None), "error");
    }
}
