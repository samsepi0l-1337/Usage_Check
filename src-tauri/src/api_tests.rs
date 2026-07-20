use super::*;
use usage_core::account::{Account, AuthSource, ProfileOwnership};
use usage_core::models::QuotaUsage;
fn sample_auth_source(provider: Provider, identity: &str) -> AuthSource {
    match provider {
        Provider::Codex | Provider::Claude => AuthSource::CliProfile {
            profile_root: format!("/profiles/{identity}").into(),
            ownership: ProfileOwnership::External,
            expected_identity: identity.into(),
        },
        Provider::Agy => AuthSource::BrowserOAuth {
            credential_id: format!("{identity}-credential"),
        },
        #[cfg(feature = "edition-pro")]
        Provider::Cursor => AuthSource::CursorDatabase {
            database_path: "/profiles/cursor/state.vscdb".into(),
            expected_identity: identity.into(),
        },
        #[cfg(feature = "edition-pro")]
        Provider::Grok => AuthSource::XaiManagement {
            credential_id: format!("{identity}-credential"),
            team_id: identity.into(),
        },
        #[cfg(feature = "edition-pro")]
        Provider::Higgsfield => AuthSource::HiggsfieldCli {
            expected_identity: identity.into(),
        },
    }
}
fn sample(provider: Provider, id: &str, five: Option<f64>, week: Option<f64>) -> AccountUsage {
    AccountUsage {
        account: Account {
            id: id.into(),
            provider,
            label: id.into(),
            auth_source: sample_auth_source(provider, id),
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
        local_status: None,
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
    // Enriched fields: freshly published snapshot is ~0s stale, one ok account.
    assert!(v["stale_seconds"].is_number());
    assert_eq!(v["status_counts"]["ok"], 1);
}
#[test]
fn accounts_endpoint_lists_inventory_with_auth_kind() {
    let state = state_with(&[
        sample(Provider::Codex, "a", Some(10.0), None),
        sample(Provider::Agy, "b", None, Some(5.0)),
    ]);
    let reply = route(&state, "GET", "/v1/accounts");
    assert_eq!(reply.status, 200);
    let v: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
    assert_eq!(v["count"], 2);
    assert_eq!(v["accounts"][0]["provider"], "codex");
    // Codex sample uses a CliProfile auth source.
    assert_eq!(v["accounts"][0]["auth_kind"], "cli_profile");
    assert_eq!(v["accounts"][1]["auth_kind"], "browser_oauth");
    assert!(!reply.body.contains("access_token"));
}

#[test]
fn alerts_endpoint_reports_near_limit_accounts() {
    std::env::remove_var("USAGECHECK_ALERT_THRESHOLD");
    let state = state_with(&[
        sample(Provider::Codex, "hot", Some(96.0), None),
        sample(Provider::Claude, "cool", Some(20.0), None),
    ]);
    let reply = route(&state, "GET", "/v1/alerts");
    assert_eq!(reply.status, 200);
    let v: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
    assert_eq!(v["threshold"], 90.0);
    assert_eq!(v["count"], 1);
    assert_eq!(v["alerts"][0]["account"], "hot@example.com");
    assert!(!reply.body.contains("cool"));
}

#[test]
fn usage_dto_includes_auth_kind() {
    let dto = AccountUsageDto::from_usage(&sample(Provider::Codex, "a", Some(1.0), None));
    assert_eq!(dto.auth_kind, "cli_profile");
}

#[test]
fn csv_endpoint_serves_text_csv() {
    let state = state_with(&[sample(Provider::Codex, "a", Some(42.5), None)]);
    let reply = route(&state, "GET", "/v1/usage.csv");
    assert_eq!(reply.status, 200);
    assert!(reply.content_type.starts_with("text/csv"));
    assert!(reply.body.starts_with("provider,account,plan,status,window,pool,used_percent\n"));
    assert!(reply.body.contains("codex,a@example.com,,ok,5h,,42.5"));
    assert!(!reply.body.contains("access_token"));
}

#[test]
fn base_url_uses_localhost_and_port() {
    assert_eq!(format_base_url(5178), "http://127.0.0.1:5178/");
    assert_eq!(format_base_url(9000), "http://127.0.0.1:9000/");
}

#[test]
fn metrics_endpoint_serves_prometheus_text() {
    let state = state_with(&[
        sample(Provider::Codex, "a", Some(42.5), None),
        sample(Provider::Claude, "b", Some(10.0), Some(3.0)),
    ]);
    let reply = route(&state, "GET", "/metrics");
    assert_eq!(reply.status, 200);
    assert!(reply.content_type.starts_with("text/plain"));
    assert!(reply.body.contains("usagecheck_account_count 2"));
    assert!(reply
        .body
        .contains("usagecheck_used_percent{provider=\"codex\",account=\"a@example.com\",window=\"5h\"} 42.5"));
    assert!(!reply.body.contains("access_token"));
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
        .write_all(
            b"GET /v1/usage/codex HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
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
#[test]
fn dto_maps_local_status_from_provenance() {
    let mut degraded = sample(Provider::Codex, "a", Some(38.0), Some(6.0));
    degraded.local_status = Some("unavailable".into());
    let dto = AccountUsageDto::from_usage(&degraded);
    assert_eq!(dto.local_status.as_deref(), Some("unavailable"));
    let clean = sample(Provider::Codex, "b", Some(1.0), Some(1.0));
    assert_eq!(AccountUsageDto::from_usage(&clean).local_status, None);
}
#[test]
fn dto_includes_detail_suffix() {
    let mut usage = sample(Provider::Codex, "a", None, None);
    usage.detail_suffix = Some("809 credits".into());
    let dto = AccountUsageDto::from_usage(&usage);
    assert_eq!(dto.detail_suffix, Some("809 credits".to_string()));
    assert!(serde_json::to_string(&dto).unwrap().contains("detail_suffix"));
}
#[test]
fn dto_never_serializes_auth_metadata() {
    let usages = std::iter::once(sample(Provider::Codex, "cli", None, None));
    #[cfg(feature = "edition-pro")]
    let usages = usages.chain([
        sample(Provider::Cursor, "cursor", None, None),
        sample(Provider::Grok, "grok", None, None),
    ]);
    for usage in usages {
        let json = serde_json::to_string(&AccountUsageDto::from_usage(&usage)).unwrap();
        for private in [
            "auth_source", "profile_root", "credential_id", "team_id", "database_path",
            "access_token", "management",
        ] {
            assert!(!json.contains(private), "DTO leaked {private}: {json}");
        }
    }
}
#[test]
fn status_stale_serializes() {
    let mut usage = sample(Provider::Codex, "a", None, None);
    usage.status = "stale".to_string();
    assert!(serde_json::to_string(&AccountUsageDto::from_usage(&usage)).unwrap().contains("\"stale\""));
}
#[test]
fn openapi_declares_detail_suffix_and_stale() {
    assert!(["detail_suffix", "stale", "higgsfield"].into_iter().all(|v| OPENAPI_YAML.contains(v)));
}
#[cfg(feature = "edition-pro")]
#[test]
fn provider_filter_accepts_pro_providers() {
    let state = state_with(&[
        sample(Provider::Cursor, "cursor", None, None),
        sample(Provider::Grok, "grok", None, None),
        sample(Provider::Higgsfield, "higgsfield", None, None),
    ]);
    for provider in ["cursor", "grok", "higgsfield"] {
        let reply = route(&state, "GET", &format!("/v1/usage/{provider}"));
        assert_eq!(reply.status, 200);
        let body: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
        assert_eq!(body["count"], 1);
        assert_eq!(body["accounts"][0]["provider"], provider);
    }
}
