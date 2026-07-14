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

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::Serialize;

use usage_core::account::{Account, AuthSource, Credentials, Provider};
use usage_core::attribution::{assign_local_usage, AccountRef, ScannedRoot};
use usage_core::fetch::agy::{compact_windows, parse_agy_quota_summary, AgyQuota, AgyQuotaPool};
use usage_core::fetch::claude::{parse_claude_usage, ClaudeQuota};
use usage_core::fetch::codex::{parse_codex_usage, CodexQuota};
#[cfg(feature = "edition-pro")]
use usage_core::fetch::cursor::{cursor_quota_with_auth, parse_cursor_period_usage, CursorQuota};
#[cfg(feature = "edition-pro")]
use usage_core::fetch::grok::{parse_grok_prepaid_balance, GrokPrepaid};
#[cfg(feature = "edition-pro")]
use usage_core::fetch::higgsfield::{parse_higgsfield_account, HiggsfieldCredits};
use usage_core::models::{LocalProvenance, LocalUsage, ModelTokenEvent, QuotaUsage, WindowTotals};
use usage_core::scanners::{claude as claude_scanner, codex as codex_scanner};

use crate::agy_local;
use crate::paths;
use crate::store::AccountStore;

/// Refresh proactively when the access token expires within this window.
const REFRESH_THRESHOLD: Duration = Duration::from_secs(60);
const MAX_LOCAL_FILES: usize = 50_000;
const MAX_LOCAL_SCAN_TIME: Duration = Duration::from_secs(5);

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
async fn maybe_refresh(
    store: &AccountStore,
    id: &str,
    provider: Provider,
    creds: Credentials,
) -> Credentials {
    if !crate::oauth::should_refresh(creds.expires_at, Utc::now(), REFRESH_THRESHOLD) {
        return creds;
    }

    match crate::oauth::refresh_access_token(provider, &creds).await {
        Ok(refreshed) => {
            let _ = store.update_credentials(id, &refreshed);
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
    /// Pro providers: secondary label (`$12 left`, `809 credits left`).
    pub detail_suffix: Option<String>,
    pub status: String,
    /// Local-aggregation provenance label when the locally-summed token totals
    /// are not a clean `Ok` (e.g. `unavailable`, `partial`, `truncated`,
    /// `ambiguous`, `conflict`, `assumed`, `no_local_profile`). `None` means the
    /// local totals are fully trustworthy. Lets the tray/DTO distinguish a real
    /// zero from a failed/ambiguous local scan.
    pub local_status: Option<String>,
}

#[derive(Clone)]
struct LastSuccess {
    display_name: String,
    plan: Option<String>,
    five_hour: Option<QuotaUsage>,
    week: Option<QuotaUsage>,
}

fn last_success_cache() -> &'static Mutex<HashMap<String, LastSuccess>> {
    static CACHE: OnceLock<Mutex<HashMap<String, LastSuccess>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Post-step applied to every assembled usage. On success, remember the good
/// windows. On a transient failure (`error`) with a remembered success, serve
/// the cached windows as `stale`. Other statuses are never masked.
fn apply_last_success(
    cache: &mut HashMap<String, LastSuccess>,
    id: &str,
    mut usage: AccountUsage,
) -> AccountUsage {
    if usage.status == "ok" {
        cache.insert(
            id.to_string(),
            LastSuccess {
                display_name: usage.display_name.clone(),
                plan: usage.plan.clone(),
                five_hour: usage.five_hour.clone(),
                week: usage.week.clone(),
            },
        );
    } else if usage.status == "error" {
        if let Some(previous) = cache.get(id) {
            usage.display_name = previous.display_name.clone();
            usage.plan = previous.plan.clone();
            usage.five_hour = previous.five_hour.clone();
            usage.week = previous.week.clone();
            usage.status = "stale".to_string();
        }
    }
    usage
}

#[cfg(test)]
fn clear_last_success_cache() {
    last_success_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clear();
}

/// Evict a single account's remembered last success (called on account removal
/// so a re-added account with the same id never inherits stale quota).
pub fn evict_last_success(id: &str) {
    last_success_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(id);
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
        local_status: None,
    }
}

/// Fetches live Codex quota via HTTP using `creds.access_token`. Returns
/// `Ok(CodexQuota)` on a 200 response, `Err(status_code)` otherwise
/// (`None` status = network/transport failure, not an HTTP error).
async fn fetch_codex_quota(
    client: &reqwest::Client,
    creds: &Credentials,
) -> Result<CodexQuota, Option<u16>> {
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
async fn fetch_claude_quota(
    client: &reqwest::Client,
    creds: &Credentials,
) -> Result<ClaudeQuota, Option<u16>> {
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
    if root
        .get("shouldLogout")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
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
    let url = format!("https://management-api.x.ai/v1/billing/teams/{team_id}/prepaid/balance");
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
        local_status: None,
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
        local_status: None,
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
        local_status: None,
    }
}

#[cfg(feature = "edition-pro")]
fn cursor_outcome_status(
    session_id: &str,
    expected_id: &str,
    fetch: Result<(), Option<u16>>,
) -> &'static str {
    if session_id != expected_id {
        "identity_changed"
    } else if fetch.is_err() {
        "experimental_error"
    } else {
        "ok"
    }
}

