use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

use crate::models::QuotaUsage;

const FIVE_HOUR_SECS: i64 = 5 * 60 * 60;
const WEEK_SECS: i64 = 7 * 24 * 60 * 60;

/// Parsed Cline balance / ClinePass windows from api.cline.bot.
/// Remaining-only credit never invents a used %.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClineUsage {
    pub email: Option<String>,
    pub plan: Option<String>,
    pub five_hour: Option<QuotaUsage>,
    pub week: Option<QuotaUsage>,
    pub detail_suffix: Option<String>,
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

fn nonempty_str(v: &Value) -> Option<&str> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty())
}

/// Cline wraps payloads as `{ success, data }`.
pub fn unwrap_cline_data(root: &Value) -> &Value {
    root.get("data").unwrap_or(root)
}

fn money_left(amount: f64) -> String {
    format!("${amount:.2} left")
}

fn unix_ts(v: Option<f64>) -> Option<DateTime<Utc>> {
    let n = v.filter(|n| *n > 0.0)?;
    let seconds = if n.abs() >= 1_000_000_000_000.0 {
        n / 1000.0
    } else {
        n
    };
    Utc.timestamp_opt(seconds as i64, 0).single()
}

fn parse_reset(v: &Value) -> Option<DateTime<Utc>> {
    if let Some(s) = nonempty_str(v) {
        return DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.with_timezone(&Utc));
    }
    unix_ts(json_f64(v))
}

fn used_percent(used: f64, limit: f64) -> Option<f64> {
    if limit > 0.0 && limit.is_finite() && used.is_finite() {
        Some((used / limit * 100.0).clamp(0.0, 100.0))
    } else {
        None
    }
}

fn first_f64(obj: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| obj.get(*key).and_then(json_f64))
        .filter(|n| n.is_finite())
}

fn first_str(obj: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| obj.get(*key).and_then(nonempty_str))
        .map(str::to_string)
}

fn window_kind(obj: &Value) -> Option<&'static str> {
    let blob = [
        first_str(obj, &["name", "type", "window", "period", "id", "label"]),
        obj.as_str().map(str::to_string),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
    .to_ascii_lowercase();
    let duration = first_f64(obj, &["duration", "window_seconds", "windowSeconds"]);
    let unit = first_str(obj, &["timeUnit", "time_unit", "unit"])
        .unwrap_or_default()
        .to_ascii_lowercase();
    if blob.contains("5h")
        || blob.contains("5-hour")
        || blob.contains("five_hour")
        || blob.contains("five-hour")
        || blob.contains("limit_5h")
        || (duration == Some(5.0) && unit.contains("hour"))
        || duration == Some(FIVE_HOUR_SECS as f64)
        || (duration == Some(300.0) && unit.contains("minute"))
    {
        return Some("five");
    }
    if blob.contains("week")
        || blob.contains("7d")
        || blob.contains("limit_7d")
        || (duration == Some(7.0) && unit.contains("day"))
        || duration == Some(WEEK_SECS as f64)
    {
        return Some("week");
    }
    None
}

fn quota_from_used_limit(obj: &Value, window_seconds: i64) -> Option<QuotaUsage> {
    let limit = first_f64(
        obj,
        &[
            "limit",
            "quota",
            "allowance",
            "total",
            "max",
            "budget",
            "creditsLimit",
            "credits_limit",
        ],
    )
    .filter(|n| *n > 0.0)?;
    let used = first_f64(
        obj,
        &["used", "usage", "creditsUsed", "credits_used", "consumed"],
    )?;
    let reset = obj
        .get("resets_at")
        .or_else(|| obj.get("resetsAt"))
        .or_else(|| obj.get("reset_time"))
        .or_else(|| obj.get("resetTime"))
        .or_else(|| obj.get("resetAt"))
        .or_else(|| obj.get("end"))
        .and_then(parse_reset);
    Some(QuotaUsage {
        percent: used_percent(used, limit)?,
        resets_at: reset,
        window_seconds: Some(window_seconds),
    })
}

