//! HTTP quota fetchers for the per-account usage poller.

use usage_core::account::Credentials;
use usage_core::fetch::agy::{parse_agy_quota_summary, AgyQuota};
use usage_core::fetch::amp::{parse_amp_balance, AmpBalance};
use usage_core::fetch::claude::{parse_claude_usage, ClaudeQuota};
use usage_core::fetch::codex::{parse_codex_usage, CodexQuota};
use usage_core::fetch::copilot::{parse_copilot_user, CopilotQuota};
use usage_core::fetch::cursor::{parse_cursor_period_usage, CursorQuota};
use usage_core::fetch::deepseek::{parse_deepseek_balance, DeepSeekBalance};
use usage_core::fetch::factory::{parse_factory_usage, FactoryUsage};
use usage_core::fetch::fireworks::{parse_fireworks_billing, FireworksBilling};
use usage_core::fetch::grok::{parse_grok_prepaid_balance, GrokPrepaid};
use usage_core::fetch::kimi::{parse_kimi_usages, KimiUsage};
use usage_core::fetch::kiro::{parse_kiro_usage_limits, KiroQuota};
use usage_core::fetch::novita::{parse_novita_balance, NovitaBalance};
use usage_core::fetch::opencode::{parse_opencode_usage, OpenCodeUsage};
use usage_core::fetch::openrouter::{parse_openrouter_key, OpenRouterUsage};
use usage_core::fetch::poe::{parse_poe_balance, PoeBalance};
use usage_core::fetch::trae::{parse_trae_entitlements, TraeQuota};
use usage_core::fetch::windsurf::{parse_windsurf_user_status, WindsurfQuota};
use usage_core::fetch::zai::{parse_zai_quota, ZaiQuota};

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

const CURSOR_API_BASE: &str = "https://api2.cursor.sh";
const CURSOR_OAUTH_CLIENT_ID: &str = "KbZUR41cY7W6zRSdpSUJ7I7mLYBKOCmB";

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

const USAGECHECK_UA: &str = "UsageCheck";

async fn bearer_json(
    client: &reqwest::Client,
    url: &str,
    token: &str,
) -> Result<serde_json::Value, Option<u16>> {
    let resp = client
        .get(url)
        .header("Accept", "application/json")
        .header("User-Agent", USAGECHECK_UA)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| None)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Some(status.as_u16()));
    }
    resp.json().await.map_err(|_| Some(status.as_u16()))
}

const KIMI_USAGE_URLS: &[&str] = &[
    "https://api.kimi.com/coding/v1/usages",
    "https://api.kimi.ai/coding/v1/usages",
];

pub(super) async fn fetch_kimi_usages(
    client: &reqwest::Client,
    creds: &Credentials,
) -> Result<KimiUsage, Option<u16>> {
    let mut last_status: Option<u16> = None;
    for url in KIMI_USAGE_URLS {
        match bearer_json(client, url, &creds.access_token).await {
            Ok(body) => return Ok(parse_kimi_usages(&body)),
            Err(Some(404)) => {
                last_status = Some(404);
                continue;
            }
            Err(status) => return Err(status),
        }
    }
    Err(last_status)
}

pub(super) async fn fetch_opencode_usage(
    client: &reqwest::Client,
    creds: &Credentials,
) -> Result<OpenCodeUsage, Option<u16>> {
    let body = bearer_json(
        client,
        "https://opencode.ai/zen/go/v1/usage",
        &creds.access_token,
    )
    .await?;
    Ok(parse_opencode_usage(&body))
}

pub(super) async fn fetch_deepseek_balance(
    client: &reqwest::Client,
    creds: &Credentials,
) -> Result<DeepSeekBalance, Option<u16>> {
    let body = bearer_json(
        client,
        "https://api.deepseek.com/user/balance",
        &creds.access_token,
    )
    .await?;
    Ok(parse_deepseek_balance(&body))
}

pub(super) async fn fetch_openrouter_key(
    client: &reqwest::Client,
    creds: &Credentials,
) -> Result<OpenRouterUsage, Option<u16>> {
    let body = bearer_json(
        client,
        "https://openrouter.ai/api/v1/key",
        &creds.access_token,
    )
    .await?;
    Ok(parse_openrouter_key(&body))
}

pub(super) async fn fetch_poe_balance(
    client: &reqwest::Client,
    creds: &Credentials,
) -> Result<PoeBalance, Option<u16>> {
    let body = bearer_json(
        client,
        "https://api.poe.com/usage/current_balance",
        &creds.access_token,
    )
    .await?;
    Ok(parse_poe_balance(&body))
}