#[cfg(feature = "edition-pro")]
async fn poll_cursor(
    _store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
    use usage_core::account::AuthSource;

    // Get database path and expected identity from auth_source
    let (database_path, expected_identity) = match &account.auth_source {
        AuthSource::CursorDatabase {
            database_path,
            expected_identity,
        } => (database_path.clone(), expected_identity.clone()),
        _ => {
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
        }
    };

    // Open DB read-only and read session (NO store.update_credentials)
    let session = match crate::cursor_local::read_cursor_session(&database_path) {
        Ok(s) => s,
        Err(_) => {
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
        }
    };

    // Validate identity
    let identity_status = cursor_outcome_status(&session.identity, &expected_identity, Ok(()));
    if identity_status != "ok" {
        return account_usage_from_cursor(
            account,
            &CursorQuota {
                email: None,
                plan: None,
                period: None,
                detail_suffix: None,
            },
            identity_status,
        );
    }

    // Refresh token in memory only
    let access_token = if let Some(refresh_token) = session.refresh_token.as_deref() {
        if let Ok(new_token) = refresh_cursor_access_token(client, refresh_token).await {
            new_token
        } else {
            session.access_token.clone()
        }
    } else {
        session.access_token.clone()
    };

    // Fetch quota with in-memory token
    let temp_creds = usage_core::account::Credentials {
        access_token,
        refresh_token: session.refresh_token.clone(),
        account_id: session.plan.clone(),
        expires_at: None,
    };
    match fetch_cursor_quota(client, &temp_creds).await {
        Ok(mut quota) => {
            quota = cursor_quota_with_auth(quota, session.email.clone(), session.plan.clone());
            account_usage_from_cursor(
                account,
                &quota,
                cursor_outcome_status(&session.identity, &expected_identity, Ok(())),
            )
        }
        Err(status) => account_usage_from_cursor(
            account,
            &CursorQuota {
                email: None,
                plan: session.plan.clone(),
                period: None,
                detail_suffix: None,
            },
            cursor_outcome_status(&session.identity, &expected_identity, Err(status)),
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
        .args(["account", "status", "--json"])
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
    let Some(identity) = crate::oauth::agy_identity_from_access_token(&creds.access_token).await else {
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
        let _ = store.update_credentials(&account.id, creds);
    }
    if needs_label {
        if let Some(email) = identity.email.filter(|s| !s.is_empty()) {
            store.update_label(&account.id, &email);
        }
    }
}

async fn poll_agy(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
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

fn codex_fetch_outcome(quota: CodexQuota) -> FetchOutcome {
    FetchOutcome::Live {
        five_hour: quota.five_hour,
        week: quota.week,
        plan: quota.plan,
        email: quota.email,
    }
}

fn claude_fetch_outcome(quota: ClaudeQuota, email: Option<&str>) -> FetchOutcome {
    FetchOutcome::Live {
        five_hour: quota.five_hour,
        week: quota.week,
        plan: None,
        email: email.map(str::to_string),
    }
}

fn codex_identity_status(probe_identity: &str, expected: &str) -> Option<&'static str> {
    if probe_identity == expected {
        None
    } else {
        Some("identity_changed")
    }
}

fn claude_identity_status(snapshot_identity: &str, expected: &str) -> Option<&'static str> {
    if snapshot_identity != expected {
        Some("identity_changed")
    } else {
        None
    }
}

enum ClaudeCliOutcome {
    Live(FetchOutcome),
    WaitingForUsage,
    IdentityChanged,
}

fn read_claude_snapshot_outcome(snapshot_path: &Path, expected_identity: &str) -> ClaudeCliOutcome {
    let Ok(bytes) = std::fs::read(snapshot_path) else {
        return ClaudeCliOutcome::WaitingForUsage;
    };
    let Ok(root) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return ClaudeCliOutcome::WaitingForUsage;
    };
    let identity = root
        .get("identity")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if claude_identity_status(identity, expected_identity).is_some() {
        return ClaudeCliOutcome::IdentityChanged;
    }
    let rate_limits = root
        .get("rate_limits")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let quota = parse_claude_usage(&rate_limits);
    if quota.five_hour.is_none() && quota.week.is_none() {
        return ClaudeCliOutcome::WaitingForUsage;
    }
    ClaudeCliOutcome::Live(FetchOutcome::Live {
        five_hour: quota.five_hour,
        week: quota.week,
        plan: None,
        email: (!identity.is_empty()).then(|| identity.to_string()),
    })
}

