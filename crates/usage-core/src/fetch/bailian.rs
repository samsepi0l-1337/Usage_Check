use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

use crate::models::QuotaUsage;

const FIVE_HOUR_SECS: i64 = 5 * 60 * 60;
const WEEK_SECS: i64 = 7 * 24 * 60 * 60;

/// Parsed Bailian `bl usage token-plan --output json` windows.
/// `per5HourPercentage` / `per1WeekPercentage` are **used** fractions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BailianQuota {
    pub five_hour: Option<QuotaUsage>,
    pub week: Option<QuotaUsage>,
}

impl BailianQuota {
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

fn used_percent(raw: f64) -> Option<f64> {
    if !raw.is_finite() {
        return None;
    }
    let percent = if raw.abs() <= 1.0 { raw * 100.0 } else { raw };
    (0.0..=100.0)
        .contains(&percent)
        .then_some(percent.clamp(0.0, 100.0))
}

fn parse_reset(ms: f64) -> Option<DateTime<Utc>> {
    if !(ms > 0.0) {
        return None;
    }
    let seconds = if ms.abs() >= 100_000_000_000.0 {
        ms / 1000.0
    } else {
        ms
    };
    Utc.timestamp_opt(seconds as i64, 0).single()
}

fn quota(percent: Option<f64>, reset_ms: Option<f64>, window_seconds: i64) -> Option<QuotaUsage> {
    Some(QuotaUsage {
        percent: used_percent(percent?)?,
        resets_at: reset_ms.and_then(parse_reset),
        window_seconds: Some(window_seconds),
    })
}

/// Reads 5h / weekly used fractions (0–1 or 0–100). Missing windows are
/// independent — one valid window is enough.
pub fn parse_bailian_token_plan(root: &Value) -> BailianQuota {
    let data = root.get("data").unwrap_or(root);
    BailianQuota {
        five_hour: quota(
            first_f64(
                data,
                &[
                    "per5HourPercentage",
                    "per_5_hour_percentage",
                    "fiveHourPercentage",
                    "five_hour_percentage",
                ],
            ),
            first_f64(
                data,
                &[
                    "per5HourResetTime",
                    "per_5_hour_reset_time",
                    "fiveHourResetTime",
                    "five_hour_reset_time",
                ],
            ),
            FIVE_HOUR_SECS,
        ),
        week: quota(
            first_f64(
                data,
                &[
                    "per1WeekPercentage",
                    "per_1_week_percentage",
                    "perWeekPercentage",
                    "weeklyPercentage",
                    "weekPercentage",
                ],
            ),
            first_f64(
                data,
                &[
                    "per1WeekResetTime",
                    "per_1_week_reset_time",
                    "perWeekResetTime",
                    "weeklyResetTime",
                    "weekResetTime",
                ],
            ),
            WEEK_SECS,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fractional_used_ratios_scale_to_percent() {
        let q = parse_bailian_token_plan(&json!({
            "per5HourPercentage": 0.70,
            "per5HourResetTime": 1_787_001_180_000u64,
            "per1WeekPercentage": 0.40,
            "per1WeekResetTime": 1_787_100_000_000u64
        }));
        let five = q.five_hour.expect("5h");
        assert!((five.percent - 70.0).abs() < 0.01);
        assert_eq!(five.window_seconds, Some(FIVE_HOUR_SECS));
        assert!(five.resets_at.is_some());
        let week = q.week.expect("week");
        assert!((week.percent - 40.0).abs() < 0.01);
        assert!(week.resets_at.is_some());
    }

    #[test]
    fn already_percent_values_are_not_rescaled() {
        let q = parse_bailian_token_plan(&json!({
            "per5HourPercentage": 25,
            "per1WeekPercentage": 10
        }));
        assert!((q.five_hour.unwrap().percent - 25.0).abs() < 0.01);
        assert!((q.week.unwrap().percent - 10.0).abs() < 0.01);
    }

    #[test]
    fn one_window_is_enough() {
        let q = parse_bailian_token_plan(&json!({
            "per1WeekPercentage": 0.70,
            "per1WeekResetTime": 1787001180000u64
        }));
        assert!(q.five_hour.is_none());
        assert!((q.week.unwrap().percent - 70.0).abs() < 0.01);
    }

    #[test]
    fn nested_data_and_snake_case() {
        let q = parse_bailian_token_plan(&json!({
            "data": {
                "per_5_hour_percentage": "0.5",
                "per_1_week_percentage": 80
            }
        }));
        assert!((q.five_hour.unwrap().percent - 50.0).abs() < 0.01);
        assert!((q.week.unwrap().percent - 80.0).abs() < 0.01);
    }

    #[test]
    fn out_of_range_is_ignored() {
        let q = parse_bailian_token_plan(&json!({
            "per5HourPercentage": 140,
            "per1WeekPercentage": -0.2
        }));
        assert!(q.is_empty());
    }

    #[test]
    fn empty_json_is_empty() {
        assert!(parse_bailian_token_plan(&json!({})).is_empty());
    }
}
