use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::QuotaUsage;

const FIVE_HOUR_SECS: i64 = 5 * 60 * 60;
const WEEK_SECS: i64 = 7 * 24 * 60 * 60;

/// Parsed Kimi Code usage windows from `/coding/v1/usages`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KimiUsage {
    pub five_hour: Option<QuotaUsage>,
    pub week: Option<QuotaUsage>,
}

impl KimiUsage {
    pub fn is_empty(&self) -> bool {
        self.five_hour.is_none() && self.week.is_none()
    }
}

fn json_f64(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_i64().map(|n| n as f64))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

fn parse_reset(v: &Value) -> Option<DateTime<Utc>> {
    if let Some(s) = v.as_str() {
        return DateTime::parse_from_rfc3339(s.trim())
            .ok()
            .map(|dt| dt.with_timezone(&Utc));
    }
    None
}

fn quota_from_ratio(
    used_ratio: f64,
    reset: Option<DateTime<Utc>>,
    window_seconds: i64,
) -> QuotaUsage {
    QuotaUsage {
        percent: (used_ratio * 100.0).clamp(0.0, 100.0),
        resets_at: reset,
        window_seconds: Some(window_seconds),
    }
}

fn used_percent_from_limit(detail: &Value) -> Option<f64> {
    let limit = detail
        .get("limit")
        .and_then(json_f64)
        .filter(|n| *n > 0.0)?;
    if let Some(used) = detail.get("used").and_then(json_f64) {
        return Some((used / limit * 100.0).clamp(0.0, 100.0));
    }
    let remaining = detail.get("remaining").and_then(json_f64)?;
    Some(((limit - remaining) / limit * 100.0).clamp(0.0, 100.0))
}

fn quota_from_limit_block(
    detail: &Value,
    reset: Option<DateTime<Utc>>,
    window_seconds: i64,
) -> Option<QuotaUsage> {
    Some(QuotaUsage {
        percent: used_percent_from_limit(detail)?,
        resets_at: reset,
        window_seconds: Some(window_seconds),
    })
}

fn is_five_hour_window(window: &Value) -> bool {
    let duration = window.get("duration").and_then(json_f64).unwrap_or(0.0);
    let unit = window
        .get("timeUnit")
        .or_else(|| window.get("time_unit"))
        .and_then(Value::as_str)
        .unwrap_or("");
    (duration == 300.0 && unit.contains("MINUTE")) || (duration == 5.0 && unit.contains("HOUR"))
}

fn is_week_window(window: &Value) -> bool {
    let duration = window.get("duration").and_then(json_f64).unwrap_or(0.0);
    let unit = window
        .get("timeUnit")
        .or_else(|| window.get("time_unit"))
        .and_then(Value::as_str)
        .unwrap_or("");
    (duration == 7.0 && unit.contains("DAY")) || (duration == 10080.0 && unit.contains("MINUTE"))
}

fn parse_new_shape(root: &Value) -> Option<KimiUsage> {
    let usages = root.get("usages")?.as_object()?;
    let five = usages.get("limit_5h").and_then(|block| {
        let ratio = block.get("used_ratio").and_then(json_f64)?;
        let reset = block.get("reset_time").and_then(parse_reset);
        Some(quota_from_ratio(ratio, reset, FIVE_HOUR_SECS))
    });
    let week = usages.get("limit_7d").and_then(|block| {
        let ratio = block.get("used_ratio").and_then(json_f64)?;
        let reset = block.get("reset_time").and_then(parse_reset);
        Some(quota_from_ratio(ratio, reset, WEEK_SECS))
    });
    if five.is_none() && week.is_none() {
        return None;
    }
    Some(KimiUsage {
        five_hour: five,
        week,
    })
}

fn parse_old_shape(root: &Value) -> KimiUsage {
    let mut five_hour = None;
    let mut week = None;

    if let Some(limits) = root.get("limits").and_then(Value::as_array) {
        for item in limits {
            let window = item.get("window").unwrap_or(item);
            let detail = item.get("detail").unwrap_or(item);
            let reset = detail
                .get("resetTime")
                .or_else(|| detail.get("reset_time"))
                .or_else(|| item.get("resetTime"))
                .and_then(parse_reset);
            if five_hour.is_none() && is_five_hour_window(window) {
                five_hour = quota_from_limit_block(detail, reset, FIVE_HOUR_SECS);
            } else if week.is_none() && is_week_window(window) {
                week = quota_from_limit_block(detail, reset, WEEK_SECS);
            }
        }
    }

    if week.is_none() {
        if let Some(usage) = root.get("usage") {
            let reset = usage
                .get("resetTime")
                .or_else(|| usage.get("reset_time"))
                .and_then(parse_reset);
            week = quota_from_limit_block(usage, reset, WEEK_SECS);
        }
    }

    KimiUsage { five_hour, week }
}

/// Parses both kimi-code (`usages.limit_5h`/`limit_7d`) and kimi-cli
/// (`usage` + `limits[]`) payload shapes.
pub fn parse_kimi_usages(root: &Value) -> KimiUsage {
    parse_new_shape(root).unwrap_or_else(|| parse_old_shape(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_kimi_code_used_ratio_windows() {
        let v = json!({
            "usages": {
                "limit_5h": { "used_ratio": 0.42, "reset_time": "2026-09-20T12:00:00Z" },
                "limit_7d": { "used_ratio": 0.18, "reset_time": "2026-09-27T00:00:00Z" }
            }
        });
        let q = parse_kimi_usages(&v);
        assert!((q.five_hour.as_ref().unwrap().percent - 42.0).abs() < 0.01);
        assert_eq!(
            q.five_hour
                .as_ref()
                .unwrap()
                .resets_at
                .unwrap()
                .to_rfc3339(),
            "2026-09-20T12:00:00+00:00"
        );
        assert_eq!(
            q.five_hour.as_ref().unwrap().window_seconds,
            Some(FIVE_HOUR_SECS)
        );
        assert!((q.week.as_ref().unwrap().percent - 18.0).abs() < 0.01);
        assert!(!q.is_empty());
    }

    #[test]
    fn parses_kimi_cli_limit_remaining_and_five_hour_window() {
        let v = json!({
            "usage": { "limit": "100", "remaining": "74", "resetTime": "2026-09-27T00:00:00Z" },
            "limits": [{
                "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                "detail": { "limit": "100", "remaining": "85" }
            }]
        });
        let q = parse_kimi_usages(&v);
        assert!((q.five_hour.as_ref().unwrap().percent - 15.0).abs() < 0.01);
        assert!((q.week.as_ref().unwrap().percent - 26.0).abs() < 0.01);
        assert_eq!(
            q.week.as_ref().unwrap().resets_at.unwrap().to_rfc3339(),
            "2026-09-27T00:00:00+00:00"
        );
    }

    #[test]
    fn old_shape_prefers_used_when_present() {
        let v = json!({
            "usage": { "limit": "50", "used": "10", "remaining": "99" }
        });
        let q = parse_kimi_usages(&v);
        assert!((q.week.as_ref().unwrap().percent - 20.0).abs() < 0.01);
        assert!(q.five_hour.is_none());
    }

    #[test]
    fn empty_payload_is_empty() {
        let q = parse_kimi_usages(&json!({ "error": "missing" }));
        assert!(q.is_empty());
    }
}