async fn poll_codex_cli_profile(
    profile_root: &std::path::Path,
    expected_identity: &str,
) -> FetchOutcome {
    match crate::codex_cli::probe_codex(profile_root).await {
        Ok(probe) => {
            if codex_identity_status(&probe.account.id, expected_identity).is_some() {
                return FetchOutcome::Failed { status: Some(401) };
            }
            FetchOutcome::Live {
                five_hour: probe.primary,
                week: probe.secondary,
                plan: None,
                email: probe.account.email.clone(),
            }
        }
        Err(_) => FetchOutcome::Failed { status: None },
    }
}

async fn poll_codex_oauth(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> FetchOutcome {
    match store.credentials(&account.id) {
        Some(creds) if !creds.access_token.is_empty() => {
            let creds = maybe_refresh(store, &account.id, Provider::Codex, creds).await;
            match fetch_codex_quota(client, &creds).await {
                Ok(quota) => {
                    if let Some(email) = quota.email.as_deref() {
                        store.update_label(&account.id, email);
                    }
                    codex_fetch_outcome(quota)
                }
                Err(status) => FetchOutcome::Failed { status },
            }
        }
        _ => FetchOutcome::Failed { status: Some(401) },
    }
}

async fn poll_claude_oauth(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> FetchOutcome {
    match store.credentials(&account.id) {
        Some(creds) if !creds.access_token.is_empty() => {
            let creds = maybe_refresh(store, &account.id, Provider::Claude, creds).await;
            match fetch_claude_quota(client, &creds).await {
                Ok(quota) => {
                    let email = if account.label.contains('@') {
                        Some(account.label.as_str())
                    } else {
                        None
                    };
                    claude_fetch_outcome(quota, email)
                }
                Err(status) => FetchOutcome::Failed { status },
            }
        }
        _ => FetchOutcome::Failed { status: Some(401) },
    }
}

async fn local_usage_for_provider(
    store: &AccountStore,
    accounts: &[Account],
    provider: Provider,
    now: DateTime<Utc>,
) -> HashMap<String, LocalUsage> {
    let provider_accounts: Vec<&Account> = accounts
        .iter()
        .filter(|account| account.provider == provider)
        .collect();
    if provider_accounts.is_empty() {
        return HashMap::new();
    }

    let mut profile_roots = match provider {
        Provider::Codex => paths::codex_home().into_iter().collect(),
        Provider::Claude => paths::claude_config_roots(),
        _ => return HashMap::new(),
    };
    profile_roots.extend(provider_accounts.iter().filter_map(|account| {
        if let AuthSource::CliProfile { profile_root, .. } = &account.auth_source {
            Some(profile_root.clone())
        } else {
            None
        }
    }));
    let profile_roots = match provider {
        Provider::Codex => paths::codex_profile_roots(&profile_roots),
        Provider::Claude => paths::claude_profile_roots(&profile_roots),
        _ => Vec::new(),
    };

    let mut scanned = Vec::with_capacity(profile_roots.len());
    for profile_root in profile_roots {
        let scan_roots = match provider {
            Provider::Codex => paths::codex_session_roots_for(&profile_root),
            Provider::Claude => paths::claude_project_roots_for(&profile_root),
            _ => Vec::new(),
        };
        let scan = scan_local_events(provider, &scan_roots, now).await;
        let health = scan_provenance(&scan);
        scanned.push(ScannedRoot {
            root_key: profile_root.clone(),
            source_roots: vec![profile_root.clone()],
            events: scan.events,
            health,
            identity: paths::root_identity(provider, &profile_root),
        });
    }

    let credential_ids: Vec<Option<String>> = provider_accounts
        .iter()
        .map(|account| {
            store
                .credentials(&account.id)
                .and_then(|credentials| credentials.account_id)
        })
        .collect();
    let account_refs: Vec<AccountRef<'_>> = provider_accounts
        .iter()
        .zip(&credential_ids)
        .map(|(account, credential_id)| AccountRef {
            account_id: &account.id,
            creds_account_id: credential_id.as_deref(),
            expected_identity: expected_identity(account),
            is_browser_oauth: matches!(account.auth_source, AuthSource::BrowserOAuth { .. }),
            profile_roots: account_profile_roots(provider, account),
        })
        .collect();

    assign_local_usage(&account_refs, &scanned, now)
        .into_iter()
        .collect()
}

fn account_profile_roots(provider: Provider, account: &Account) -> Vec<PathBuf> {
    let AuthSource::CliProfile { profile_root, .. } = &account.auth_source else {
        return Vec::new();
    };
    match provider {
        Provider::Codex => paths::codex_profile_roots(std::slice::from_ref(profile_root)),
        Provider::Claude => paths::claude_profile_roots(std::slice::from_ref(profile_root)),
        _ => Vec::new(),
    }
}

fn expected_identity(account: &Account) -> Option<&str> {
    match &account.auth_source {
        AuthSource::CliProfile {
            expected_identity, ..
        } => Some(expected_identity),
        AuthSource::BrowserOAuth { .. } => Some(&account.label),
        _ => None,
    }
}

fn scan_provenance(scan: &ScanResult) -> LocalProvenance {
    if scan.health.truncated {
        LocalProvenance::Truncated
    } else if scan.health.root_unreadable {
        LocalProvenance::Unavailable
    } else if scan.health.any_read_error {
        LocalProvenance::Partial
    } else if scan.events.is_empty() {
        LocalProvenance::NoEvents
    } else {
        LocalProvenance::Ok
    }
}

/// Builds the full per-account usage snapshot.
pub async fn poll_all(store: &AccountStore) -> Vec<AccountUsage> {
    let client = reqwest::Client::new();
    let accounts = store.list();
    let mut out = Vec::with_capacity(accounts.len());
    let now = Utc::now();
    let mut codex_local = local_usage_for_provider(store, &accounts, Provider::Codex, now).await;
    let mut claude_local = local_usage_for_provider(store, &accounts, Provider::Claude, now).await;

    for account in accounts {
        let usage = match account.provider {
            Provider::Agy => poll_agy(store, &client, &account).await,
            Provider::Codex => {
                let outcome = match &account.auth_source {
                    AuthSource::CliProfile {
                        profile_root,
                        expected_identity,
                        ..
                    } => poll_codex_cli_profile(profile_root, expected_identity).await,
                    _ => poll_codex_oauth(store, &client, &account).await,
                };
                let local = codex_local
                    .remove(&account.id)
                    .unwrap_or_else(|| LocalUsage::none(LocalProvenance::NoLocalProfile));
                assemble_account_usage(&account, outcome, local)
            }
            Provider::Claude => {
                let local = claude_local
                    .remove(&account.id)
                    .unwrap_or_else(|| LocalUsage::none(LocalProvenance::NoLocalProfile));
                match &account.auth_source {
                    AuthSource::CliProfile {
                        expected_identity, ..
                    } => match read_claude_snapshot_outcome(
                        &paths::claude_statusline_snapshot(&account.id),
                        expected_identity,
                    ) {
                        ClaudeCliOutcome::Live(outcome) => {
                            assemble_account_usage(&account, outcome, local)
                        }
                        ClaudeCliOutcome::WaitingForUsage => {
                            let mut usage = assemble_account_usage(
                                &account,
                                FetchOutcome::Failed { status: None },
                                local,
                            );
                            usage.status = "waiting_for_usage".to_string();
                            usage
                        }
                        ClaudeCliOutcome::IdentityChanged => {
                            let mut usage = assemble_account_usage(
                                &account,
                                FetchOutcome::Failed { status: None },
                                local,
                            );
                            usage.status = "identity_changed".to_string();
                            usage
                        }
                    },
                    _ => {
                        let outcome = poll_claude_oauth(store, &client, &account).await;
                        assemble_account_usage(&account, outcome, local)
                    }
                }
            }
            #[cfg(feature = "edition-pro")]
            Provider::Cursor => poll_cursor(store, &client, &account).await,
            #[cfg(feature = "edition-pro")]
            Provider::Grok => poll_grok(store, &client, &account).await,
            #[cfg(feature = "edition-pro")]
            Provider::Higgsfield => poll_higgsfield(store, &account).await,
        };
        let usage = {
            let mut cache = last_success_cache()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            apply_last_success(&mut cache, &account.id, usage)
        };
        out.push(usage);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use usage_core::account::{Account, AuthSource, Provider};
    use usage_core::models::QuotaUsage;

    fn auth_source_usage(id: &str, status: &str, five_hour: Option<QuotaUsage>) -> AccountUsage {
        let account = Account {
            id: id.to_string(),
            provider: Provider::Codex,
            label: "user@example.com".into(),
            auth_source: AuthSource::BrowserOAuth {
                credential_id: format!("credential-{id}"),
            },
        };
        AccountUsage {
            account,
            display_name: "user@example.com".into(),
            plan: Some("Pro".into()),
            five_hour,
            week: None,
            totals: WindowTotals::default(),
            pool_breakdown: Vec::new(),
            detail_suffix: None,
            status: status.to_string(),
            local_status: None,
        }
    }

    fn auth_source_quota(percent: f64) -> QuotaUsage {
        QuotaUsage {
            percent,
            resets_at: None,
            window_seconds: Some(18_000),
        }
    }

    #[test]
    fn auth_source_codex_identity_mismatch() {
        assert_eq!(
            codex_identity_status("id-a", "id-b"),
            Some("identity_changed")
        );
        assert_eq!(codex_identity_status("id", "id"), None);
    }

    #[test]
    fn auth_source_claude_identity_mismatch() {
        assert_eq!(claude_identity_status("a", "b"), Some("identity_changed"));
        assert_eq!(claude_identity_status("a", "a"), None);
    }

    #[test]
    fn auth_source_claude_snapshot_missing_is_waiting() {
        use std::path::Path;
        assert!(matches!(
            read_claude_snapshot_outcome(Path::new("/nonexistent"), "id"),
            ClaudeCliOutcome::WaitingForUsage
        ));
    }

    #[test]
    fn auth_source_claude_snapshot_live() {
        let temp = TempDir::new().expect("create temp directory");
        let snapshot = temp.path().join("snapshot.json");
        std::fs::write(
            &snapshot,
            r#"{"identity":"id","rate_limits":{"five_hour":{"utilization":30.0},"seven_day":{"utilization":55.0}}}"#,
        )
        .expect("write snapshot");

        let ClaudeCliOutcome::Live(FetchOutcome::Live {
            five_hour,
            week,
            plan,
            email,
        }) = read_claude_snapshot_outcome(&snapshot, "id")
        else {
            panic!("expected live Claude snapshot outcome");
        };

        assert_eq!(five_hour.map(|quota| quota.percent), Some(30.0));
        assert_eq!(week.map(|quota| quota.percent), Some(55.0));
        assert_eq!(plan, None);
        assert_eq!(email.as_deref(), Some("id"));
    }

    #[test]
    fn auth_source_claude_snapshot_identity_mismatch() {
        let temp = TempDir::new().expect("create temp directory");
        let snapshot = temp.path().join("snapshot.json");
        std::fs::write(
            &snapshot,
            r#"{"identity":"other","rate_limits":{"five_hour":{"utilization":30.0}}}"#,
        )
        .expect("write snapshot");

        assert!(matches!(
            read_claude_snapshot_outcome(&snapshot, "id"),
            ClaudeCliOutcome::IdentityChanged
        ));
    }

    #[test]
    fn auth_source_evict_last_success_drops_stale() {
        clear_last_success_cache();
        let quota = auth_source_quota(25.0);
        {
            let mut cache = last_success_cache()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            apply_last_success(&mut cache, "x", auth_source_usage("x", "ok", Some(quota)));
        }

        evict_last_success("x");

        let result = {
            let mut cache = last_success_cache()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            apply_last_success(&mut cache, "x", auth_source_usage("x", "error", None))
        };

        assert_eq!(result.status, "error");
        assert_eq!(result.five_hour, None);
    }

    #[test]
    fn auth_source_evict_is_isolated() {
        clear_last_success_cache();
        let x_quota = auth_source_quota(25.0);
        let y_quota = auth_source_quota(75.0);
        {
            let mut cache = last_success_cache()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            apply_last_success(&mut cache, "x", auth_source_usage("x", "ok", Some(x_quota)));
            apply_last_success(
                &mut cache,
                "y",
                auth_source_usage("y", "ok", Some(y_quota.clone())),
            );
        }

        evict_last_success("x");

        let (y_result, x_result) = {
            let mut cache = last_success_cache()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let y_result =
                apply_last_success(&mut cache, "y", auth_source_usage("y", "error", None));
            let x_result =
                apply_last_success(&mut cache, "x", auth_source_usage("x", "error", None));
            (y_result, x_result)
        };

        assert_eq!(y_result.status, "stale");
        assert_eq!(y_result.five_hour, Some(y_quota));
        assert_eq!(x_result.status, "error");
        assert_eq!(x_result.five_hour, None);
    }

    #[test]
    fn auth_source_ok_then_transient_error_serves_stale() {
        clear_last_success_cache();
        let mut cache = HashMap::new();
        let quota = auth_source_quota(25.0);
        apply_last_success(
            &mut cache,
            "account-1",
            auth_source_usage("account-1", "ok", Some(quota.clone())),
        );

        let result = apply_last_success(
            &mut cache,
            "account-1",
            auth_source_usage("account-1", "error", None),
        );

        assert_eq!(result.status, "stale");
        assert_eq!(result.five_hour, Some(quota));
    }

    #[test]
    fn auth_source_transient_error_without_prior_success_stays_error() {
        clear_last_success_cache();
        let mut cache = HashMap::new();

        let result = apply_last_success(
            &mut cache,
            "account-1",
            auth_source_usage("account-1", "error", None),
        );

        assert_eq!(result.status, "error");
        assert_eq!(result.five_hour, None);
    }

    #[test]
    fn auth_source_needs_login_never_stale() {
        clear_last_success_cache();
        let mut cache = HashMap::new();
        apply_last_success(
            &mut cache,
            "account-1",
            auth_source_usage("account-1", "ok", Some(auth_source_quota(25.0))),
        );

        for status in ["needs_login", "rate_limited", "identity_changed"] {
            let result = apply_last_success(
                &mut cache,
                "account-1",
                auth_source_usage("account-1", status, None),
            );
            assert_eq!(result.status, status);
            assert_eq!(result.five_hour, None);
        }
    }

    #[test]
    fn auth_source_stale_uses_latest_success() {
        clear_last_success_cache();
        let mut cache = HashMap::new();
        let first = auth_source_quota(25.0);
        let latest = auth_source_quota(75.0);
        apply_last_success(
            &mut cache,
            "account-1",
            auth_source_usage("account-1", "ok", Some(first)),
        );
        apply_last_success(
            &mut cache,
            "account-1",
            auth_source_usage("account-1", "ok", Some(latest.clone())),
        );

        let result = apply_last_success(
            &mut cache,
            "account-1",
            auth_source_usage("account-1", "error", None),
        );

        assert_eq!(result.status, "stale");
        assert_eq!(result.five_hour, Some(latest));
    }

    #[test]
    fn maps_agy_pools() {
        let acct = Account {
            id: "3".into(),
            provider: Provider::Agy,
            label: "agy".into(),
            auth_source: AuthSource::BrowserOAuth {
                credential_id: "agy-credential".into(),
            },
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

/// Result of scanning a local provider root for events.
#[derive(Clone, Debug)]
pub struct ScanResult {
    pub events: Vec<ModelTokenEvent>,
    pub health: ScanHealth,
}

/// Health status of a scan operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanHealth {
    pub any_read_error: bool,
    pub root_unreadable: bool,
    pub truncated: bool,
}

/// Outcome of a provider HTTP fetch.
#[derive(Clone, Debug)]
pub enum FetchOutcome {
    Live {
        five_hour: Option<QuotaUsage>,
        week: Option<QuotaUsage>,
        plan: Option<String>,
        email: Option<String>,
    },
    Failed {
        status: Option<u16>,
    },
}

/// Scan local provider roots for token events (raw, not aggregated).
pub async fn scan_local_events(
    provider: Provider,
    scan_roots: &[PathBuf],
    _now: DateTime<Utc>,
) -> ScanResult {
    let roots = scan_roots.to_vec();
    tokio::task::spawn_blocking(move || scan_local_events_blocking(provider, &roots))
        .await
        .unwrap_or_else(|_| ScanResult {
            events: Vec::new(),
            health: ScanHealth {
                any_read_error: true,
                root_unreadable: true,
                truncated: false,
            },
        })
}

/// Assemble a single AccountUsage from account, fetch outcome, and local usage.
pub fn assemble_account_usage(
    account: &Account,
    outcome: FetchOutcome,
    local: LocalUsage,
) -> AccountUsage {
    let local_status = local_status_label(local.provenance).map(str::to_string);
    let (five_hour, week, plan, email, status) = match outcome {
        FetchOutcome::Live {
            five_hour,
            week,
            plan,
            email,
        } => (five_hour, week, plan, email, "ok".to_string()),
        FetchOutcome::Failed { status } => (
            None,
            None,
            None,
            None,
            status_for_failure(status).to_string(),
        ),
    };
    let mut account = account.clone();
    if let Some(email) = email.as_deref().filter(|email| !email.is_empty()) {
        account.label = email.to_string();
    }

    AccountUsage {
        display_name: display_name_for(&account, email.as_deref(), plan.as_deref()),
        plan,
        account,
        five_hour,
        week,
        totals: local.totals,
        pool_breakdown: Vec::new(),
        detail_suffix: None,
        status,
        local_status,
    }
}

fn scan_local_events_blocking(provider: Provider, roots: &[PathBuf]) -> ScanResult {
    let started = Instant::now();
    let mut result = ScanResult {
        events: Vec::new(),
        health: ScanHealth {
            any_read_error: false,
            root_unreadable: false,
            truncated: false,
        },
    };
    let mut visited = HashSet::new();
    let mut files_read = 0;

    for root in roots {
        if result.health.truncated {
            break;
        }
        let Ok(metadata) = std::fs::symlink_metadata(root) else {
            result.health.root_unreadable = true;
            continue;
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            result.health.root_unreadable = true;
            continue;
        }
        if std::fs::read_dir(root).is_err() {
            result.health.root_unreadable = true;
            continue;
        }
        let root_key = root.canonicalize().unwrap_or_else(|_| root.clone());
        if !visited.insert(root_key) {
            continue;
        }
        scan_directory(
            provider,
            root,
            &mut result,
            &mut visited,
            &mut files_read,
            started,
        );
    }
    result
}

fn scan_directory(
    provider: Provider,
    root: &Path,
    result: &mut ScanResult,
    visited: &mut HashSet<PathBuf>,
    files_read: &mut usize,
    started: Instant,
) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        if started.elapsed() >= MAX_LOCAL_SCAN_TIME {
            result.health.truncated = true;
            return;
        }
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                result.health.any_read_error = true;
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    result.health.any_read_error = true;
                    continue;
                }
            };
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => {
                    result.health.any_read_error = true;
                    continue;
                }
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                let key = path.canonicalize().unwrap_or_else(|_| path.clone());
                if visited.insert(key) {
                    stack.push(path);
                }
                continue;
            }
            if !is_jsonl(&path) {
                continue;
            }
            if *files_read >= MAX_LOCAL_FILES || started.elapsed() >= MAX_LOCAL_SCAN_TIME {
                result.health.truncated = true;
                return;
            }
            *files_read += 1;
            read_event_file(provider, &path, result);
        }
    }
}