fn fireworks_month_bounds(now: chrono::DateTime<chrono::Utc>) -> (String, String) {
    use chrono::{Datelike, NaiveDate, SecondsFormat};

    let start_date =
        NaiveDate::from_ymd_opt(now.year(), now.month(), 1).unwrap_or_else(|| now.date_naive());
    let end_date = if now.month() == 12 {
        NaiveDate::from_ymd_opt(now.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(now.year(), now.month() + 1, 1)
    }
    .unwrap_or(start_date);
    let start = start_date
        .and_hms_opt(0, 0, 0)
        .unwrap_or_default()
        .and_utc()
        .to_rfc3339_opts(SecondsFormat::Secs, true);
    let end = end_date
        .and_hms_opt(0, 0, 0)
        .unwrap_or_default()
        .and_utc()
        .to_rfc3339_opts(SecondsFormat::Secs, true);
    (start, end)
}

pub(super) async fn fetch_fireworks_billing(
    client: &reqwest::Client,
    creds: &Credentials,
) -> Result<FireworksBilling, Option<u16>> {
    let account_id = creds
        .account_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(Some(404_u16))?;
    let (start, end) = fireworks_month_bounds(chrono::Utc::now());
    let url = format!(
        "https://api.fireworks.ai/v1/accounts/{}/billing/summary?startTime={}&endTime={}",
        urlencoding::encode(account_id),
        urlencoding::encode(&start),
        urlencoding::encode(&end),
    );
    let body = bearer_json(client, &url, &creds.access_token).await?;
    Ok(parse_fireworks_billing(&body))
}

pub(super) async fn fetch_novita_balance(
    client: &reqwest::Client,
    creds: &Credentials,
) -> Result<NovitaBalance, Option<u16>> {
    let body = bearer_json(
        client,
        "https://api.novita.ai/openapi/v1/billing/balance/detail",
        &creds.access_token,
    )
    .await?;
    Ok(parse_novita_balance(&body))
}

const AMP_RPC_URL: &str = "https://ampcode.com/api/internal";

pub(super) async fn fetch_amp_balance(
    client: &reqwest::Client,
    creds: &Credentials,
) -> Result<AmpBalance, Option<u16>> {
    let resp = client
        .post(AMP_RPC_URL)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("User-Agent", USAGECHECK_UA)
        .bearer_auth(&creds.access_token)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "userDisplayBalanceInfo",
            "params": {}
        }))
        .send()
        .await
        .map_err(|_| None)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Some(status.as_u16()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|_| Some(status.as_u16()))?;
    Ok(parse_amp_balance(&body))
}

const ZAI_QUOTA_URL: &str = "https://api.z.ai/api/monitor/usage/quota/limit";

async fn zai_quota_request(
    client: &reqwest::Client,
    authorization: &str,
) -> Result<serde_json::Value, Option<u16>> {
    let resp = client
        .get(ZAI_QUOTA_URL)
        .header("Accept", "application/json")
        .header("Accept-Language", "en-US,en")
        .header("Content-Type", "application/json")
        .header("Authorization", authorization)
        .header("User-Agent", USAGECHECK_UA)
        .send()
        .await
        .map_err(|_| None)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Some(status.as_u16()));
    }
    resp.json().await.map_err(|_| Some(status.as_u16()))
}

pub(super) async fn fetch_zai_quota(
    client: &reqwest::Client,
    creds: &Credentials,
) -> Result<ZaiQuota, Option<u16>> {
    // Dashboard XHR uses raw `Authorization: <key>` (no Bearer). Retry Bearer
    // only after a 401 so a mis-prefixed key is not the first attempt.
    match zai_quota_request(client, &creds.access_token).await {
        Ok(body) => Ok(parse_zai_quota(&body)),
        Err(Some(401)) => {
            let body = zai_quota_request(client, &format!("Bearer {}", creds.access_token)).await?;
            Ok(parse_zai_quota(&body))
        }
        Err(status) => Err(status),
    }
}

const COPILOT_USER_URL: &str = "https://api.github.com/copilot_internal/user";
const COPILOT_EDITOR_VERSION: &str = "vscode/1.98.1";
const COPILOT_PLUGIN_VERSION: &str = "copilot-chat/0.26.7";

