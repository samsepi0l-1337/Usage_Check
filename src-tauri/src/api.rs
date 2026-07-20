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
use tiny_http::{Header, Response, Server};

use usage_core::account::Provider;
use usage_core::fetch::agy::AgyQuotaPool;
use usage_core::fetch::codex::window_label;
use usage_core::models::{QuotaUsage, WindowTotals};

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
pub struct QuotaDto {
    pub used_percent: f64,
    pub window_label: String,
    pub window_seconds: Option<i64>,
    pub resets_at: Option<DateTime<Utc>>,
}

impl QuotaDto {
    fn from_quota(q: &QuotaUsage, fallback_label: &str) -> QuotaDto {
        QuotaDto {
            used_percent: q.percent,
            window_label: window_label(q.window_seconds, fallback_label),
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
                .map(|q| QuotaDto::from_quota(q, "5h")),
            week: pool.week.as_ref().map(|q| QuotaDto::from_quota(q, "7d")),
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
    pub token_totals: TokenTotalsDto,
    pub local_status: Option<String>,
    pub detail_suffix: Option<String>,
}

impl AccountUsageDto {
    /// Maps an internal `AccountUsage` snapshot into the public wire shape.
    pub fn from_usage(u: &AccountUsage) -> AccountUsageDto {
        AccountUsageDto {
            id: u.account.id.clone(),
            provider: u.account.provider,
            auth_kind: crate::api_accounts::auth_kind(&u.account.auth_source),
            display_name: u.display_name.clone(),
            plan: u.plan.clone(),
            status: u.status.clone(),
            five_hour: u.five_hour.as_ref().map(|q| QuotaDto::from_quota(q, "5h")),
            week: u.week.as_ref().map(|q| QuotaDto::from_quota(q, "7d")),
            pools: u.pool_breakdown.iter().map(PoolDto::from_pool).collect(),
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

// ---------------------------------------------------------------------------
// Shared, poll-published snapshot
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Snapshot {
    updated_at: Option<DateTime<Utc>>,
    accounts: Vec<AccountUsageDto>,
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

    /// Replaces the served snapshot with the latest poll result. Called from
    /// the tray refresh path; runs synchronously (no `.await`).
    pub fn publish(&self, usages: &[AccountUsage]) {
        let accounts = usages.iter().map(AccountUsageDto::from_usage).collect();
        if let Ok(mut guard) = self.inner.lock() {
            guard.updated_at = Some(Utc::now());
            guard.accounts = accounts;
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
struct Reply {
    status: u16,
    content_type: &'static str,
    body: String,
}

fn json(status: u16, body: String) -> Reply {
    Reply {
        status,
        content_type: "application/json",
        body,
    }
}

/// Resolves a request `(method, path)` into a response. Pure: takes the state
/// by reference and does no I/O, so it is unit-testable.
fn route(state: &ApiState, method: &str, path: &str) -> Reply {
    if method != "GET" {
        return json(
            405,
            r#"{"error":"method_not_allowed","message":"only GET is supported"}"#.to_string(),
        );
    }

    match path {
        "/" => json(200, index_body()),
        "/health" => json(200, health_body(state)),
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
            let raw = std::env::var("USAGECHECK_ALERT_THRESHOLD").ok();
            let threshold = crate::api_alerts::alert_threshold(raw.as_deref());
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
                                "unknown provider '{}' (expected {})",
                                name,
                                if cfg!(feature = "edition-pro") {
                                    "codex, claude, agy, cursor, grok, or higgsfield"
                                } else {
                                    "codex, claude, or agy"
                                }
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
        r#"{{"service":"usagecheck-local-api","version":"{}","endpoints":["GET /health","GET /v1/usage","GET /v1/usage/{{provider}}","GET /v1/accounts","GET /v1/alerts","GET /v1/usage.csv","GET /metrics","GET /openapi.yaml"]}}"#,
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
fn configured_port() -> u16 {
    std::env::var("USAGECHECK_API_PORT")
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .filter(|p| *p != 0)
        .unwrap_or(DEFAULT_PORT)
}

/// True unless `USAGECHECK_API_DISABLE` is set to a truthy value.
fn is_disabled() -> bool {
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

/// Starts the localhost API server on a dedicated thread. No-op when disabled
/// via env. Bind failures are logged (not fatal) so the tray still runs.
pub fn spawn(state: ApiState) {
    if is_disabled() {
        return;
    }
    let port = configured_port();
    std::thread::Builder::new()
        .name("usagecheck-api".into())
        .spawn(move || {
            let addr = format!("127.0.0.1:{port}");
            let server = match Server::http(&addr) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("api: failed to bind {addr}: {e} (is another instance running?)");
                    return;
                }
            };
            for request in server.incoming_requests() {
                // Strip any query string before routing.
                let path = request.url().split('?').next().unwrap_or("/").to_string();
                let method = request.method().as_str().to_string();
                let reply = route(&state, &method, &path);
                let header =
                    Header::from_bytes(&b"Content-Type"[..], reply.content_type.as_bytes())
                        .expect("static content-type header is valid");
                let response = Response::from_string(reply.body)
                    .with_status_code(reply.status)
                    .with_header(header);
                let _ = request.respond(response);
            }
        })
        .expect("failed to spawn usagecheck-api thread");
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;