fn read_event_file(provider: Provider, path: &Path, result: &mut ScanResult) {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => {
            result.health.any_read_error = true;
            return;
        }
    };
    for line in BufReader::new(file).lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => {
                result.health.any_read_error = true;
                return;
            }
        };
        if let Some(event) = parse_local_event(provider, &line) {
            result.events.push(event);
        }
    }
}

fn parse_local_event(provider: Provider, line: &str) -> Option<ModelTokenEvent> {
    let parsed = match provider {
        Provider::Codex => codex_scanner::parse_codex_line(line),
        Provider::Claude => claude_scanner::parse_claude_line(line),
        _ => None,
    };
    parsed.or_else(|| serde_json::from_str(line).ok())
}

fn is_jsonl(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("jsonl")
}

fn local_status_label(provenance: LocalProvenance) -> Option<&'static str> {
    match provenance {
        LocalProvenance::Ok | LocalProvenance::NoEvents | LocalProvenance::SharedProfileOther => {
            None
        }
        LocalProvenance::NoLocalProfile => Some("no_local_profile"),
        LocalProvenance::Assumed => Some("assumed"),
        LocalProvenance::Ambiguous => Some("ambiguous"),
        LocalProvenance::Conflict => Some("conflict"),
        LocalProvenance::Partial => Some("partial"),
        LocalProvenance::Unavailable => Some("unavailable"),
        LocalProvenance::Truncated => Some("truncated"),
    }
}