fn copilot_request<'a>(
    client: &'a reqwest::Client,
    authorization: &str,
) -> reqwest::RequestBuilder {
    client
        .get(COPILOT_USER_URL)
        .header("Accept", "application/json")
        .header("Authorization", authorization)
        .header("Editor-Version", COPILOT_EDITOR_VERSION)
        .header("Editor-Plugin-Version", COPILOT_PLUGIN_VERSION)
        .header("User-Agent", USAGECHECK_UA)
}

pub(super) async fn fetch_copilot_user(
    client: &reqwest::Client,
    creds: &Credentials,
) -> Result<CopilotQuota, Option<u16>> {
    let token_auth = format!("token {}", creds.access_token);
    let resp = copilot_request(client, &token_auth)
        .send()
        .await
        .map_err(|_| None)?;
    let status = resp.status();
    let resp = if status.as_u16() == 401 {
        copilot_request(client, &format!("Bearer {}", creds.access_token))
            .send()
            .await
            .map_err(|_| None)?
    } else {
        resp
    };
    let status = resp.status();
    if !status.is_success() {
        return Err(Some(status.as_u16()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|_| Some(status.as_u16()))?;
    Ok(parse_copilot_user(&body))
}

const WINDSURF_STATUS_URLS: &[&str] = &[
    "https://server.self-serve.windsurf.com/exa.seat_management_pb.SeatManagementService/GetUserStatus",
    "https://server.codeium.com/exa.seat_management_pb.SeatManagementService/GetUserStatus",
];

/// Continue to the Codeium host only on 404, 5xx, or transport failure.
/// 401/403 (and other client errors) must not be overwritten by the fallback.
fn windsurf_should_try_fallback(status: Option<u16>) -> bool {
    match status {
        None => true,
        Some(404) => true,
        Some(code) if (500..600).contains(&code) => true,
        _ => false,
    }
}

pub(super) async fn fetch_windsurf_status(
    client: &reqwest::Client,
    api_key: &str,
) -> Result<WindsurfQuota, Option<u16>> {
    let body = serde_json::json!({
        "metadata": {
            "apiKey": api_key,
            "ideName": "windsurf"
        }
    });
    let mut last_status: Option<u16> = None;
    for url in WINDSURF_STATUS_URLS {
        let resp = match client
            .post(*url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("Connect-Protocol-Version", "1")
            .header("User-Agent", USAGECHECK_UA)
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(_) => continue,
        };
        let status = resp.status();
        if !status.is_success() {
            let code = status.as_u16();
            if !windsurf_should_try_fallback(Some(code)) {
                return Err(Some(code));
            }
            last_status = Some(code);
            continue;
        }
        let root: serde_json::Value = resp.json().await.map_err(|_| Some(status.as_u16()))?;
        return Ok(parse_windsurf_user_status(&root));
    }
    Err(last_status)
}

const TRAE_ENTITLEMENT_URLS: &[&str] = &[
    "https://api-sg-central.trae.ai/trae/api/v1/pay/user_current_entitlement_list",
    "https://api-us-east.trae.ai/trae/api/v1/pay/user_current_entitlement_list",
];

fn trae_should_try_fallback(status: Option<u16>) -> bool {
    match status {
        None => true,
        Some(404) => true,
        Some(code) if (500..600).contains(&code) => true,
        _ => false,
    }
}

pub(super) async fn fetch_trae_entitlements(
    client: &reqwest::Client,
    jwt: &str,
) -> Result<TraeQuota, Option<u16>> {
    let mut last_status: Option<u16> = None;
    let token = jwt.strip_prefix("Cloud-IDE-JWT ").unwrap_or(jwt).trim();
    for url in TRAE_ENTITLEMENT_URLS {
        let resp = match client
            .post(*url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Cloud-IDE-JWT {token}"))
            .header("User-Agent", USAGECHECK_UA)
            .json(&serde_json::json!({ "require_usage": true }))
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(_) => continue,
        };
        let status = resp.status();
        if !status.is_success() {
            let code = status.as_u16();
            if !trae_should_try_fallback(Some(code)) {
                return Err(Some(code));
            }
            last_status = Some(code);
            continue;
        }
        let root: serde_json::Value = resp.json().await.map_err(|_| Some(status.as_u16()))?;
        return Ok(parse_trae_entitlements(&root));
    }
    Err(last_status)
}

pub(super) async fn refresh_kiro_access_token(
    client: &reqwest::Client,
    refresh_token: &str,
    region: &str,
) -> Result<(String, Option<String>, Option<String>), Option<u16>> {
    let Some((url, _)) = crate::import::kiro_endpoints(region) else {
        return Err(Some(404));
    };
    let resp = client
        .post(&url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("User-Agent", USAGECHECK_UA)
        .json(&serde_json::json!({ "refreshToken": refresh_token }))
        .send()
        .await
        .map_err(|_| None)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Some(status.as_u16()));
    }
    let root: serde_json::Value = resp.json().await.map_err(|_| Some(status.as_u16()))?;
    let access = root
        .get("accessToken")
        .or_else(|| root.get("access_token"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(Some(status.as_u16()))?;
    let refresh = root
        .get("refreshToken")
        .or_else(|| root.get("refresh_token"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let profile_arn = root
        .get("profileArn")
        .or_else(|| root.get("profile_arn"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|arn| crate::import::region_from_profile_arn(arn).is_some())
        .map(str::to_string);
    Ok((access.to_string(), refresh, profile_arn))
}

pub(super) async fn fetch_kiro_usage_limits(
    client: &reqwest::Client,
    access_token: &str,
    region: &str,
    profile_arn: &str,
) -> Result<KiroQuota, Option<u16>> {
    let Some((_, url)) = crate::import::kiro_endpoints(region) else {
        return Err(Some(404));
    };
    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .header("User-Agent", USAGECHECK_UA)
        .bearer_auth(access_token)
        .query(&[
            ("origin", "AI_EDITOR"),
            ("profileArn", profile_arn),
            ("resourceType", "AGENTIC_REQUEST"),
        ])
        .send()
        .await
        .map_err(|_| None)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Some(status.as_u16()));
    }
    let root: serde_json::Value = resp.json().await.map_err(|_| Some(status.as_u16()))?;
    Ok(parse_kiro_usage_limits(&root))
}

const FACTORY_USAGE_URL: &str = "https://api.factory.ai/api/organization/subscription/usage";
const FACTORY_REFRESH_URL: &str = "https://api.workos.com/user_management/authenticate";
const FACTORY_CLIENT_ID: &str = "client_01HNM792M5G5G1A2THWPXKFMXB";

pub(super) async fn refresh_factory_access_token(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<(String, Option<String>), Option<u16>> {
    let resp = client
        .post(FACTORY_REFRESH_URL)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("User-Agent", USAGECHECK_UA)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", FACTORY_CLIENT_ID),
        ])
        .send()
        .await
        .map_err(|_| None)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Some(status.as_u16()));
    }
    let root: serde_json::Value = resp.json().await.map_err(|_| Some(status.as_u16()))?;
    let access = root
        .get("access_token")
        .or_else(|| root.get("accessToken"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(Some(status.as_u16()))?;
    let refresh = root
        .get("refresh_token")
        .or_else(|| root.get("refreshToken"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok((access.to_string(), refresh))
}

pub(super) async fn fetch_factory_usage(
    client: &reqwest::Client,
    creds: &Credentials,
) -> Result<FactoryUsage, Option<u16>> {
    let resp = client
        .post(FACTORY_USAGE_URL)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("User-Agent", USAGECHECK_UA)
        .bearer_auth(&creds.access_token)
        .json(&serde_json::json!({ "useCache": true }))
        .send()
        .await
        .map_err(|_| None)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Some(status.as_u16()));
    }
    let root: serde_json::Value = resp.json().await.map_err(|_| Some(status.as_u16()))?;
    Ok(parse_factory_usage(&root))
}

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

#[cfg(test)]
mod windsurf_fallback_tests {
    use super::windsurf_should_try_fallback;

    #[test]
    fn auth_errors_do_not_fall_through() {
        assert!(!windsurf_should_try_fallback(Some(401)));
        assert!(!windsurf_should_try_fallback(Some(403)));
        assert!(!windsurf_should_try_fallback(Some(429)));
    }

    #[test]
    fn not_found_server_errors_and_transport_fall_through() {
        assert!(windsurf_should_try_fallback(Some(404)));
        assert!(windsurf_should_try_fallback(Some(500)));
        assert!(windsurf_should_try_fallback(Some(503)));
        assert!(windsurf_should_try_fallback(None));
    }

    #[test]
    fn trae_auth_errors_do_not_fall_through() {
        assert!(!super::trae_should_try_fallback(Some(401)));
        assert!(!super::trae_should_try_fallback(Some(403)));
        assert!(super::trae_should_try_fallback(Some(404)));
        assert!(super::trae_should_try_fallback(Some(500)));
        assert!(super::trae_should_try_fallback(None));
    }
}
