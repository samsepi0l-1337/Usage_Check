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
#[cfg(feature = "edition-pro")]
use usage_core::fetch::cursor::{cursor_quota_with_auth, parse_cursor_period_usage, CursorQuota};
#[cfg(feature = "edition-pro")]
use usage_core::fetch::grok::{parse_grok_prepaid_balance, GrokPrepaid};
#[cfg(feature = "edition-pro")]
use usage_core::fetch::higgsfield::{parse_higgsfield_account, HiggsfieldCredits};
use usage_core::models::{ModelTokenEvent, QuotaUsage, WindowTotals};
use usage_core::scanners::{claude as claude_scanner, codex as codex_scanner};

use crate::agy_local;
use crate::import::{self, ImportedAccount};
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

/// When `codex login` rotates tokens in `~/.codex/auth.json`, UsageCheck's
/// file-backed copy can keep the old access_token (same identity). Prefer the
/// CLI snapshot for the *matching* account only — never overwrite siblings.
fn sync_codex_creds_from_cli(
    store: &AccountStore,
    account: &Account,
    stored: Credentials,
    cli: Option<&ImportedAccount>,
) -> Credentials {
    let Some(imported) = cli else {
        return stored;
    };
    if !import::codex_cli_creds_are_newer(
        &stored,
        &account.label,
        &imported.credentials,
        &imported.label,
    ) {
        return stored;
    }
    store.update_credentials(&account.id, &imported.credentials);
    if !imported.label.is_empty() && imported.label != "Codex" {
        store.update_label(&account.id, &imported.label);
    }
    imported.credentials.clone()
}

/// When `claude` login rotates Keychain tokens, UsageCheck's file-backed copy
/// can keep a still-unexpired but revoked access_token. Prefer the CLI
/// snapshot for the *matching* account only — never overwrite siblings.
fn sync_claude_creds_from_cli(
    store: &AccountStore,
    account: &Account,
    stored: Credentials,
    cli: Option<&ImportedAccount>,
) -> Credentials {
    let Some(imported) = cli else {
        return stored;
    };
    if !import::claude_cli_creds_are_newer(
        &stored,
        &account.label,
        &imported.credentials,
        &imported.label,
    ) {
        return stored;
    }
    store.update_credentials(&account.id, &imported.credentials);
    if !imported.label.is_empty() && imported.label != "Claude" {
        store.update_label(&account.id, &imported.label);
    }
    imported.credentials.clone()
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
    /// Pro providers: secondary label (`$12 left`, `809 credits left`).
    pub detail_suffix: Option<String>,
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
    p.display_name()
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
        detail_suffix: None,
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
        detail_suffix: None,
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
        detail_suffix: None,
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
        detail_suffix: None,
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
        #[cfg(feature = "edition-pro")]
        Provider::Cursor | Provider::Grok | Provider::Higgsfield => return Err(()),
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

#[cfg(feature = "edition-pro")]
const CURSOR_API_BASE: &str = "https://api2.cursor.sh";
#[cfg(feature = "edition-pro")]
const CURSOR_OAUTH_CLIENT_ID: &str = "KbZUR41cY7W6zRSdpSUJ7I7mLYBKOCmB";

#[cfg(feature = "edition-pro")]
async fn refresh_cursor_access_token(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<String, ()> {
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": CURSOR_OAUTH_CLIENT_ID,
        "refresh_token": refresh_token,
    });
    let resp = client
        .post(format!("{CURSOR_API_BASE}/oauth/token"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|_| ())?;
    if !resp.status().is_success() {
        return Err(());
    }
    let root: serde_json::Value = resp.json().await.map_err(|_| ())?;
    if root.get("shouldLogout").and_then(|v| v.as_bool()).unwrap_or(false) {
        return Err(());
    }
    root.get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or(())
}

#[cfg(feature = "edition-pro")]
async fn fetch_cursor_quota(
    client: &reqwest::Client,
    creds: &Credentials,
) -> Result<CursorQuota, Option<u16>> {
    let resp = client
        .post(format!(
            "{CURSOR_API_BASE}/aiserver.v1.DashboardService/GetCurrentPeriodUsage"
        ))
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("Connect-Protocol-Version", "1")
        .header("User-Agent", "UsageCheck")
        .bearer_auth(&creds.access_token)
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|_| None)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Some(status.as_u16()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|_| Some(status.as_u16()))?;
    Ok(parse_cursor_period_usage(&body))
}