fn quota_from_ratio(obj: &Value, window_seconds: i64) -> Option<QuotaUsage> {
    let raw = first_f64(obj, &["used_ratio", "usedRatio", "usageRatio", "percent"])?;
    let percent = if (0.0..=1.0).contains(&raw) {
        raw * 100.0
    } else {
        raw
    };
    (0.0..=100.0).contains(&percent).then_some(QuotaUsage {
        percent: percent.clamp(0.0, 100.0),
        resets_at: obj
            .get("reset_time")
            .or_else(|| obj.get("resetTime"))
            .or_else(|| obj.get("resets_at"))
            .and_then(parse_reset),
        window_seconds: Some(window_seconds),
    })
}

fn window_quota(obj: &Value, window_seconds: i64) -> Option<QuotaUsage> {
    quota_from_used_limit(obj, window_seconds).or_else(|| quota_from_ratio(obj, window_seconds))
}

fn take_named_window(root: &Value, keys: &[&str], window_seconds: i64) -> Option<QuotaUsage> {
    for key in keys {
        if let Some(obj) = root.get(*key) {
            if let Some(q) = window_quota(obj, window_seconds) {
                return Some(q);
            }
        }
    }
    None
}

fn walk_windows(root: &Value, five: &mut Option<QuotaUsage>, week: &mut Option<QuotaUsage>) {
    if five.is_some() && week.is_some() {
        return;
    }
    match root {
        Value::Object(map) => {
            if let Some(kind) = window_kind(root) {
                let secs = if kind == "five" {
                    FIVE_HOUR_SECS
                } else {
                    WEEK_SECS
                };
                if let Some(q) = window_quota(root, secs) {
                    if kind == "five" {
                        five.get_or_insert(q);
                    } else {
                        week.get_or_insert(q);
                    }
                }
            }
            for (key, value) in map {
                if five.is_none() {
                    let k = key.to_ascii_lowercase();
                    if k.contains("5h") || k.contains("five_hour") || k.contains("five-hour") {
                        if let Some(q) = window_quota(value, FIVE_HOUR_SECS) {
                            *five = Some(q);
                        }
                    }
                }
                if week.is_none() {
                    let k = key.to_ascii_lowercase();
                    if k.contains("week") || k.contains("7d") {
                        if let Some(q) = window_quota(value, WEEK_SECS) {
                            *week = Some(q);
                        }
                    }
                }
                walk_windows(value, five, week);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_windows(item, five, week);
            }
        }
        _ => {}
    }
}

/// Pay-as-you-go remaining credits. Never invents used % from remaining-only.
pub fn parse_cline_balance(root: &Value) -> ClineUsage {
    let data = unwrap_cline_data(root);
    let remaining = first_f64(
        data,
        &["balance", "credits", "credits_remaining", "remaining"],
    )
    .or_else(|| first_f64(root, &["balance"]));
    let email = first_str(data, &["email"]).or_else(|| first_str(root, &["email"]));
    ClineUsage {
        email,
        plan: None,
        five_hour: None,
        week: None,
        detail_suffix: remaining.filter(|n| n.is_finite()).map(money_left),
    }
}

/// ClinePass used+limit windows when the usages payload includes them.
/// Transaction lists without used+limit never invent a percent.
pub fn parse_cline_usages(root: &Value) -> ClineUsage {
    let data = unwrap_cline_data(root);
    let usages = data.get("usages").unwrap_or(data);
    let mut five_hour = take_named_window(
        usages,
        &[
            "limit_5h",
            "five_hour",
            "fiveHour",
            "five_hour_window",
            "rolling_5h",
        ],
        FIVE_HOUR_SECS,
    );
    let mut week = take_named_window(
        usages,
        &["limit_7d", "week", "weekly", "weekly_window"],
        WEEK_SECS,
    );
    walk_windows(data, &mut five_hour, &mut week);
    let plan = first_str(
        data,
        &["plan", "plan_name", "planName", "subscription", "tier"],
    );
    let email = first_str(data, &["email"]);
    ClineUsage {
        email,
        plan,
        five_hour,
        week,
        detail_suffix: None,
    }
}

