use super::*;
use crate::api::{AccountUsageDto, PoolDto, QuotaDto, TokenTotalsDto, UsageResponse};

fn quota(percent: f64) -> QuotaDto {
    QuotaDto {
        used_percent: percent,
        window_label: "5h".into(),
        window_seconds: Some(18_000),
        resets_at: None,
    }
}

fn dto(provider: Provider, name: &str, five: Option<f64>, week: Option<f64>) -> AccountUsageDto {
    AccountUsageDto {
        id: name.into(),
        provider,
        auth_kind: "cli_profile",
        display_name: name.into(),
        plan: None,
        status: "ok".into(),
        five_hour: five.map(quota),
        week: week.map(quota),
        pools: Vec::new(),
        breakdown: Vec::new(),
        token_totals: TokenTotalsDto {
            five_hours: 0,
            week: 0,
            month: 0,
        },
        local_status: None,
        detail_suffix: None,
    }
}

fn response(accounts: Vec<AccountUsageDto>) -> UsageResponse {
    UsageResponse {
        updated_at: None,
        count: accounts.len(),
        accounts,
    }
}

#[test]
fn threshold_defaults_and_clamps() {
    assert_eq!(alert_threshold(None), 90.0);
    assert_eq!(alert_threshold(Some("abc")), 90.0);
    assert_eq!(alert_threshold(Some("  75.5 ")), 75.5);
    assert_eq!(alert_threshold(Some("-5")), 0.0);
    assert_eq!(alert_threshold(Some("150")), 100.0);
    assert_eq!(alert_threshold(Some("nan")), 90.0);
}

#[test]
fn only_windows_at_or_above_threshold_are_reported() {
    let resp = response(vec![
        dto(Provider::Codex, "hot", Some(95.0), Some(50.0)),
        dto(Provider::Claude, "cool", Some(10.0), None),
        dto(Provider::Agy, "nan", Some(f64::NAN), None),
    ]);
    let out = alerts_response(&resp, 90.0);
    let json = serde_json::to_string(&out).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["threshold"], 90.0);
    assert_eq!(v["count"], 1);
    assert_eq!(v["alerts"][0]["account"], "hot");
    assert_eq!(v["alerts"][0]["window"], "5h");
    assert_eq!(v["alerts"][0]["used_percent"], 95.0);
    // below-threshold + NaN accounts never appear
    assert!(!json.contains("cool"));
    assert!(!json.contains("\"nan\""));
}

#[test]
fn boundary_value_is_inclusive() {
    let resp = response(vec![dto(Provider::Codex, "edge", Some(90.0), None)]);
    let out = alerts_response(&resp, 90.0);
    assert_eq!(serde_json::to_value(&out).unwrap()["count"], 1);
}

#[test]
fn pool_windows_are_included() {
    let mut agy = dto(Provider::Agy, "bob", None, None);
    agy.pools = vec![PoolDto {
        name: "Gemini Models".into(),
        five_hour: None,
        week: Some(quota(99.0)),
    }];
    let resp = response(vec![agy]);
    let out = alerts_response(&resp, 90.0);
    let v = serde_json::to_value(&out).unwrap();
    assert_eq!(v["count"], 1);
    assert_eq!(v["alerts"][0]["pool"], "Gemini Models");
    assert_eq!(v["alerts"][0]["window"], "7d");
}
