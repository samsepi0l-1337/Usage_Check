//! HTTP quota fetchers for the per-account usage poller.

use usage_core::account::Credentials;
use usage_core::fetch::agy::{parse_agy_quota_summary, AgyQuota};
use usage_core::fetch::claude::{parse_claude_usage, ClaudeQuota};
use usage_core::fetch::codex::{parse_codex_usage, CodexQuota};
#[cfg(feature = "edition-pro")]
use usage_core::fetch::cursor::{parse_cursor_period_usage, CursorQuota};
#[cfg(feature = "edition-pro")]
use usage_core::fetch::grok::{parse_grok_prepaid_balance, GrokPrepaid};

const AGY_USER_AGENT: &str = "antigravity/usagecheck macos/arm64";
const AGY_QUOTA_SUMMARY_URLS: &[&str] = &[
    "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary",
    "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary",
];
const AGY_LOAD_CODE_ASSIST_URLS: &[&str] = &[
    "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
    "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
];

/// Fetches live Codex quota via HTTP using `creds.access_token`. Returns
/// `Ok(CodexQuota)` on a 200 response, `Err(status_code)` otherwise
/// (`None` status = network/transport failure, not an HTTP error).
pub(super) async fn fetch_codex_quota(
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
pub(super) async fn fetch_claude_quota(
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
pub(super) async fn fetch_agy_quota_remote(
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
pub(super) async fn refresh_cursor_access_token(
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
pub(super) async fn fetch_cursor_quota(
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
pub(super) async fn fetch_grok_prepaid(
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
pub(super) fn fetch_higgsfield_account_json() -> Result<serde_json::Value, ()> {
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
