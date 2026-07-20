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

fn dto(provider: Provider, name: &str, plan: Option<&str>, five: Option<f64>, week: Option<f64>) -> AccountUsageDto {
    AccountUsageDto {
        id: name.into(),
        provider,
        display_name: name.into(),
        plan: plan.map(Into::into),
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
fn quotes_fields_with_special_chars() {
    assert_eq!(csv_field("plain"), "plain");
    assert_eq!(csv_field("a,b"), "\"a,b\"");
    assert_eq!(csv_field("a\"b"), "\"a\"\"b\"");
    assert_eq!(csv_field("line\nbreak"), "\"line\nbreak\"");
}

#[test]
fn neutralizes_formula_injection() {
    assert_eq!(csv_field("=SUM(A1)"), "'=SUM(A1)");
    assert_eq!(csv_field("+1"), "'+1");
    assert_eq!(csv_field("-cmd"), "'-cmd");
    assert_eq!(csv_field("@handle"), "'@handle");
    // Guard + RFC-4180 quoting compose when a comma is also present.
    assert_eq!(csv_field("=1,2"), "\"'=1,2\"");
    // A normal email is untouched (does not start with a formula char).
    assert_eq!(csv_field("user@example.com"), "user@example.com");
}

#[test]
fn emits_header_and_row_per_window() {
    let body = csv_body(&response(vec![dto(
        Provider::Codex,
        "alice",
        Some("pro"),
        Some(42.5),
        Some(18.0),
    )]));
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines[0], "provider,account,plan,status,window,pool,used_percent");
    assert!(lines.contains(&"codex,alice,pro,ok,5h,,42.5"));
    assert!(lines.contains(&"codex,alice,pro,ok,7d,,18"));
}

#[test]
fn skips_absent_and_nonfinite_windows() {
    let body = csv_body(&response(vec![
        dto(Provider::Codex, "none", None, None, None),
        dto(Provider::Claude, "nan", None, Some(f64::NAN), None),
    ]));
    // Only the header line remains.
    assert_eq!(body.lines().count(), 1);
    assert!(!body.contains("none"));
    assert!(!body.contains("nan"));
}

#[test]
fn empty_plan_renders_as_empty_field() {
    let body = csv_body(&response(vec![dto(Provider::Codex, "a", None, Some(1.0), None)]));
    assert!(body.contains("codex,a,,ok,5h,,1"));
}

#[test]
fn emits_pool_rows() {
    let mut agy = dto(Provider::Agy, "bob", None, None, None);
    agy.pools = vec![PoolDto {
        name: "Gemini Models".into(),
        five_hour: None,
        week: Some(quota(30.0)),
    }];
    let body = csv_body(&response(vec![agy]));
    assert!(body.contains("agy,bob,,ok,7d,Gemini Models,30"));
}
