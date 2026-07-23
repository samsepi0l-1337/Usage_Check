use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

use crate::models::{QuotaUsage, UsageBreakdownRow};

/// Parsed Cursor billing-period usage (unofficial Connect RPC).
#[derive(Clone, Debug, PartialEq)]
pub struct CursorQuota {
    pub email: Option<String>,
    pub plan: Option<String>,
    /// Primary used % for the current billing period (0–100).
    pub period: Option<QuotaUsage>,
    /// Secondary tray label, e.g. `"$12.34 left"`.
    pub detail_suffix: Option<String>,
    /// Extra breakdown rows ("First Party" / "API"), sourced from
    /// `planUsage.autoPercentUsed` / `planUsage.apiPercentUsed`.
    pub breakdown: Vec<UsageBreakdownRow>,
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

/// Builds a "First Party"/"API" breakdown row when the corresponding
/// `planUsage` field is present. A missing field yields no row; a
/// present-and-zero percent DOES yield a row.
fn breakdown_row(label: &str, plan_usage: &Value, key: &str, resets_at: Option<DateTime<Utc>>) -> Option<UsageBreakdownRow> {
    let percent = plan_usage.get(key).and_then(|v| v.as_f64())?;
    Some(UsageBreakdownRow {
        label: label.to_string(),
        usage: QuotaUsage {
            percent,
            resets_at,
            window_seconds: None,
        },
    })
}

/// Parses `GetCurrentPeriodUsage` JSON into tray-friendly quota fields.
pub fn parse_cursor_period_usage(root: &Value) -> CursorQuota {
    // Key `period` off a successfully *parsed* percent, not the raw `planUsage`
    // Option — otherwise a `planUsage` object lacking a percent field would leak
    // the `0.0` default and display "0% used" (full quota) for unknown usage.
    let plan_usage = root.get("planUsage");
    let parsed = plan_usage.and_then(parse_plan_usage);

    let billing_end = root
        .get("billingCycleEnd")
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .or_else(|| v.as_i64())
        })
        .and_then(|ms| Utc.timestamp_millis_opt(ms).single());

    let (period, detail_suffix) = match parsed {
        Some((percent, suffix)) => (
            Some(QuotaUsage {
                percent,
                resets_at: billing_end,
                window_seconds: None,
            }),
            suffix,
        ),
        None => (None, None),
    };

    let breakdown = plan_usage
        .into_iter()
        .flat_map(|plan_usage| {
            [
                breakdown_row("First Party", plan_usage, "autoPercentUsed", billing_end),
                breakdown_row("API", plan_usage, "apiPercentUsed", billing_end),
            ]
        })
        .flatten()
        .collect();

    CursorQuota {
        email: None,
        plan: None,
        period,
        detail_suffix,
        breakdown,
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
        assert!(q.breakdown.is_empty());
    }

    #[test]
    fn missing_plan_usage_yields_none_period() {
        let v = json!({ "billingCycleEnd": "1771077734000" });
        let q = parse_cursor_period_usage(&v);
        assert!(q.period.is_none());
        assert!(q.breakdown.is_empty());
    }

    #[test]
    fn parses_first_party_and_api_breakdown_rows() {
        let v = json!({
            "billingCycleEnd": "1771077734000",
            "planUsage": {
                "totalPercentUsed": 20.0,
                "autoPercentUsed": 17.0,
                "apiPercentUsed": 41.0,
                "remaining": 16778
            }
        });
        let q = parse_cursor_period_usage(&v);
        assert_eq!(q.breakdown.len(), 2);
        assert_eq!(q.breakdown[0].label, "First Party");
        assert_eq!(q.breakdown[0].usage.percent, 17.0);
        assert!(q.breakdown[0].usage.resets_at.is_some());
        assert_eq!(q.breakdown[1].label, "API");
        assert_eq!(q.breakdown[1].usage.percent, 41.0);
    }

    #[test]
    fn absent_first_party_and_api_fields_yield_empty_breakdown() {
        let v = json!({
            "billingCycleEnd": "1771077734000",
            "planUsage": { "totalPercentUsed": 20.0, "remaining": 16778 }
        });
        let q = parse_cursor_period_usage(&v);
        assert!(q.breakdown.is_empty());
        // Primary `period` line stays unaffected by breakdown absence.
        assert!((q.period.as_ref().unwrap().percent - 20.0).abs() < 0.001);
    }

    #[test]
    fn plan_usage_present_without_percent_yields_none_period() {
        // A `planUsage` object with no `totalPercentUsed`/`apiPercentUsed` must
        // NOT be reported as 0% used (full quota); usage is unknown → no period.
        let v = json!({
            "billingCycleEnd": "1771077734000",
            "planUsage": { "remaining": 16778, "limit": 40000 }
        });
        let q = parse_cursor_period_usage(&v);
        assert!(
            q.period.is_none(),
            "percent-less planUsage must yield None period, not 0%"
        );
        assert_eq!(q.detail_suffix, None);
    }
}
