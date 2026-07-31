//! Local HTTP API exposing the current usage snapshot for other agents.
//!
//! A small `tiny_http` server (localhost-only) serves the same Codex / Claude /
//! agy usage the tray menu shows, in a stable JSON contract so MCP servers and
//! agent skills can wrap it instead of scraping the tray UI.
//!
//! Freshness: the background poll loop calls [`ApiState::publish`] on every
//! refresh, so the API returns exactly what the tray last rendered — no extra
//! provider API calls per request.
//!
//! SECURITY: binds `127.0.0.1` only; the API is read-only and never returns
//! access tokens, refresh tokens, or other credential values.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::Serialize;

use usage_core::account::Provider;
use usage_core::fetch::agy::AgyQuotaPool;
use usage_core::fetch::codex::window_label;
use usage_core::models::{QuotaUsage, UsageBreakdownRow, WindowTotals};

use crate::poller::AccountUsage;

/// Default localhost port. Chosen to avoid Vite (5173), Codex OAuth (1455),
/// and agy OAuth (8080) callbacks. Override with `USAGECHECK_API_PORT`.
const DEFAULT_PORT: u16 = 5178;

/// Embedded copy of the OpenAPI spec, served at `/openapi.yaml`.
const OPENAPI_YAML: &str = include_str!("../../docs/openapi.yaml");

// ---------------------------------------------------------------------------
// Wire DTOs (stable public contract, decoupled from internal poller structs)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
#[serde(transparent)]
pub struct WindowLabelDto(Option<String>);

#[derive(Clone, Copy, Debug)]
enum WindowLabelHint {
    FiveHour,
    SevenDay,
    BillingPeriod,
    NoLabel,
}

impl WindowLabelHint {
    fn fallback(self) -> Option<&'static str> {
        match self {
            WindowLabelHint::FiveHour => Some("5h"),
            WindowLabelHint::SevenDay => Some("7d"),
            WindowLabelHint::BillingPeriod => Some("billing period"),
            WindowLabelHint::NoLabel => None,
        }
    }

    fn for_account_week(provider: Provider) -> WindowLabelHint {
        match provider {
            Provider::Codex | Provider::Claude | Provider::Agy => WindowLabelHint::SevenDay,
            Provider::Cursor | Provider::Grok => WindowLabelHint::BillingPeriod,
            Provider::Higgsfield => WindowLabelHint::NoLabel,
        }
    }

    fn for_breakdown(provider: Provider) -> WindowLabelHint {
        match provider {
            Provider::Cursor | Provider::Grok => WindowLabelHint::BillingPeriod,
            Provider::Codex | Provider::Claude | Provider::Agy | Provider::Higgsfield => {
                WindowLabelHint::NoLabel
            }
        }
    }
}

impl WindowLabelDto {
    fn from_quota(q: &QuotaUsage, hint: WindowLabelHint) -> WindowLabelDto {
        WindowLabelDto(
            q.window_seconds
                .map(|seconds| window_label(Some(seconds), ""))
                .or_else(|| hint.fallback().map(str::to_owned)),
        )
    }
}

impl From<&str> for WindowLabelDto {
    fn from(label: &str) -> WindowLabelDto {
        WindowLabelDto(Some(label.to_string()))
    }
}

