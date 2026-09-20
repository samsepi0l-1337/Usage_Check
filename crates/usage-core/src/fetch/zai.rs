use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

use crate::models::QuotaUsage;

const FIVE_HOUR_SECS: i64 = 5 * 60 * 60;
const WEEK_SECS: i64 = 7 * 24 * 60 * 60;

/// Parsed Z.AI `GET /api/monitor/usage/quota/limit` coding-plan windows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ZaiQuota {
    pub plan: Option<String>,
    pub five_hour: Option<QuotaUsage>,
    pub week: Option<QuotaUsage>,
}

impl ZaiQuota {
    pub fn is_empty(&self) -> bool {
        self.five_hour.is_none() && self.week.is_none()
    }
}

fn json_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Null => None,
        other => other
            .as_f64()
            .or_else(|| other.as_i64().map(|n| n as f64))
            .or_else(|| other.as_str().and_then(|s| s.trim().parse().ok())),
    }
}

fn first_f64(root: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| root.get(*key).and_then(json_f64))
        .filter(|n| n.is_finite())
}

fn first_str(root: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| root.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn used_percent_value(raw: f64) -> Option<f64> {
    if !raw.is_finite() {
        return None;
    }
    let percent = if raw.abs() <= 1.0 { raw * 100.0 } else { raw };
    (0.0..=100.0)
        .contains(&percent)
        .then_some(percent.clamp(0.0, 100.0))
}

fn remaining_to_used(remaining: f64) -> Option<f64> {
    let remaining = if remaining.abs() <= 1.0 {
        remaining * 100.0
    } else {
        remaining
    };
    if (0.0..=100.0).contains(&remaining) {
        Some((100.0 - remaining).clamp(0.0, 100.0))
    } else {
        None
    }
}

fn parse_reset(v: &Value) -> Option<DateTime<Utc>> {
    if let Some(s) = v.as_str() {
        return DateTime::parse_from_rfc3339(s.trim())
            .ok()
            .map(|dt| dt.with_timezone(&Utc));
    }
    let ms = json_f64(v).filter(|n| *n > 0.0)?;
    let seconds = if ms.abs() >= 100_000_000_000.0 {
        ms / 1000.0
    } else {
        ms
    };
    Utc.timestamp_opt(seconds as i64, 0).single()
}

fn first_reset(root: &Value) -> Option<DateTime<Utc>> {
    [
        "nextResetTime",
        "next_reset_time",
        "resetsAt",
        "resets_at",
        "resetTime",
        "reset_time",
    ]
    .into_iter()
    .find_map(|key| root.get(key).and_then(parse_reset))
}

fn used_percent_from_window(row: &Value) -> Option<f64> {
    if let Some(percent) = first_f64(
        row,
        &[
            "percentage",
            "percent",
            "percentUsed",
            "percent_used",
            "usedPercent",
            "used_percent",
        ],
    )
    .and_then(used_percent_value)
    {
        return Some(percent);
    }
    if let Some(remaining) = first_f64(
        row,
        &[
            "remainingPercent",
            "remaining_percent",
            "remainingPercentage",
            "percentRemaining",
        ],
    )
    .and_then(remaining_to_used)
    {
        return Some(remaining);
    }
    let used = first_f64(row, &["used", "currentValue", "current_value"]);
    let limit = first_f64(row, &["limit", "total", "usage"]).filter(|n| *n > 0.0);
    match (used, limit) {
        (Some(used), Some(limit)) => Some((used / limit * 100.0).clamp(0.0, 100.0)),
        (None, Some(limit)) => first_f64(row, &["remaining", "remain"])
            .map(|remaining| ((limit - remaining) / limit * 100.0).clamp(0.0, 100.0)),
        _ => None,
    }
}

fn quota_from_window(row: &Value, window_seconds: i64) -> Option<QuotaUsage> {
    Some(QuotaUsage {
        percent: used_percent_from_window(row)?,
        resets_at: first_reset(row),
        window_seconds: Some(window_seconds),
    })
}

fn unit_number(row: &Value) -> (Option<u64>, Option<u64>) {
    let unit = first_f64(row, &["unit"]).map(|n| n as u64);
    let number = first_f64(row, &["number", "num"]).map(|n| n as u64);
    (unit, number)
}

fn kind(row: &Value) -> String {
    first_str(row, &["type", "kind", "name", "window", "period"])
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn is_five_hour(row: &Value) -> bool {
    let (unit, number) = unit_number(row);
    if unit == Some(3) && number == Some(5) {
        return true;
    }
    let k = kind(row);
    k.contains("5h")
        || k.contains("five")
        || k.contains("5-hour")
        || k.contains("5_hour")
        || (k.contains("hour") && !k.contains("24"))
}

fn is_week(row: &Value) -> bool {
    let (unit, number) = unit_number(row);
    if unit == Some(6) && (number == Some(1) || number.is_none()) {
        return true;
    }
    let k = kind(row);
    k.contains("week") || k.contains("weekly") || k.contains("7d")
}

fn data_root(root: &Value) -> &Value {
    root.get("data").unwrap_or(root)
}

fn limits_array(data: &Value) -> Option<&Vec<Value>> {
    data.get("limits")
        .or_else(|| data.get("quota"))
        .or_else(|| data.get("windows"))
        .and_then(Value::as_array)
}

/// Reads 5h / weekly used % from quota windows. `percentage` is used %
/// (0–1 or 0–100); remaining percents and used/limit are also accepted.
pub fn parse_zai_quota(root: &Value) -> ZaiQuota {
    let data = data_root(root);
    let plan = first_str(data, &["level", "plan", "planName", "plan_name", "tier"]);
    let mut five_hour = None;
    let mut week = None;

    if let Some(rows) = limits_array(data) {
        for row in rows {
            if five_hour.is_none() && is_five_hour(row) {
                five_hour = quota_from_window(row, FIVE_HOUR_SECS);
            } else if week.is_none() && is_week(row) {
                week = quota_from_window(row, WEEK_SECS);
            }
        }
    }

    if five_hour.is_none() {
        if let Some(row) = data
            .get("fiveHour")
            .or_else(|| data.get("five_hour"))
            .or_else(|| data.get("hourly"))
        {
            five_hour = quota_from_window(row, FIVE_HOUR_SECS);
        }
    }
    if week.is_none() {
        if let Some(row) = data.get("weekly").or_else(|| data.get("week")) {
            week = quota_from_window(row, WEEK_SECS);
        }
    }

    ZaiQuota {
        plan,
        five_hour,
        week,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tokens_limit_percentage_is_used_percent() {
        let q = parse_zai_quota(&json!({
            "data": {
                "level": "PRO",
                "limits": [
                    {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":18.5,"total":6000000,"nextResetTime":1735000000000u64},
                    {"type":"TOKENS_LIMIT","unit":6,"number":1,"percentage":47.2,"total":80000000,"nextResetTime":1735500000000u64}
                ]
            }
        }));
        assert_eq!(q.plan.as_deref(), Some("PRO"));
        let five = q.five_hour.expect("5h");
        assert!((five.percent - 18.5).abs() < 0.01);
        assert_eq!(five.window_seconds, Some(FIVE_HOUR_SECS));
        assert!(five.resets_at.is_some());
        let week = q.week.expect("week");
        assert!((week.percent - 47.2).abs() < 0.01);
        assert_eq!(week.window_seconds, Some(WEEK_SECS));
    }

    #[test]
    fn remaining_percent_converts_to_used() {
        let q = parse_zai_quota(&json!({
            "limits": [
                {"type":"five_hour","remainingPercent":40},
                {"type":"weekly","remaining_percent":0.75}
            ]
        }));
        assert!((q.five_hour.unwrap().percent - 60.0).abs() < 0.01);
        assert!((q.week.unwrap().percent - 25.0).abs() < 0.01);
    }

    #[test]
    fn used_over_limit_windows() {
        let q = parse_zai_quota(&json!({
            "fiveHour": {"used": 20, "limit": 80, "nextResetTime": "2026-09-20T12:00:00Z"},
            "weekly": {"used": 100, "limit": 400}
        }));
        let five = q.five_hour.expect("5h");
        assert!((five.percent - 25.0).abs() < 0.01);
        assert!(five.resets_at.is_some());
        assert!((q.week.unwrap().percent - 25.0).abs() < 0.01);
    }

    #[test]
    fn ratio_percentage_is_scaled() {
        let q = parse_zai_quota(&json!({
            "limits": [
                {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":0.4}
            ]
        }));
        assert!((q.five_hour.unwrap().percent - 40.0).abs() < 0.01);
    }

    #[test]
    fn unknown_windows_are_ignored() {
        let q = parse_zai_quota(&json!({
            "data": {"limits": [{"type":"TIME_LIMIT","percentage":4.0}]}
        }));
        assert!(q.is_empty());
    }

    #[test]
    fn empty_json_is_empty() {
        assert!(parse_zai_quota(&json!({})).is_empty());
    }
}
