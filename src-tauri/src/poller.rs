//! Per-account usage poller: builds an `AccountUsage` snapshot for every
//! account in the `AccountStore`.
//!
//! Codex/Claude: live HTTP quota using the stored access token; on failure
//! falls back to local-log aggregation.
//! Agy: Antigravity Model Quota — prefer the running app's local
//! `RetrieveUserQuotaSummary`, else Cloud Code OAuth remote fetch. No local
//! SQLite token totals (those are not the UI quota %).
//!
//! SECURITY: never log/print an access token or other credential value.

use std::time::Duration;

use chrono::Utc;
use serde::Serialize;

use usage_core::account::{Account, Credentials, Provider};
use usage_core::aggregate::aggregate;
use usage_core::fetch::agy::{compact_windows, parse_agy_quota_summary, AgyQuota, AgyQuotaPool};
use usage_core::fetch::claude::{parse_claude_usage, ClaudeQuota};
use usage_core::fetch::codex::{parse_codex_usage, CodexQuota};
use usage_core::models::{ModelTokenEvent, QuotaUsage, WindowTotals};
use usage_core::scanners::{claude as claude_scanner, codex as codex_scanner};

use crate::agy_local;
use crate::oauth;
use crate::paths;
use crate::store::AccountStore;

/// Refresh proactively when the access token expires within this window.
const REFRESH_THRESHOLD: Duration = Duration::from_secs(60);

const AGY_USER_AGENT: &str = "antigravity/usagecheck macos/arm64";
const AGY_QUOTA_SUMMARY_URLS: &[&str] = &[
    "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary",
    "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary",
];
const AGY_LOAD_CODE_ASSIST_URLS: &[&str] = &[
    "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
    "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
];

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
/// local-log token totals (Codex/Claude fallback only), ready for the tray.
#[derive(Clone, Debug, Serialize)]
pub struct AccountUsage {
    pub account: Account,
    /// Prefer email / plan-aware name over the stored label when available.
    pub display_name: String,
    pub plan: Option<String>,
    pub five_hour: Option<QuotaUsage>,
    pub week: Option<QuotaUsage>,
    pub totals: WindowTotals,
    /// Agy: Gemini / Claude+GPT pool rows (used %, like Codex/Claude).
    pub pool_breakdown: Vec<AgyQuotaPool>,
    pub status: String,
}

fn display_name_for(account: &Account, email: Option<&str>, plan: Option<&str>) -> String {
    if let Some(email) = email.filter(|s| !s.is_empty()) {
        return email.to_string();
    }
    let label = account.label.trim();
    if !label.is_empty()
        && !label.ends_with(" (CLI import)")
        && !label.ends_with(" account")
        && label != "agy (local logs)"
        && label != "Gemini (local logs)"
        && label != "Gemini"
        && label != "agy"
    {
        return label.to_string();
    }
    if let Some(plan) = plan.filter(|s| !s.is_empty()) {
        return format!("{} · {}", provider_short(account.provider), plan);
    }
    if !label.is_empty()
        && label != "Gemini"
        && label != "agy"
        && label != "agy (local logs)"
        && label != "Gemini (local logs)"
    {
        return label.to_string();
    }
    provider_short(account.provider).to_string()
}

fn provider_short(p: Provider) -> &'static str {
    match p {
        Provider::Codex => "Codex",
        Provider::Claude => "Claude",
        Provider::Agy => "agy",
    }
}

/// Maps a parsed `CodexQuota` + local-log `totals` into an `AccountUsage`
/// with `status = "ok"`. Pure — no I/O.
pub fn account_usage_from_codex(
    account: &Account,
    quota: &CodexQuota,
    totals: WindowTotals,
) -> AccountUsage {
    let mut account = account.clone();
    if let Some(email) = quota.email.as_deref() {
        account.label = email.to_string();
    }
    AccountUsage {
        display_name: display_name_for(
            &account,
            quota.email.as_deref(),
            quota.plan.as_deref(),
        ),
        plan: quota.plan.clone(),
        account,
        five_hour: quota.five_hour.clone(),
        week: quota.week.clone(),
        totals,
        pool_breakdown: Vec::new(),
        status: "ok".to_string(),
    }
}