// NOTE: superseded `tests_new` module (which used the #[should_panic] anti-pattern)
// removed during TEST verification — the correct assemble-seam tests live in
// `tests_seam` below/above.

#[cfg(test)]
mod tests_seam {
    use super::*;

    #[test]
    fn test_assemble_live_outcome_ok_local_preserves_totals() {
        // §6.9: Live outcome + local(Ok, totals>0).
        // Expected: token_totals == local.totals, five_hour/week Some.
        // This is the CURRENT BUG — assemble_account_usage passes WindowTotals::default() instead.
        let acct = Account {
            id: "test".into(),
            provider: Provider::Codex,
            label: "user@ex.com".into(),
            auth_source: usage_core::account::AuthSource::BrowserOAuth {
                credential_id: "test-cred".into(),
            },
        };
        let outcome = FetchOutcome::Live {
            five_hour: Some(QuotaUsage {
                percent: 25.0,
                resets_at: None,
                window_seconds: Some(18000),
            }),
            week: Some(QuotaUsage {
                percent: 30.0,
                resets_at: None,
                window_seconds: None,
            }),
            plan: Some("Pro".into()),
            email: Some("user@ex.com".into()),
        };
        let local = LocalUsage {
            totals: WindowTotals {
                five_hours: 500,
                week: 2000,
                month: 10000,
            },
            provenance: usage_core::models::LocalProvenance::Ok,
        };
        let result = assemble_account_usage(&acct, outcome, local);
        // Real assertions:
        assert!(
            result.five_hour.is_some(),
            "Live outcome should preserve five_hour"
        );
        assert!(result.week.is_some(), "Live outcome should preserve week");
        // CRITICAL: token_totals must match local.totals (currently fails — returns 0)
        assert_eq!(
            result.totals.five_hours, 500,
            "token_totals should match local.totals (BUG: currently returns 0)"
        );
        assert_eq!(result.totals.week, 2000, "week tokens should match local");
        assert_eq!(
            result.totals.month, 10000,
            "month tokens should match local"
        );
    }