#[cfg(feature = "edition-pro")]
async fn fetch_grok_prepaid(
    client: &reqwest::Client,
    creds: &Credentials,
) -> Result<GrokPrepaid, Option<u16>> {
    let team_id = creds
        .account_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or(None)?;
    let url = format!(
        "https://management-api.x.ai/v1/billing/teams/{team_id}/prepaid/balance"
    );
    let resp = client
        .get(url)
        .header("Accept", "application/json")
        .header("User-Agent", "UsageCheck")
        .bearer_auth(&creds.access_token)
        .send()
        .await
        .map_err(|_| None)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Some(status.as_u16()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|_| Some(status.as_u16()))?;
    Ok(parse_grok_prepaid_balance(&body))
}

#[cfg(feature = "edition-pro")]
fn account_usage_from_cursor(account: &Account, quota: &CursorQuota, status: &str) -> AccountUsage {
    AccountUsage {
        display_name: display_name_for(account, quota.email.as_deref(), quota.plan.as_deref()),
        plan: quota.plan.clone(),
        account: account.clone(),
        five_hour: None,
        week: quota.period.clone(),
        totals: WindowTotals::default(),
        pool_breakdown: Vec::new(),
        detail_suffix: quota.detail_suffix.clone(),
        status: status.to_string(),
    }
}

#[cfg(feature = "edition-pro")]
fn account_usage_from_grok(account: &Account, prepaid: &GrokPrepaid, status: &str) -> AccountUsage {
    AccountUsage {
        display_name: display_name_for(account, None, Some("API credits")),
        plan: Some("API credits".into()),
        account: account.clone(),
        five_hour: None,
        week: prepaid.period.clone(),
        totals: WindowTotals::default(),
        pool_breakdown: Vec::new(),
        detail_suffix: prepaid.detail_suffix.clone(),
        status: status.to_string(),
    }
}

#[cfg(feature = "edition-pro")]
fn account_usage_from_higgsfield(
    account: &Account,
    credits: &HiggsfieldCredits,
    status: &str,
) -> AccountUsage {
    AccountUsage {
        display_name: display_name_for(account, credits.email.as_deref(), credits.plan.as_deref()),
        plan: credits.plan.clone(),
        account: account.clone(),
        five_hour: None,
        week: credits.to_quota(),
        totals: WindowTotals::default(),
        pool_breakdown: Vec::new(),
        detail_suffix: credits.detail_suffix(),
        status: status.to_string(),
    }
}

#[cfg(feature = "edition-pro")]
fn sync_cursor_creds_from_local(
    store: &AccountStore,
    account: &Account,
    stored: Credentials,
) -> Credentials {
    let Ok(imported) = crate::cursor_local::load_cursor_local_auth() else {
        return stored;
    };
    if imported.credentials.access_token.is_empty()
        || imported.credentials.access_token == stored.access_token
    {
        return stored;
    }
    if !import::cli_identity_matches(
        &stored,
        &account.label,
        &imported.credentials,
        &imported.label,
    ) && !account.label.eq_ignore_ascii_case(&imported.label)
    {
        return stored;
    }
    store.update_credentials(&account.id, &imported.credentials);
    if !imported.label.is_empty() {
        store.update_label(&account.id, &imported.label);
    }
    imported.credentials
}

#[cfg(feature = "edition-pro")]
async fn poll_cursor(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
    let Some(mut creds) = store.credentials(&account.id) else {
        return account_usage_from_cursor(
            account,
            &CursorQuota {
                email: None,
                plan: None,
                period: None,
                detail_suffix: None,
            },
            "needs_login",
        );
    };
    creds = sync_cursor_creds_from_local(store, account, creds);
    if let Some(refresh) = creds.refresh_token.as_deref() {
        if let Ok(new_token) = refresh_cursor_access_token(client, refresh).await {
            creds.access_token = new_token;
            store.update_credentials(&account.id, &creds);
        }
    }

    let plan = creds.account_id.clone();
    match fetch_cursor_quota(client, &creds).await {
        Ok(mut quota) => {
            quota = cursor_quota_with_auth(
                quota,
                if account.label.contains('@') {
                    Some(account.label.clone())
                } else {
                    None
                },
                plan,
            );
            account_usage_from_cursor(account, &quota, "ok")
        }
        Err(status) => account_usage_from_cursor(
            account,
            &CursorQuota {
                email: None,
                plan: creds.account_id.clone(),
                period: None,
                detail_suffix: None,
            },
            status_for_failure(status),
        ),
    }
}

