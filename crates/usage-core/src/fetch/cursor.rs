use chrono::{TimeZone, Utc};
use serde_json::Value;

use crate::models::QuotaUsage;

/// Parsed Cursor billing-period usage (unofficial Connect RPC).
#[derive(Clone, Debug, PartialEq)]
pub struct CursorQuota {
    pub email: Option<String>,
    pub plan: Option<String>,
    /// Primary used % for the current billing period (0–100).
    pub period: Option<QuotaUsage>,
    /// Secondary tray label, e.g. `"$12.34 left"`.
    pub detail_suffix: Option<String>,
}

fn cents_to_dollars(cents: i64) -> f64 {
    cents as f64 / 100.0
}

fn format_usd_left(cents: i64) -> String {
    format!("${:.2} left", cents_to_dollars(cents))
}

fn parse_plan_usage(plan: &Value) -> Option<(f64, Option<String>)> {
    let percent = plan
        .get("totalPercentUsed")
        .or_else(|| plan.get("apiPercentUsed"))
        .and_then(|v| v.as_f64())?;
    let suffix = plan
        .get("remaining")
        .and_then(|v| v.as_i64())
        .map(format_usd_left);
    Some((percent, suffix))
}

/// Parses `GetCurrentPeriodUsage` JSON into tray-friendly quota fields.
pub fn parse_cursor_period_usage(root: &Value) -> CursorQuota {
    let plan_usage = root.get("planUsage");
    let (percent, detail_suffix) = plan_usage
        .and_then(parse_plan_usage)
        .unwrap_or((0.0, None));

    let billing_end = root
        .get("billingCycleEnd")
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .or_else(|| v.as_i64())
        })
        .and_then(|ms| Utc.timestamp_millis_opt(ms).single());

    let period = plan_usage.map(|_| QuotaUsage {
        percent,
        resets_at: billing_end,
        window_seconds: None,
    });

    CursorQuota {
        email: None,
        plan: None,
        period,
        detail_suffix,
    }
}

/// Merges local auth metadata (email, plan tier) into a parsed quota snapshot.
pub fn cursor_quota_with_auth(
    mut quota: CursorQuota,
    email: Option<String>,
    plan: Option<String>,
) -> CursorQuota {
    if quota.email.is_none() {
        quota.email = email;
    }
    if quota.plan.is_none() {
        quota.plan = plan;
    }
    quota
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_plan_usage_percent_and_remaining() {
        let v = json!({
            "billingCycleEnd": "1771077734000",
            "planUsage": {
                "totalPercentUsed": 46.444,
                "remaining": 16778,
                "limit": 40000
            }
        });
        let q = parse_cursor_period_usage(&v);
        assert!((q.period.as_ref().unwrap().percent - 46.444).abs() < 0.001);
        assert_eq!(q.detail_suffix.as_deref(), Some("$167.78 left"));
        assert!(q.period.as_ref().unwrap().resets_at.is_some());
    }

    #[test]
    fn missing_plan_usage_yields_none_period() {
        let v = json!({ "billingCycleEnd": "1771077734000" });
        let q = parse_cursor_period_usage(&v);
        assert!(q.period.is_none());
    }
}