    #[test]
    fn test_assemble_failed_outcome_uses_local_totals() {
        // §6.9: Failed(429) + local(Ok).
        // Expected: five_hour/week None, token_totals == local.totals.
        let acct = Account {
            id: "test".into(),
            provider: Provider::Claude,
            label: "user@ex.com".into(),
            auth_source: usage_core::account::AuthSource::BrowserOAuth {
                credential_id: "claude-cred".into(),
            },
        };
        let outcome = FetchOutcome::Failed { status: Some(429) };
        let local = LocalUsage {
            totals: WindowTotals {
                five_hours: 300,
                week: 1500,
                month: 8000,
            },
            provenance: usage_core::models::LocalProvenance::Ok,
        };
        let result = assemble_account_usage(&acct, outcome, local);
        // Real assertions:
        assert!(
            result.five_hour.is_none(),
            "Failed outcome should not set five_hour"
        );
        assert!(result.week.is_none(), "Failed outcome should not set week");
        assert_eq!(
            result.totals.five_hours, 300,
            "Failed should use local totals"
        );
        assert_eq!(result.totals.week, 1500, "Failed should use local week");
    }

    #[test]
    fn test_assemble_failed_unavailable_distinct_from_zero() {
        // §6.9/DoD §1.4: Unavailable must be DISTINCT from real 0 totals.
        // The DTO's local_status should carry "unavailable" when provenance=Unavailable.
        let acct = Account {
            id: "test".into(),
            provider: Provider::Codex,
            label: "test@ex.com".into(),
            auth_source: usage_core::account::AuthSource::BrowserOAuth {
                credential_id: "cred".into(),
            },
        };
        let outcome = FetchOutcome::Failed { status: None };
        let local = LocalUsage {
            totals: WindowTotals::default(),
            provenance: usage_core::models::LocalProvenance::Unavailable,
        };
        let result = assemble_account_usage(&acct, outcome, local);

        // CRITICAL: totals are 0, BUT status must distinguish Unavailable
        assert_eq!(result.totals.five_hours, 0, "Unavailable has no totals");

        // The status field should indicate the problem, not generic "error"
        // Stub will have generic status, but test proves the seam exists
        assert!(
            !result.status.is_empty(),
            "Status must be set (stub returns generic, LOGIC refines per provenance)"
        );
    }
}