#[cfg(feature = "edition-pro")]
async fn poll_grok(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
    let Some(creds) = store.credentials(&account.id) else {
        return account_usage_from_grok(
            account,
            &GrokPrepaid {
                period: None,
                detail_suffix: Some("needs setup".into()),
            },
            "needs_login",
        );
    };
    match fetch_grok_prepaid(client, &creds).await {
        Ok(prepaid) => account_usage_from_grok(account, &prepaid, "ok"),
        Err(status) => account_usage_from_grok(
            account,
            &GrokPrepaid {
                period: None,
                detail_suffix: None,
            },
            status_for_failure(status),
        ),
    }
}

#[cfg(feature = "edition-pro")]
fn fetch_higgsfield_account_json() -> Result<serde_json::Value, ()> {
    use std::process::Command;

    let output = Command::new("higgsfield")
        .args(["account", "--json"])
        .output()
        .map_err(|_| ())?;
    if !output.status.success() {
        return Err(());
    }
    serde_json::from_slice(&output.stdout).map_err(|_| ())
}

#[cfg(feature = "edition-pro")]
async fn poll_higgsfield(store: &AccountStore, account: &Account) -> AccountUsage {
    if store.credentials(&account.id).is_none() {
        return account_usage_from_higgsfield(
            account,
            &HiggsfieldCredits {
                email: None,
                plan: None,
                credits_remaining: None,
                credits_total: None,
            },
            "needs_login",
        );
    }

    match fetch_higgsfield_account_json() {
        Ok(root) => {
            let credits = parse_higgsfield_account(&root);
            let status = if credits.credits_remaining.is_some() {
                "ok"
            } else {
                "needs_setup"
            };
            account_usage_from_higgsfield(account, &credits, status)
        }
        Err(()) => account_usage_from_higgsfield(
            account,
            &HiggsfieldCredits {
                email: None,
                plan: None,
                credits_remaining: None,
                credits_total: None,
            },
            "needs_setup",
        ),
    }
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

/// Backfills Google `account_id` / email label for legacy agy rows that were
/// saved before identity was persisted (opaque `ya29` tokens, no `id_token`).
async fn enrich_agy_identity(store: &AccountStore, account: &Account, creds: &mut Credentials) {
    let needs_id = creds
        .account_id
        .as_deref()
        .map(|s| s.is_empty())
        .unwrap_or(true);
    let needs_label = !account.label.contains('@');
    if !needs_id && !needs_label {
        return;
    }
    let Some(identity) = oauth::agy_identity_from_access_token(&creds.access_token).await else {
        return;
    };
    let mut changed = false;
    if needs_id {
        if let Some(id) = identity.account_id.filter(|s| !s.is_empty()) {
            creds.account_id = Some(id);
            changed = true;
        }
    }
    if changed {
        store.update_credentials(&account.id, creds);
    }
    if needs_label {
        if let Some(email) = identity.email.filter(|s| !s.is_empty()) {
            store.update_label(&account.id, &email);
        }
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
            let mut creds = maybe_refresh(store, &account.id, Provider::Agy, creds).await;
            enrich_agy_identity(store, account, &mut creds).await;
            match fetch_agy_quota_remote(client, &creds).await {
                Ok(mut quota) => {
                    // Remote summary has no email; use stored label / userinfo.
                    if quota.email.is_none() {
                        let label = store
                            .list()
                            .into_iter()
                            .find(|a| a.id == account.id)
                            .map(|a| a.label)
                            .unwrap_or_else(|| account.label.clone());
                        if label.contains('@') {
                            quota.email = Some(label);
                        }
                    }
                    if let Some(email) = quota.email.as_deref() {
                        store.update_label(&account.id, email);
                    }
                    account_usage_from_agy(account, &quota, "ok")
                }
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

async fn poll_codex(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
    cli: Option<&ImportedAccount>,
) -> AccountUsage {
    match store.credentials(&account.id) {
        Some(creds) if !creds.access_token.is_empty() => {
            let creds = sync_codex_creds_from_cli(store, account, creds, cli);
            let creds = maybe_refresh(store, &account.id, Provider::Codex, creds).await;
            match fetch_codex_quota(client, &creds).await {
                Ok(quota) => {
                    if let Some(email) = quota.email.as_deref() {
                        store.update_label(&account.id, email);
                    }
                    account_usage_from_codex(account, &quota, WindowTotals::default())
                }
                Err(status) => {
                    // Auth failure for the matching identity: re-read auth.json
                    // once more (covers race where CLI wrote mid-poll) and retry
                    // before falling back to local logs. Never adopts a
                    // different CLI account onto this row.
                    if matches!(status, Some(401) | Some(403)) {
                        if let Ok(fresh) = import::load_codex_cli_auth() {
                            let retried = sync_codex_creds_from_cli(
                                store,
                                account,
                                creds.clone(),
                                Some(&fresh),
                            );
                            if retried.access_token != creds.access_token {
                                if let Ok(quota) = fetch_codex_quota(client, &retried).await {
                                    if let Some(email) = quota.email.as_deref() {
                                        store.update_label(&account.id, email);
                                    }
                                    return account_usage_from_codex(
                                        account,
                                        &quota,
                                        WindowTotals::default(),
                                    );
                                }
                            }
                        }
                    }
                    let totals = aggregate_local_logs(Provider::Codex).unwrap_or_default();
                    account_usage_from_logs(account, totals, status_for_failure(status))
                }
            }
        }
        _ => {
            // No usable stored token: adopt CLI auth only when identity matches
            // this row. Never wipe/replace siblings or invent a match.
            if let Some(imported) = cli {
                let stored = store.credentials(&account.id).unwrap_or(Credentials {
                    access_token: String::new(),
                    refresh_token: None,
                    account_id: None,
                    expires_at: None,
                });
                let adopt = import::cli_identity_matches(
                    &stored,
                    &account.label,
                    &imported.credentials,
                    &imported.label,
                );
                if adopt && !imported.credentials.access_token.is_empty() {
                    store.update_credentials(&account.id, &imported.credentials);
                    if !imported.label.is_empty() && imported.label != "Codex" {
                        store.update_label(&account.id, &imported.label);
                    }
                    let creds = maybe_refresh(
                        store,
                        &account.id,
                        Provider::Codex,
                        imported.credentials.clone(),
                    )
                    .await;
                    if let Ok(quota) = fetch_codex_quota(client, &creds).await {
                        if let Some(email) = quota.email.as_deref() {
                            store.update_label(&account.id, email);
                        }
                        return account_usage_from_codex(
                            account,
                            &quota,
                            WindowTotals::default(),
                        );
                    }
                }
            }
            let totals = aggregate_local_logs(Provider::Codex).unwrap_or_default();
            account_usage_from_logs(account, totals, "needs_login")
        }
    }
}

async fn poll_claude(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
    cli: Option<&ImportedAccount>,
) -> AccountUsage {
    match store.credentials(&account.id) {
        Some(creds) if !creds.access_token.is_empty() => {
            let creds = sync_claude_creds_from_cli(store, account, creds, cli);
            let creds = maybe_refresh(store, &account.id, Provider::Claude, creds).await;
            match fetch_claude_quota(client, &creds).await {
                Ok(quota) => {
                    let email = if account.label.contains('@') {
                        Some(account.label.as_str())
                    } else {
                        None
                    };
                    account_usage_from_claude(
                        account,
                        &quota,
                        WindowTotals::default(),
                        email,
                        None,
                    )
                }
                Err(status) => {
                    // Matching-identity token revoked: re-read Keychain once
                    // and retry on 401/403. Never adopts a different CLI
                    // account onto this row.
                    if matches!(status, Some(401) | Some(403)) {
                        if let Ok(fresh) = import::load_claude_cli_auth() {
                            let retried = sync_claude_creds_from_cli(
                                store,
                                account,
                                creds.clone(),
                                Some(&fresh),
                            );
                            if retried.access_token != creds.access_token {
                                if let Ok(quota) = fetch_claude_quota(client, &retried).await {
                                    let email = if !fresh.label.is_empty()
                                        && fresh.label.contains('@')
                                    {
                                        Some(fresh.label.as_str())
                                    } else if account.label.contains('@') {
                                        Some(account.label.as_str())
                                    } else {
                                        None
                                    };
                                    return account_usage_from_claude(
                                        account,
                                        &quota,
                                        WindowTotals::default(),
                                        email,
                                        None,
                                    );
                                }
                            }
                        }
                    }
                    let totals = aggregate_local_logs(Provider::Claude).unwrap_or_default();
                    account_usage_from_logs(account, totals, status_for_failure(status))
                }
            }
        }
        _ => {
            if let Some(imported) = cli {
                let stored = store.credentials(&account.id).unwrap_or(Credentials {
                    access_token: String::new(),
                    refresh_token: None,
                    account_id: None,
                    expires_at: None,
                });
                let adopt = import::cli_identity_matches(
                    &stored,
                    &account.label,
                    &imported.credentials,
                    &imported.label,
                );
                if adopt && !imported.credentials.access_token.is_empty() {
                    store.update_credentials(&account.id, &imported.credentials);
                    if !imported.label.is_empty() && imported.label != "Claude" {
                        store.update_label(&account.id, &imported.label);
                    }
                    let creds = maybe_refresh(
                        store,
                        &account.id,
                        Provider::Claude,
                        imported.credentials.clone(),
                    )
                    .await;
                    if let Ok(quota) = fetch_claude_quota(client, &creds).await {
                        let email = if imported.label.contains('@') {
                            Some(imported.label.as_str())
                        } else {
                            None
                        };
                        return account_usage_from_claude(
                            account,
                            &quota,
                            WindowTotals::default(),
                            email,
                            None,
                        );
                    }
                }
            }
            let totals = aggregate_local_logs(Provider::Claude).unwrap_or_default();
            account_usage_from_logs(account, totals, "needs_login")
        }
    }
}

/// Builds the full per-account usage snapshot.
pub async fn poll_all(store: &AccountStore) -> Vec<AccountUsage> {
    let client = reqwest::Client::new();
    let accounts = store.list();
    let mut out = Vec::with_capacity(accounts.len());

    // Read CLI auth once per poll tick so the *matching* row can sync after
    // terminal login. Never use sole-row replace — siblings stay intact.
    let codex_cli = import::load_codex_cli_auth().ok();
    let claude_cli = import::load_claude_cli_auth().ok();

    for account in accounts {
        let usage = match account.provider {
            Provider::Agy => poll_agy(store, &client, &account).await,
            Provider::Codex => poll_codex(store, &client, &account, codex_cli.as_ref()).await,
            Provider::Claude => poll_claude(store, &client, &account, claude_cli.as_ref()).await,
            #[cfg(feature = "edition-pro")]
            Provider::Cursor => poll_cursor(store, &client, &account).await,
            #[cfg(feature = "edition-pro")]
            Provider::Grok => poll_grok(store, &client, &account).await,
            #[cfg(feature = "edition-pro")]
            Provider::Higgsfield => poll_higgsfield(store, &account).await,
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

    #[test]
    fn multi_codex_browser_rows_cli_sync_mutates_only_match() {
        // Simulates one poll tick: two browser-logged Codex rows, CLI auth for A.
        // Only A's credentials are eligible for replacement; B stays untouched.
        use usage_core::account::Credentials;

        let cli = ImportedAccount {
            label: "a@ex.com".into(),
            credentials: Credentials {
                access_token: "a-new".into(),
                refresh_token: Some("a-rt2".into()),
                account_id: Some("acct-a".into()),
                expires_at: None,
            },
        };
        let rows = [
            (
                Account {
                    id: "row-a".into(),
                    provider: Provider::Codex,
                    label: "a@ex.com".into(),
                },
                Credentials {
                    access_token: "a-old".into(),
                    refresh_token: Some("a-rt".into()),
                    account_id: Some("acct-a".into()),
                    expires_at: None,
                },
            ),
            (
                Account {
                    id: "row-b".into(),
                    provider: Provider::Codex,
                    label: "b@ex.com".into(),
                },
                Credentials {
                    access_token: "b-old".into(),
                    refresh_token: Some("b-rt".into()),
                    account_id: Some("acct-b".into()),
                    expires_at: None,
                },
            ),
        ];

        let decisions: Vec<(&str, bool)> = rows
            .iter()
            .map(|(account, stored)| {
                (
                    account.id.as_str(),
                    import::codex_cli_creds_are_newer(
                        stored,
                        &account.label,
                        &cli.credentials,
                        &cli.label,
                    ),
                )
            })
            .collect();

        assert_eq!(decisions, vec![("row-a", true), ("row-b", false)]);
        // Non-matching row's stored token is unchanged by the decision.
        assert_eq!(rows[1].1.access_token, "b-old");
        assert_eq!(rows[1].1.account_id.as_deref(), Some("acct-b"));
    }
}
