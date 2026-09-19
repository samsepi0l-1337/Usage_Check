use serde_json::Value;

use crate::models::QuotaUsage;

const DAY_SECS: i64 = 24 * 60 * 60;
const WEEK_SECS: i64 = 7 * 24 * 60 * 60;

/// Parsed Windsurf `GetUserStatus` quota (remaining percents → used %).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindsurfQuota {
    pub email: Option<String>,
    pub plan: Option<String>,
    pub five_hour: Option<QuotaUsage>,
    pub week: Option<QuotaUsage>,
    pub detail_suffix: Option<String>,
}

fn json_f64(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_i64().map(|n| n as f64))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

fn nonempty_str(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn first_f64(root: &Value, paths: &[&[&str]]) -> Option<f64> {
    for path in paths {
        let mut current = root;
        for key in *path {
            current = &current[*key];
            if current.is_null() {
                break;
            }
        }
        if let Some(n) = json_f64(current).filter(|n| n.is_finite()) {
            return Some(n);
        }
    }
    None
}

fn first_str(root: &Value, paths: &[&[&str]]) -> Option<String> {
    for path in paths {
        let mut current = root;
        for key in *path {
            current = &current[*key];
            if current.is_null() {
                break;
            }
        }
        if let Some(s) = nonempty_str(current) {
            return Some(s);
        }
    }
    None
}

fn used_from_remaining(remaining: f64) -> f64 {
    (100.0 - remaining).clamp(0.0, 100.0)
}

fn quota_from_remaining(remaining: f64, window_seconds: i64) -> QuotaUsage {
    QuotaUsage {
        percent: used_from_remaining(remaining),
        resets_at: None,
        window_seconds: Some(window_seconds),
    }
}

fn overage_suffix(root: &Value) -> Option<String> {
    let micros = first_f64(
        root,
        &[
            &["overageBalanceMicros"],
            &["overage_balance_micros"],
            &["planStatus", "overageBalanceMicros"],
            &["planStatus", "overage_balance_micros"],
            &["plan_status", "overageBalanceMicros"],
            &["quota", "overageBalanceMicros"],
        ],
    )?;
    let dollars = micros / 1_000_000.0;
    if !dollars.is_finite() {
        return None;
    }
    Some(format!("${dollars:.2} left"))
}

/// Daily remaining % → `five_hour` used %; weekly remaining % → `week`.
pub fn parse_windsurf_user_status(root: &Value) -> WindsurfQuota {
    let email = first_str(
        root,
        &[&["email"], &["user", "email"], &["userStatus", "email"]],
    );
    let plan = first_str(
        root,
        &[
            &["planStatus", "planName"],
            &["plan_status", "plan_name"],
            &["planName"],
            &["plan_name"],
            &["plan"],
            &["userStatus", "planName"],
        ],
    );
    let daily = first_f64(
        root,
        &[
            &["planStatus", "dailyQuotaRemainingPercent"],
            &["plan_status", "daily_quota_remaining_percent"],
            &["dailyQuotaRemainingPercent"],
            &["daily_quota_remaining_percent"],
            &["quota", "dailyQuotaRemainingPercent"],
            &["quota", "daily_quota_remaining_percent"],
            &["userStatus", "dailyQuotaRemainingPercent"],
        ],
    )
    .map(|remaining| quota_from_remaining(remaining, DAY_SECS));
    let week = first_f64(
        root,
        &[
            &["planStatus", "weeklyQuotaRemainingPercent"],
            &["plan_status", "weekly_quota_remaining_percent"],
            &["weeklyQuotaRemainingPercent"],
            &["weekly_quota_remaining_percent"],
            &["quota", "weeklyQuotaRemainingPercent"],
            &["quota", "weekly_quota_remaining_percent"],
            &["userStatus", "weeklyQuotaRemainingPercent"],
        ],
    )
    .map(|remaining| quota_from_remaining(remaining, WEEK_SECS));

    WindsurfQuota {
        email,
        plan,
        five_hour: daily,
        week,
        detail_suffix: overage_suffix(root),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn remaining_percents_become_used() {
        let v = json!({
            "email": "dev@example.com",
            "planStatus": {
                "planName": "Pro",
                "dailyQuotaRemainingPercent": 80.0,
                "weeklyQuotaRemainingPercent": 55.0,
                "overageBalanceMicros": 1_250_000
            }
        });
        let q = parse_windsurf_user_status(&v);
        assert_eq!(q.email.as_deref(), Some("dev@example.com"));
        assert_eq!(q.plan.as_deref(), Some("Pro"));
        assert!((q.five_hour.as_ref().unwrap().percent - 20.0).abs() < 0.001);
        assert_eq!(q.five_hour.as_ref().unwrap().window_seconds, Some(DAY_SECS));
        assert!((q.week.as_ref().unwrap().percent - 45.0).abs() < 0.001);
        assert_eq!(q.week.as_ref().unwrap().window_seconds, Some(WEEK_SECS));
        assert_eq!(q.detail_suffix.as_deref(), Some("$1.25 left"));
    }

    #[test]
    fn accepts_flat_and_quota_nested_paths() {
        let flat = json!({
            "dailyQuotaRemainingPercent": 100,
            "weeklyQuotaRemainingPercent": 0
        });
        let q = parse_windsurf_user_status(&flat);
        assert!((q.five_hour.as_ref().unwrap().percent - 0.0).abs() < 0.001);
        assert!((q.week.as_ref().unwrap().percent - 100.0).abs() < 0.001);

        let nested = json!({
            "quota": {
                "daily_quota_remaining_percent": "40",
                "weekly_quota_remaining_percent": "70"
            }
        });
        let q = parse_windsurf_user_status(&nested);
        assert!((q.five_hour.as_ref().unwrap().percent - 60.0).abs() < 0.001);
        assert!((q.week.as_ref().unwrap().percent - 30.0).abs() < 0.001);
    }

    #[test]
    fn missing_fields_yield_empty_windows() {
        let q = parse_windsurf_user_status(&json!({ "error": "missing" }));
        assert!(q.five_hour.is_none());
        assert!(q.week.is_none());
        assert!(q.detail_suffix.is_none());
    }
}