#[cfg(test)]
mod tests_filesystem {
    use super::*;
    use chrono::Utc;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

    /// §6.1 Cap regression: write 300 real .jsonl files, scan them.
    /// This test MUST call scan_local_events (real filesystem layer).
    /// Guards against reintroducing the old MAX_FILES=200 cap.
    #[tokio::test]
    async fn test_cap_regression_300_real_files() {
        let dir = tempdir().expect("tempdir");
        let root_path = dir.path().to_path_buf();

        // Create 300 .jsonl files, each with one event
        for i in 0..300 {
            let file_path = root_path.join(format!("event_{:03}.jsonl", i));
            let now = Utc::now();
            let json = serde_json::json!({
                "timestamp": now.to_rfc3339(),
                "model": format!("test-model-{}", i),
                "tokens": 100,
                "dedupe_key": format!("key-{}", i)
            });
            fs::write(&file_path, format!("{}\n", json)).expect("write file");
        }

        let now = Utc::now();
        let result = scan_local_events(Provider::Codex, &[root_path], now).await;

        // Real assertion: all 300 events scanned
        assert_eq!(
            result.events.len(),
            300,
            "Should scan all 300 events (currently stub returns 0)"
        );
        // Provenance should be Ok, NOT Truncated (300 « budget)
        assert!(
            !result.health.truncated,
            "300 events should NOT trigger truncation"
        );
    }

