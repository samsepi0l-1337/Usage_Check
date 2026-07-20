use super::*;
use crate::api::{AccountUsageDto, PoolDto, QuotaDto, TokenTotalsDto, UsageResponse};
use usage_core::account::Provider;

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
        display_name: name.into(),
        plan: None,
        status: "ok".into(),
        five_hour: five.map(quota),
        week: week.map(quota),
        pools: Vec::new(),
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
fn escapes_special_label_chars() {
    assert_eq!(escape_label("a\\b\"c\nd"), "a\\\\b\\\"c\\nd");
    assert_eq!(escape_label("plain"), "plain");
}

#[test]
fn emits_help_type_and_account_count() {
    let body = metrics_body(&response(vec![dto(Provider::Codex, "a", Some(1.0), None)]));
    assert!(body.contains("# TYPE usagecheck_account_count gauge"));
    assert!(body.contains("usagecheck_account_count 1"));
    assert!(body.contains("# TYPE usagecheck_used_percent gauge"));
}

#[test]
fn emits_one_line_per_window() {
    let body = metrics_body(&response(vec![dto(Provider::Codex, "alice", Some(42.5), Some(18.0))]));
    assert!(body.contains(
        "usagecheck_used_percent{provider=\"codex\",account=\"alice\",window=\"5h\"} 42.5"
    ));
    assert!(body.contains(
        "usagecheck_used_percent{provider=\"codex\",account=\"alice\",window=\"7d\"} 18"
    ));
}

#[test]
fn skips_absent_windows_and_nonfinite() {
    let body = metrics_body(&response(vec![
        dto(Provider::Codex, "none", None, None),
        dto(Provider::Claude, "nan", Some(f64::NAN), Some(f64::INFINITY)),
    ]));
    // No used_percent lines emitted for missing or non-finite values.
    assert!(!body.contains("account=\"none\""));
    assert!(!body.contains("account=\"nan\""));
    assert!(body.contains("usagecheck_account_count 2"));
}

#[test]
fn emits_pool_labeled_lines() {
    let mut agy = dto(Provider::Agy, "bob", None, None);
    agy.pools = vec![PoolDto {
        name: "Gemini Models".into(),
        five_hour: None,
        week: Some(quota(30.0)),
    }];
    let body = metrics_body(&response(vec![agy]));
    assert!(body.contains(
        "usagecheck_used_percent{provider=\"agy\",account=\"bob\",pool=\"Gemini Models\",window=\"7d\"} 30"
    ));
}
