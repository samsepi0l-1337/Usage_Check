use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::QuotaUsage;

const FIVE_HOUR_SECS: i64 = 5 * 60 * 60;
const WEEK_SECS: i64 = 7 * 24 * 60 * 60;
const MONTH_SECS: i64 = 30 * 24 * 60 * 60;

/// Parsed OpenCode Go `/zen/go/v1/usage` windows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpenCodeUsage {
    pub five_hour: Option<QuotaUsage>,
    pub week: Option<QuotaUsage>,
    pub monthly: Option<QuotaUsage>,
    pub plan: Option<String>,
}

fn json_f64(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_i64().map(|n| n as f64))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

fn parse_reset(v: &Value) -> Option<DateTime<Utc>> {
    v.as_str().and_then(|s| {
        DateTime::parse_from_rfc3339(s.trim())
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    })
}

fn window_quota(block: &Value, window_seconds: i64) -> Option<QuotaUsage> {
    let percent = block.get("percent").and_then(json_f64)?;
    if !percent.is_finite() {
        return None;
    }
    let reset = block
        .get("resetsAt")
        .or_else(|| block.get("resets_at"))
        .and_then(parse_reset);
    Some(QuotaUsage {
        percent: percent.clamp(0.0, 100.0),
        resets_at: reset,
        window_seconds: Some(window_seconds),
    })
}

/// `percent` is already used % 0–100. rolling→five_hour, weekly→week,
/// monthly is an optional extra window.
pub fn parse_opencode_usage(root: &Value) -> OpenCodeUsage {
    let usage = root.get("usage").unwrap_or(root);
    let five_hour = usage
        .get("rolling")
        .and_then(|b| window_quota(b, FIVE_HOUR_SECS));
    let week = usage.get("weekly").and_then(|b| window_quota(b, WEEK_SECS));
    let monthly = usage
        .get("monthly")
        .and_then(|b| window_quota(b, MONTH_SECS));
    let plan = if five_hour.is_some() || week.is_some() || monthly.is_some() {
        Some("Go".into())
    } else {
        None
    };
    OpenCodeUsage {
        five_hour,
        week,
        monthly,
        plan,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_rolling_weekly_monthly_percents() {
        let v = json!({
            "usage": {
                "rolling":  { "status": "ok", "percent": 1,  "resetsAt": "2026-09-20T12:00:00Z" },
                "weekly":   { "status": "ok", "percent": 39, "resetsAt": "2026-09-22T00:00:00Z" },
                "monthly":  { "status": "ok", "percent": 19, "resetsAt": "2026-10-01T00:00:00Z" }
            }
        });
        let q = parse_opencode_usage(&v);
        assert_eq!(q.five_hour.as_ref().unwrap().percent, 1.0);
        assert_eq!(q.week.as_ref().unwrap().percent, 39.0);
        assert_eq!(q.monthly.as_ref().unwrap().percent, 19.0);
        assert_eq!(q.plan.as_deref(), Some("Go"));
        assert_eq!(
            q.five_hour
                .as_ref()
                .unwrap()
                .resets_at
                .unwrap()
                .to_rfc3339(),
            "2026-09-20T12:00:00+00:00"
        );
    }

    #[test]
    fn missing_windows_leave_plan_empty() {
        let q = parse_opencode_usage(&json!({ "error": "nope" }));
        assert!(q.five_hour.is_none());
        assert!(q.week.is_none());
        assert!(q.monthly.is_none());
        assert!(q.plan.is_none());
    }
}