/// Maps a parsed `ClaudeQuota` + local-log `totals` into an `AccountUsage`
/// with `status = "ok"`. Pure — no I/O.
pub fn account_usage_from_claude(
    account: &Account,
    quota: &ClaudeQuota,
    totals: WindowTotals,
    email: Option<&str>,
    plan: Option<&str>,
) -> AccountUsage {
    AccountUsage {
        display_name: display_name_for(account, email, plan),
        plan: plan.map(str::to_string),
        account: account.clone(),
        five_hour: quota.five_hour.clone(),
        week: quota.week.clone(),
        totals,
        pool_breakdown: Vec::new(),
        status: "ok".to_string(),
    }
}

/// Maps Antigravity Model Quota pools into tray `AccountUsage`.
pub fn account_usage_from_agy(account: &Account, quota: &AgyQuota, status: &str) -> AccountUsage {
    let (five_hour, week) = compact_windows(&quota.pools);
    AccountUsage {
        display_name: display_name_for(account, quota.email.as_deref(), quota.plan.as_deref()),
        plan: quota.plan.clone(),
        account: account.clone(),
        five_hour,
        week,
        totals: WindowTotals::default(),
        pool_breakdown: quota.pools.clone(),
        status: status.to_string(),
    }
}

/// Builds an `AccountUsage` from local-log aggregation only (no live quota).
/// Used as the Codex/Claude fallback when the HTTP fetch fails.
fn account_usage_from_logs(account: &Account, totals: WindowTotals, status: &str) -> AccountUsage {
    AccountUsage {
        display_name: display_name_for(account, None, None),
        plan: None,
        account: account.clone(),
        five_hour: None,
        week: None,
        totals,
        pool_breakdown: Vec::new(),
        status: status.to_string(),
    }
}

/// Reads and aggregates local JSONL logs for Codex/Claude into `WindowTotals`.
fn aggregate_local_logs(provider: Provider) -> Result<WindowTotals, ()> {
    let (roots, parse_line): (
        Vec<std::path::PathBuf>,
        fn(&str) -> Option<ModelTokenEvent>,
    ) = match provider {
        Provider::Codex => (paths::codex_session_roots(), codex_scanner::parse_codex_line),
        Provider::Claude => (paths::claude_project_roots(), claude_scanner::parse_claude_line),
        Provider::Agy => return Err(()),
    };

    if roots.is_empty() {
        return Err(());
    }

    let mut events = Vec::new();
    for root in roots {
        if !root.exists() {
            continue;
        }
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
    }

    Ok(aggregate(&events, Utc::now()))
}

/// Recursively collects `.jsonl` file paths under `root`. Best-effort: read
/// errors on individual directories are skipped rather than propagated.
/// Caps the number of files so tray refresh stays responsive.
fn walk_jsonl(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    const MAX_FILES: usize = 200;
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
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.starts_with("jsonl"))
                .unwrap_or(false)
                || path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.contains("transcript") && n.ends_with(".jsonl"))
                    .unwrap_or(false)
            {
                out.push(path);
                if out.len() >= MAX_FILES {
                    return out;
                }
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
        .header("anthropic-version", "2023-06-01")
        .header("User-Agent", "claude-code/2.1.197")
        .bearer_auth(&creds.access_token);

    let resp = req.send().await.map_err(|_| None)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Some(status.as_u16()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|_| Some(status.as_u16()))?;
    Ok(parse_claude_usage(&body))
}

async fn resolve_agy_project_id(client: &reqwest::Client, access_token: &str) -> Option<String> {
    let body = serde_json::json!({ "metadata": { "ideType": "ANTIGRAVITY" } });
    for url in AGY_LOAD_CODE_ASSIST_URLS {
        let Ok(resp) = client
            .post(*url)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Content-Type", "application/json")
            .header("User-Agent", AGY_USER_AGENT)
            .header(
                "Client-Metadata",
                r#"{"ideType":"ANTIGRAVITY","platform":"MACOS","pluginType":"GEMINI"}"#,
            )
            .json(&body)
            .send()
            .await
        else {
            continue;
        };
        if !resp.status().is_success() {
            continue;
        }
        let Ok(v) = resp.json::<serde_json::Value>().await else {
            continue;
        };
        if let Some(id) = v
            .get("cloudaicompanionProject")
            .or_else(|| v.get("cloudAiCompanionProject"))
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
        {
            return Some(id.to_string());
        }
    }
    None
}