/// ClinePass windows win; remaining-only balance is a suffix when no windows.
pub fn merge_cline_usage(balance: ClineUsage, usages: ClineUsage) -> ClineUsage {
    let has_windows = usages.five_hour.is_some() || usages.week.is_some();
    ClineUsage {
        email: usages.email.or(balance.email),
        plan: usages.plan.or(balance.plan),
        five_hour: usages.five_hour.or(balance.five_hour),
        week: usages.week.or(balance.week),
        detail_suffix: if has_windows {
            None
        } else {
            usages.detail_suffix.or(balance.detail_suffix)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn remaining_only_balance_is_suffix() {
        let q = parse_cline_balance(&json!({ "balance": 12.5, "userId": "user_1" }));
        assert!(q.five_hour.is_none());
        assert!(q.week.is_none());
        assert_eq!(q.detail_suffix.as_deref(), Some("$12.50 left"));
    }

    #[test]
    fn wrapped_balance_envelope() {
        let q = parse_cline_balance(&json!({
            "success": true,
            "data": { "balance": 0, "userId": "user_1" }
        }));
        assert_eq!(q.detail_suffix.as_deref(), Some("$0.00 left"));
    }

    #[test]
    fn clinepass_used_limit_windows() {
        let q = parse_cline_usages(&json!({
            "usages": {
                "limit_5h": { "used": 25, "limit": 100, "reset_time": "2026-09-20T12:00:00Z" },
                "limit_7d": { "used": 40, "limit": 200 }
            }
        }));
        assert!((q.five_hour.as_ref().unwrap().percent - 25.0).abs() < 0.01);
        assert_eq!(q.five_hour.as_ref().unwrap().window_seconds, Some(18_000));
        assert!(q.five_hour.as_ref().unwrap().resets_at.is_some());
        assert!((q.week.as_ref().unwrap().percent - 20.0).abs() < 0.01);
        assert_eq!(q.week.as_ref().unwrap().window_seconds, Some(604_800));
    }

    #[test]
    fn transaction_list_does_not_invent_percent() {
        let q = parse_cline_usages(&json!({
            "items": [
                { "creditsUsed": 1.5, "costUsd": 1.5, "createdAt": "2026-09-01T00:00:00Z" },
                { "creditsUsed": 2.0, "costUsd": 2.0, "createdAt": "2026-09-02T00:00:00Z" }
            ]
        }));
        assert!(q.five_hour.is_none());
        assert!(q.week.is_none());
        assert!(q.detail_suffix.is_none());
    }

    #[test]
    fn remaining_only_window_does_not_invent_percent() {
        let q = parse_cline_usages(&json!({
            "five_hour": { "remaining": 80, "name": "5h" }
        }));
        assert!(q.five_hour.is_none());
    }

    #[test]
    fn merge_prefers_windows_over_remaining_suffix() {
        let merged = merge_cline_usage(
            parse_cline_balance(&json!({ "balance": 9.0 })),
            parse_cline_usages(&json!({
                "limit_5h": { "used": 1, "limit": 2 }
            })),
        );
        assert!(merged.five_hour.is_some());
        assert!(merged.detail_suffix.is_none());
    }

    #[test]
    fn merge_keeps_remaining_when_no_windows() {
        let merged = merge_cline_usage(
            parse_cline_balance(&json!({ "balance": 3.25 })),
            parse_cline_usages(&json!({ "items": [] })),
        );
        assert_eq!(merged.detail_suffix.as_deref(), Some("$3.25 left"));
        assert!(merged.five_hour.is_none());
    }

    #[test]
    fn used_ratio_fraction_becomes_percent() {
        let q = parse_cline_usages(&json!({
            "windows": [{ "name": "weekly", "usedRatio": 0.4 }]
        }));
        assert!((q.week.as_ref().unwrap().percent - 40.0).abs() < 0.01);
    }
}
