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

use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use usage_core::account::{Account, AuthSource, Credentials, Provider};
use usage_core::fetch::agy::{parse_agy_quota_summary, AgyQuota};
use usage_core::fetch::claude::{parse_claude_usage, ClaudeQuota};
use usage_core::fetch::codex::{parse_codex_usage, CodexQuota};
#[cfg(feature = "edition-pro")]
use usage_core::fetch::cursor::{cursor_quota_with_auth, parse_cursor_period_usage, CursorQuota};
#[cfg(feature = "edition-pro")]
use usage_core::fetch::grok::{parse_grok_prepaid_balance, GrokPrepaid};
#[cfg(feature = "edition-pro")]
use usage_core::fetch::higgsfield::{parse_higgsfield_account, HiggsfieldCredits};
use usage_core::models::{LocalProvenance, LocalUsage};

use crate::agy_local;
use crate::paths;
use crate::store::AccountStore;

mod local_scan;
use local_scan::local_usage_for_provider;

mod usage_model;
pub use usage_model::{
    account_usage_from_agy,
    assemble_account_usage,
    AccountUsage,
    FetchOutcome,
};
use usage_model::{
    claude_fetch_outcome,
    claude_identity_status,
    codex_fetch_outcome,
    codex_identity_status,
    status_for_failure,
};
#[cfg(feature = "edition-pro")]
use usage_model::{
    account_usage_from_cursor,
    account_usage_from_grok,
    account_usage_from_higgsfield,
};

mod last_success;
pub use last_success::evict_last_success;
use last_success::{apply_last_success, last_success_cache};

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

}


#[cfg(test)]
#[cfg(feature = "edition-pro")]
mod tests_pro {
    use super::*;
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