/// Remote Cloud Code `retrieveUserQuotaSummary` using a Google OAuth token.
async fn fetch_agy_quota_remote(
    client: &reqwest::Client,
    creds: &Credentials,
) -> Result<AgyQuota, Option<u16>> {
    let project = resolve_agy_project_id(client, &creds.access_token).await;
    let mut last_status: Option<u16> = None;
    for url in AGY_QUOTA_SUMMARY_URLS {
        let mut body = serde_json::Map::new();
        if let Some(p) = &project {
            body.insert("project".into(), serde_json::Value::String(p.clone()));
        }
        let resp = client
            .post(*url)
            .header("Authorization", format!("Bearer {}", creds.access_token))
            .header("Content-Type", "application/json")
            .header("User-Agent", AGY_USER_AGENT)
            .header(
                "Client-Metadata",
                r#"{"ideType":"ANTIGRAVITY","platform":"MACOS","pluginType":"GEMINI"}"#,
            )
            .json(&serde_json::Value::Object(body))
            .send()
            .await
            .map_err(|_| None)?;
        let status = resp.status();
        if !status.is_success() {
            last_status = Some(status.as_u16());
            continue;
        }
        let root: serde_json::Value = resp.json().await.map_err(|_| Some(status.as_u16()))?;
        let quota = parse_agy_quota_summary(&root);
        if quota.pools.is_empty() {
            last_status = Some(status.as_u16());
            continue;
        }
        return Ok(quota);
    }
    Err(last_status)
}

/// Maps an HTTP failure to a status string: 401/403 (expired/invalid token)
/// -> "needs_login", 429 -> "rate_limited", anything else -> "error".
fn status_for_failure(status: Option<u16>) -> &'static str {
    match status {
        Some(401) | Some(403) => "needs_login",
        Some(429) => "rate_limited",
        _ => "error",
    }
}

async fn poll_agy(store: &AccountStore, client: &reqwest::Client, account: &Account) -> AccountUsage {
    // 1) Prefer live Antigravity.app language_server (same as Model Quota UI).
    if let Some(quota) = agy_local::fetch_local_quota().await {
        if let Some(email) = quota.email.as_deref() {
            store.update_label(&account.id, email);
        }
        return account_usage_from_agy(account, &quota, "ok");
    }

    // 2) OAuth remote Cloud Code fallback.
    match store.credentials(&account.id) {
        Some(creds) if !creds.access_token.is_empty() => {
            let creds = maybe_refresh(store, &account.id, Provider::Agy, creds).await;
            match fetch_agy_quota_remote(client, &creds).await {
                Ok(quota) => account_usage_from_agy(account, &quota, "ok"),
                Err(status) => account_usage_from_agy(
                    account,
                    &AgyQuota {
                        email: None,
                        plan: None,
                        pools: Vec::new(),
                    },
                    status_for_failure(status),
                ),
            }
        }
        _ => account_usage_from_agy(
            account,
            &AgyQuota {
                email: None,
                plan: None,
                pools: Vec::new(),
            },
            "needs_login",
        ),
    }
}

