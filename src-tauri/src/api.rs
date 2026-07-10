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
            five_hour: pool.five_hour.as_ref().map(|q| QuotaDto::from_quota(q, "5h")),
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
    pub display_name: String,
    pub plan: Option<String>,
    pub status: String,
    pub five_hour: Option<QuotaDto>,
    pub week: Option<QuotaDto>,
    pub pools: Vec<PoolDto>,
    pub token_totals: TokenTotalsDto,
}

impl AccountUsageDto {
    /// Maps an internal `AccountUsage` snapshot into the public wire shape.
    pub fn from_usage(u: &AccountUsage) -> AccountUsageDto {
        AccountUsageDto {
            id: u.account.id.clone(),
            provider: u.account.provider,
            display_name: u.display_name.clone(),
            plan: u.plan.clone(),
            status: u.status.clone(),
            five_hour: u.five_hour.as_ref().map(|q| QuotaDto::from_quota(q, "5h")),
            week: u.week.as_ref().map(|q| QuotaDto::from_quota(q, "7d")),
            pools: u.pool_breakdown.iter().map(PoolDto::from_pool).collect(),
            token_totals: TokenTotalsDto::from_totals(&u.totals),
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

    fn account_count(&self) -> usize {
        self.inner.lock().map(|g| g.accounts.len()).unwrap_or(0)
    }

    fn updated_at(&self) -> Option<DateTime<Utc>> {
        self.inner.lock().ok().and_then(|g| g.updated_at)
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
        _ => {
            if let Some(rest) = path.strip_prefix("/v1/usage/") {
                let name = rest.trim_end_matches('/');
                return match Provider::from_str(name) {
                    Some(p) => serialize(&state.usage_response_for(p)),
                    None => json(
                        404,
                        format!(
                            r#"{{"error":"unknown_provider","message":"unknown provider '{}' (expected codex, claude, or agy)"}}"#,
                            name.replace('"', "")
                        ),
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
        r#"{{"service":"usagecheck-local-api","version":"{}","endpoints":["GET /health","GET /v1/usage","GET /v1/usage/{{provider}}","GET /openapi.yaml"]}}"#,
        env!("CARGO_PKG_VERSION")
    )
}

fn health_body(state: &ApiState) -> String {
    let updated_at = match state.updated_at() {
        Some(ts) => format!("\"{}\"", ts.to_rfc3339()),
        None => "null".to_string(),
    };
    format!(
        r#"{{"status":"ok","version":"{}","updated_at":{},"account_count":{}}}"#,
        env!("CARGO_PKG_VERSION"),
        updated_at,
        state.account_count()
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
                let header = Header::from_bytes(&b"Content-Type"[..], reply.content_type.as_bytes())
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
mod tests {
    use super::*;
    use usage_core::account::Account;
    use usage_core::models::QuotaUsage;

    fn sample(provider: Provider, id: &str, five: Option<f64>, week: Option<f64>) -> AccountUsage {
        AccountUsage {
            account: Account {
                id: id.into(),
                provider,
                label: id.into(),
            },
            display_name: format!("{id}@example.com"),
            plan: None,
            five_hour: five.map(|p| QuotaUsage {
                percent: p,
                resets_at: None,
                window_seconds: Some(18_000),
            }),
            week: week.map(|p| QuotaUsage {
                percent: p,
                resets_at: None,
                window_seconds: Some(604_800),
            }),
            totals: WindowTotals::default(),
            pool_breakdown: Vec::new(),
            detail_suffix: None,
            status: "ok".into(),
        }
    }

    fn state_with(usages: &[AccountUsage]) -> ApiState {
        let state = ApiState::new();
        state.publish(usages);
        state
    }

    #[test]
    fn dto_renames_percent_and_labels_windows() {
        let dto = AccountUsageDto::from_usage(&sample(Provider::Codex, "a", Some(38.0), Some(6.0)));
        let five = dto.five_hour.unwrap();
        assert_eq!(five.used_percent, 38.0);
        assert_eq!(five.window_label, "5h");
        assert_eq!(dto.week.unwrap().window_label, "7d");
    }

    #[test]
    fn usage_endpoint_serializes_all_accounts() {
        let state = state_with(&[
            sample(Provider::Codex, "a", Some(10.0), None),
            sample(Provider::Claude, "b", Some(20.0), None),
        ]);
        let reply = route(&state, "GET", "/v1/usage");
        assert_eq!(reply.status, 200);
        assert_eq!(reply.content_type, "application/json");
        let v: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
        assert_eq!(v["count"], 2);
        assert_eq!(v["accounts"][0]["provider"], "codex");
        // Never leak internal credential-ish fields.
        assert!(reply.body.contains("used_percent"));
        assert!(!reply.body.contains("access_token"));
    }

    #[test]
    fn provider_filter_returns_only_matching() {
        let state = state_with(&[
            sample(Provider::Codex, "a", Some(10.0), None),
            sample(Provider::Claude, "b", Some(20.0), None),
            sample(Provider::Agy, "c", None, Some(5.0)),
        ]);
        let reply = route(&state, "GET", "/v1/usage/claude");
        let v: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
        assert_eq!(v["count"], 1);
        assert_eq!(v["accounts"][0]["provider"], "claude");
    }

    #[test]
    fn unknown_provider_is_404() {
        let state = state_with(&[]);
        let reply = route(&state, "GET", "/v1/usage/foo");
        assert_eq!(reply.status, 404);
        assert!(reply.body.contains("unknown_provider"));
    }

    #[test]
    fn trailing_slash_provider_is_accepted() {
        let state = state_with(&[sample(Provider::Agy, "c", None, Some(5.0))]);
        let reply = route(&state, "GET", "/v1/usage/agy/");
        assert_eq!(reply.status, 200);
        let v: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
        assert_eq!(v["count"], 1);
    }

    #[test]
    fn health_reports_account_count() {
        let state = state_with(&[sample(Provider::Codex, "a", Some(1.0), None)]);
        let reply = route(&state, "GET", "/health");
        assert_eq!(reply.status, 200);
        let v: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["account_count"], 1);
        assert!(v["updated_at"].is_string());
    }

    #[test]
    fn non_get_is_405() {
        let state = state_with(&[]);
        let reply = route(&state, "POST", "/v1/usage");
        assert_eq!(reply.status, 405);
    }

    #[test]
    fn openapi_is_served_as_yaml() {
        let state = state_with(&[]);
        let reply = route(&state, "GET", "/openapi.yaml");
        assert_eq!(reply.status, 200);
        assert_eq!(reply.content_type, "application/yaml");
        assert!(reply.body.contains("openapi: 3.1.0"));
    }

    #[test]
    fn index_lists_endpoints() {
        let state = state_with(&[]);
        let reply = route(&state, "GET", "/");
        let v: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
        assert_eq!(v["service"], "usagecheck-local-api");
        assert!(v["endpoints"].as_array().unwrap().len() >= 3);
    }

    /// Live end-to-end bind + HTTP round-trip. `#[ignore]`d because it binds a
    /// real socket; run explicitly with `cargo test -p usage-app -- --ignored`.
    #[test]
    #[ignore]
    fn live_server_round_trip() {
        use std::io::{Read, Write};
        use std::net::TcpStream;

        std::env::set_var("USAGECHECK_API_PORT", "5199");
        std::env::remove_var("USAGECHECK_API_DISABLE");
        let state = state_with(&[sample(Provider::Codex, "a", Some(42.0), None)]);
        spawn(state);
        std::thread::sleep(std::time::Duration::from_millis(300));

        let mut stream = TcpStream::connect("127.0.0.1:5199").unwrap();
        stream
            .write_all(b"GET /v1/usage/codex HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();

        let body = raw.split("\r\n\r\n").nth(1).unwrap();
        let v: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(v["count"], 1);
        assert_eq!(v["accounts"][0]["five_hour"]["used_percent"], 42.0);
    }

    #[test]
    fn agy_pools_are_exposed() {
        let mut agy = sample(Provider::Agy, "c", None, Some(18.4));
        agy.pool_breakdown = vec![AgyQuotaPool {
            name: "Gemini Models".into(),
            five_hour: None,
            week: Some(QuotaUsage {
                percent: 0.0,
                resets_at: None,
                window_seconds: Some(604_800),
            }),
        }];
        let state = state_with(&[agy]);
        let reply = route(&state, "GET", "/v1/usage/agy");
        let v: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
        assert_eq!(v["accounts"][0]["pools"][0]["name"], "Gemini Models");
        assert_eq!(v["accounts"][0]["pools"][0]["week"]["used_percent"], 0.0);
    }
}