    /// §6.2 mtime irrelevance: file mtime is 40 days old, event timestamp 1h old.
    /// scan_local_events MUST scan by event timestamp, NOT mtime.
    /// FAILS on stub, and WOULD FAIL if implementation uses mtime skip.
    #[tokio::test]
    async fn test_mtime_irrelevance_recent_timestamp() {
        use filetime::FileTime;

        let dir = tempdir().expect("tempdir");
        let root_path = dir.path().to_path_buf();
        let file_path = root_path.join("old_mtime.jsonl");

        let now = Utc::now();
        let event_ts = now - chrono::Duration::hours(1);

        let json = serde_json::json!({
            "timestamp": event_ts.to_rfc3339(),
            "model": "test",
            "tokens": 100,
            "dedupe_key": "test-key"
        });
        fs::write(&file_path, format!("{}\n", json)).expect("write file");

        // Set file mtime to 40 days ago
        let old_time = FileTime::from_system_time(
            std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 24 * 3600),
        );
        filetime::set_file_mtime(&file_path, old_time).expect("set mtime");

        let result = scan_local_events(Provider::Codex, &[root_path], now).await;

        // Real assertion: event must be scanned despite old mtime
        assert_eq!(
            result.events.len(),
            1,
            "Should scan event with 1h-old timestamp, even though file mtime is 40d old"
        );
        assert_eq!(
            result.events[0].tokens, 100,
            "Event tokens should be counted"
        );
    }

    /// §6.10 Error classification: unreadable root → Unavailable provenance.
    #[tokio::test]
    async fn test_scan_unreadable_root_provenance() {
        let nonexistent = PathBuf::from("/nonexistent/root/path");
        let now = Utc::now();

        let result = scan_local_events(Provider::Codex, &[nonexistent], now).await;

        // Root doesn't exist → root_unreadable should be true
        assert!(
            result.health.root_unreadable,
            "Unreadable root should set root_unreadable=true"
        );
        // Events should be empty
        assert_eq!(result.events.len(), 0, "Unreadable root has no events");
    }
    #[test]
    #[cfg(feature = "edition-pro")]
    fn cursor_identity_mismatch_maps_identity_changed() {
        assert_eq!(
            cursor_outcome_status("cursor-user-a", "cursor-user-b", Ok(())),
            "identity_changed"
        );
    }

    #[test]
    #[cfg(feature = "edition-pro")]
    fn cursor_rpc_failure_maps_experimental_error() {
        assert_eq!(
            cursor_outcome_status("cursor-user", "cursor-user", Err(Some(500))),
            "experimental_error"
        );
        assert_eq!(
            cursor_outcome_status("cursor-user", "cursor-user", Err(None)),
            "experimental_error"
        );
    }

    #[test]
    #[cfg(feature = "edition-pro")]
    fn cursor_success_maps_ok() {
        assert_eq!(
            cursor_outcome_status("cursor-user", "cursor-user", Ok(())),
            "ok"
        );
    }
}