impl From<String> for WindowLabelDto {
    fn from(label: String) -> WindowLabelDto {
        WindowLabelDto(Some(label))
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct QuotaDto {
    pub used_percent: f64,
    pub window_label: WindowLabelDto,
    pub window_seconds: Option<i64>,
    pub resets_at: Option<DateTime<Utc>>,
}

impl QuotaDto {
    fn from_quota(q: &QuotaUsage, label_hint: WindowLabelHint) -> QuotaDto {
        QuotaDto {
            used_percent: q.percent,
            window_label: WindowLabelDto::from_quota(q, label_hint),
            window_seconds: q.window_seconds,
            resets_at: q.resets_at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PoolDto {
    pub name: String,
    pub five_hour: Option<QuotaDto>,
    pub week: Option<QuotaDto>,
}

impl PoolDto {
    fn from_pool(pool: &AgyQuotaPool) -> PoolDto {
        PoolDto {
            name: pool.name.clone(),
            five_hour: pool
                .five_hour
                .as_ref()
                .map(|q| QuotaDto::from_quota(q, WindowLabelHint::FiveHour)),
            week: pool
                .week
                .as_ref()
                .map(|q| QuotaDto::from_quota(q, WindowLabelHint::SevenDay)),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct BreakdownDto {
    pub label: String,
    pub usage: QuotaDto,
}

impl BreakdownDto {
    fn from_row(row: &UsageBreakdownRow, label_hint: WindowLabelHint) -> BreakdownDto {
        BreakdownDto {
            label: row.label.clone(),
            usage: QuotaDto::from_quota(&row.usage, label_hint),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct TokenTotalsDto {
    pub five_hours: i64,
    pub week: i64,
    pub month: i64,
}

impl TokenTotalsDto {
    fn from_totals(t: &WindowTotals) -> TokenTotalsDto {
        TokenTotalsDto {
            five_hours: t.five_hours,
            week: t.week,
            month: t.month,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AccountUsageDto {
    pub id: String,
    pub provider: Provider,
    /// Non-secret label for how the account authenticates (e.g. cli_profile).
    pub auth_kind: &'static str,
    pub display_name: String,
    pub plan: Option<String>,
    pub status: String,
    pub five_hour: Option<QuotaDto>,
    pub week: Option<QuotaDto>,
    pub pools: Vec<PoolDto>,
    pub breakdown: Vec<BreakdownDto>,
    pub token_totals: TokenTotalsDto,
    pub local_status: Option<String>,
    pub detail_suffix: Option<String>,
}

impl AccountUsageDto {
    /// Maps an internal `AccountUsage` snapshot into the public wire shape.
    pub fn from_usage(u: &AccountUsage) -> AccountUsageDto {
        let week_label_hint = WindowLabelHint::for_account_week(u.account.provider);
        let breakdown_label_hint = WindowLabelHint::for_breakdown(u.account.provider);
        AccountUsageDto {
            id: u.account.id.clone(),
            provider: u.account.provider,
            auth_kind: crate::api_accounts::auth_kind(&u.account.auth_source),
            display_name: u.display_name.clone(),
            plan: u.plan.clone(),
            status: u.status.clone(),
            five_hour: u
                .five_hour
                .as_ref()
                .map(|q| QuotaDto::from_quota(q, WindowLabelHint::FiveHour)),
            week: u
                .week
                .as_ref()
                .map(|q| QuotaDto::from_quota(q, week_label_hint)),
            pools: u.pool_breakdown.iter().map(PoolDto::from_pool).collect(),
            breakdown: u
                .breakdown
                .iter()
                .map(|row| BreakdownDto::from_row(row, breakdown_label_hint))
                .collect(),
            token_totals: TokenTotalsDto::from_totals(&u.totals),
            local_status: u.local_status.clone(),
            detail_suffix: u.detail_suffix.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct UsageResponse {
    pub updated_at: Option<DateTime<Utc>>,
    pub count: usize,
    pub accounts: Vec<AccountUsageDto>,
}

/// Privacy-safe view of the current runtime license gate. It intentionally
/// contains no persisted key, signed token, or device identifier.
#[derive(Clone, Debug, Serialize)]
pub struct LicenseResponse {
    pub status: &'static str,
    pub expires_at: Option<DateTime<Utc>>,
    pub forced: bool,
}

impl LicenseResponse {
    fn from_status(status: crate::license::LicenseStatus) -> LicenseResponse {
        match status {
            crate::license::LicenseStatus::Free => LicenseResponse {
                status: "free",
                expires_at: None,
                forced: false,
            },
            crate::license::LicenseStatus::Pro { expires_at } => LicenseResponse {
                status: "pro",
                expires_at,
                forced: false,
            },
            crate::license::LicenseStatus::ProDevOverride => LicenseResponse {
                status: "pro",
                expires_at: None,
                forced: true,
            },
            crate::license::LicenseStatus::Expired => LicenseResponse {
                status: "expired",
                expires_at: None,
                forced: false,
            },
            crate::license::LicenseStatus::GracePeriodEnded => LicenseResponse {
                status: "grace_period_ended",
                expires_at: None,
                forced: false,
            },
        }
    }
}

impl Default for LicenseResponse {
    fn default() -> LicenseResponse {
        LicenseResponse::from_status(crate::license::LicenseStatus::Free)
    }
}

// ---------------------------------------------------------------------------
// Shared, poll-published snapshot
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Snapshot {
    updated_at: Option<DateTime<Utc>>,
    accounts: Vec<AccountUsageDto>,
    license: LicenseResponse,
}

/// Cheaply-clonable handle to the latest usage snapshot. Managed by Tauri and
/// shared with the HTTP server thread.
#[derive(Clone, Default)]
pub struct ApiState {
    inner: Arc<Mutex<Snapshot>>,
}

impl ApiState {
    pub fn new() -> ApiState {
        ApiState::default()
    }

    /// Replaces the served usage and license snapshot with the latest poll
    /// result. License filesystem reads happen here on the refresh path, not
    /// while serving an HTTP request. Runs synchronously (no `.await`).
    pub fn publish(&self, usages: &[AccountUsage]) {
        let accounts = usages.iter().map(AccountUsageDto::from_usage).collect();
        let license = LicenseResponse::from_status(crate::license::status());
        if let Ok(mut guard) = self.inner.lock() {
            guard.updated_at = Some(Utc::now());
            guard.accounts = accounts;
            guard.license = license;
        }
    }

    /// Full snapshot for `GET /v1/usage`.
    fn usage_response(&self) -> UsageResponse {
        let guard = self.inner.lock().ok();
        match guard {
            Some(g) => UsageResponse {
                updated_at: g.updated_at,
                count: g.accounts.len(),
                accounts: g.accounts.clone(),
            },
            None => UsageResponse {
                updated_at: None,
                count: 0,
                accounts: Vec::new(),
            },
        }
    }

    /// Snapshot filtered to a single provider for `GET /v1/usage/{provider}`.
    fn usage_response_for(&self, provider: Provider) -> UsageResponse {
        let mut resp = self.usage_response();
        resp.accounts.retain(|a| a.provider == provider);
        resp.count = resp.accounts.len();
        resp
    }

    /// Poll-published license state for `GET /v1/license`.
    fn license_response(&self) -> LicenseResponse {
        self.inner
            .lock()
            .map(|guard| guard.license.clone())
            .unwrap_or_default()
    }

    /// Snapshot inputs for `/health`: publish timestamp + each account's status.
    fn health_inputs(&self) -> (Option<DateTime<Utc>>, Vec<String>) {
        match self.inner.lock() {
            Ok(g) => (
                g.updated_at,
                g.accounts.iter().map(|a| a.status.clone()).collect(),
            ),
            Err(_) => (None, Vec::new()),
        }
    }
}

// ---------------------------------------------------------------------------
// Routing
// ---------------------------------------------------------------------------

/// A resolved route + the JSON/YAML body and status to serve.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) content_type: &'static str,
    pub(crate) body: String,
}

pub(crate) fn json(status: u16, body: String) -> Reply {
    Reply {
        status,
        content_type: "application/json",
        body,
    }
}

/// Resolves a request `(method, path)` into a response. License and usage
/// routes read only the supplied in-memory snapshot; they do no filesystem or
/// provider I/O per request.
pub(crate) fn route(state: &ApiState, method: &str, path: &str) -> Reply {
    if method != "GET" {
        return json(
            405,
            r#"{"error":"method_not_allowed","message":"only GET is supported"}"#.to_string(),
        );
    }

    match path {
        "/" => json(200, index_body()),
        "/health" => json(200, health_body(state)),
        "/v1/license" => serialize(&state.license_response()),
        "/openapi.yaml" | "/openapi.yml" => Reply {
            status: 200,
            content_type: "application/yaml",
            body: OPENAPI_YAML.to_string(),
        },
        "/v1/usage" => serialize(&state.usage_response()),
        "/v1/accounts" => {
            let resp = state.usage_response();
            serialize(&crate::api_accounts::accounts_response(&resp))
        }
        "/v1/alerts" => {
            let resp = state.usage_response();
            let threshold = crate::api_alerts::current_alert_threshold();
            serialize(&crate::api_alerts::alerts_response(&resp, threshold))
        }
        "/v1/usage.csv" => Reply {
            status: 200,
            content_type: crate::api_csv::CSV_CONTENT_TYPE,
            body: crate::api_csv::csv_body(&state.usage_response()),
        },
        "/metrics" => Reply {
            status: 200,
            content_type: crate::api_metrics::METRICS_CONTENT_TYPE,
            body: crate::api_metrics::metrics_body(&state.usage_response()),
        },
        _ => {
            if let Some(rest) = path.strip_prefix("/v1/usage/") {
                let name = rest.trim_end_matches('/');
                return match Provider::from_str(name) {
                    Some(p) => serialize(&state.usage_response_for(p)),
                    None => json(
                        404,
                        serde_json::json!({
                            "error": "unknown_provider",
                            "message": format!(
                                "unknown provider '{}' (expected codex, claude, agy, cursor, grok, or higgsfield)",
                                name
                            ),
                        })
                        .to_string(),
                    ),
                };
            }
            json(
                404,
                r#"{"error":"not_found","message":"no such endpoint"}"#.to_string(),
            )
        }
    }
}

fn serialize<T: Serialize>(value: &T) -> Reply {
    match serde_json::to_string(value) {
        Ok(body) => json(200, body),
        Err(_) => json(
            500,
            r#"{"error":"serialization_failed","message":"could not encode response"}"#.to_string(),
        ),
    }
}

fn index_body() -> String {
    format!(
        r#"{{"service":"usagecheck-local-api","version":"{}","endpoints":["GET /health","GET /v1/license","GET /v1/usage","GET /v1/usage/{{provider}}","GET /v1/accounts","GET /v1/alerts","GET /v1/usage.csv","GET /metrics","GET /openapi.yaml"]}}"#,
        env!("CARGO_PKG_VERSION")
    )
}

fn health_body(state: &ApiState) -> String {
    let (updated_at, statuses) = state.health_inputs();
    let status_refs: Vec<&str> = statuses.iter().map(String::as_str).collect();
    crate::api_health::health_body(
        env!("CARGO_PKG_VERSION"),
        updated_at,
        &status_refs,
        Utc::now(),
    )
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// Resolves the configured port: `USAGECHECK_API_PORT` if a valid port,
/// else [`DEFAULT_PORT`].
pub(crate) fn configured_port() -> u16 {
    std::env::var("USAGECHECK_API_PORT")
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .filter(|p| *p != 0)
        .unwrap_or(DEFAULT_PORT)
}

/// True unless `USAGECHECK_API_DISABLE` is set to a truthy value.
pub(crate) fn is_disabled() -> bool {
    matches!(
        std::env::var("USAGECHECK_API_DISABLE").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

/// Localhost base URL for the API on `port` (pure; testable).
fn format_base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/")
}

/// The localhost base URL other tools/the tray can open, or `None` when the
/// API is disabled via env.
pub(crate) fn public_url() -> Option<String> {
    if is_disabled() {
        return None;
    }
    Some(format_base_url(configured_port()))
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;