/// Builds the full per-account usage snapshot.
pub async fn poll_all(store: &AccountStore) -> Vec<AccountUsage> {
    let client = reqwest::Client::new();
    let accounts = store.list();
    let mut out = Vec::with_capacity(accounts.len());

    for account in accounts {
        let usage = match account.provider {
            Provider::Agy => poll_agy(store, &client, &account).await,
            Provider::Codex => {
                match store.credentials(&account.id) {
                    Some(creds) if !creds.access_token.is_empty() => {
                        let creds = maybe_refresh(store, &account.id, Provider::Codex, creds).await;
                        match fetch_codex_quota(&client, &creds).await {
                            Ok(quota) => {
                                if let Some(email) = quota.email.as_deref() {
                                    store.update_label(&account.id, email);
                                }
                                account_usage_from_codex(
                                    &account,
                                    &quota,
                                    WindowTotals::default(),
                                )
                            }
                            Err(status) => {
                                let totals = aggregate_local_logs(Provider::Codex).unwrap_or_default();
                                account_usage_from_logs(&account, totals, status_for_failure(status))
                            }
                        }
                    }
                    _ => {
                        let totals = aggregate_local_logs(Provider::Codex).unwrap_or_default();
                        account_usage_from_logs(&account, totals, "needs_login")
                    }
                }
            }
            Provider::Claude => {
                match store.credentials(&account.id) {
                    Some(creds) if !creds.access_token.is_empty() => {
                        let creds = maybe_refresh(store, &account.id, Provider::Claude, creds).await;
                        match fetch_claude_quota(&client, &creds).await {
                            Ok(quota) => {
                                let email = if account.label.contains('@') {
                                    Some(account.label.as_str())
                                } else {
                                    None
                                };
                                account_usage_from_claude(
                                    &account,
                                    &quota,
                                    WindowTotals::default(),
                                    email,
                                    None,
                                )
                            }
                            Err(status) => {
                                let totals =
                                    aggregate_local_logs(Provider::Claude).unwrap_or_default();
                                account_usage_from_logs(&account, totals, status_for_failure(status))
                            }
                        }
                    }
                    _ => {
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
        let acct = Account {
            id: "1".into(),
            provider: Provider::Codex,
            label: "w".into(),
        };
        let quota = CodexQuota {
            plan: Some("prolite".into()),
            email: Some("a@b.com".into()),
            five_hour: Some(QuotaUsage {
                percent: 12.0,
                resets_at: None,
                window_seconds: Some(18000),
            }),
            week: None,
        };
        let au = account_usage_from_codex(&acct, &quota, WindowTotals::default());
        assert_eq!(au.status, "ok");
        assert_eq!(au.display_name, "a@b.com");
        assert_eq!(au.five_hour.as_ref().unwrap().percent, 12.0);
    }

    #[test]
    fn maps_claude_quota_to_account_usage() {
        let acct = Account {
            id: "2".into(),
            provider: Provider::Claude,
            label: "w".into(),
        };
        let quota = ClaudeQuota {
            five_hour: Some(QuotaUsage {
                percent: 30.0,
                resets_at: None,
                window_seconds: None,
            }),
            week: Some(QuotaUsage {
                percent: 55.5,
                resets_at: None,
                window_seconds: None,
            }),
        };
        let au =
            account_usage_from_claude(&acct, &quota, WindowTotals::default(), Some("c@d.com"), None);
        assert_eq!(au.status, "ok");
        assert_eq!(au.display_name, "c@d.com");
        assert_eq!(au.five_hour.as_ref().unwrap().percent, 30.0);
        assert_eq!(au.week.as_ref().unwrap().percent, 55.5);
    }

    #[test]
    fn maps_agy_pools() {
        let acct = Account {
            id: "3".into(),
            provider: Provider::Agy,
            label: "agy".into(),
        };
        let quota = AgyQuota {
            email: Some("a@b.com".into()),
            plan: Some("Pro".into()),
            pools: vec![AgyQuotaPool {
                name: "Gemini Models".into(),
                five_hour: None,
                week: Some(QuotaUsage {
                    percent: 0.0,
                    resets_at: None,
                    window_seconds: Some(604_800),
                }),
            }],
        };
        let au = account_usage_from_agy(&acct, &quota, "ok");
        assert_eq!(au.display_name, "a@b.com");
        assert_eq!(au.pool_breakdown.len(), 1);
        assert!((au.week.as_ref().unwrap().percent - 0.0).abs() < 0.01);
    }

    #[test]
    fn status_for_failure_maps_auth_errors() {
        assert_eq!(status_for_failure(Some(401)), "needs_login");
        assert_eq!(status_for_failure(Some(403)), "needs_login");
        assert_eq!(status_for_failure(Some(429)), "rate_limited");
        assert_eq!(status_for_failure(Some(500)), "error");
        assert_eq!(status_for_failure(None), "error");
    }
}
