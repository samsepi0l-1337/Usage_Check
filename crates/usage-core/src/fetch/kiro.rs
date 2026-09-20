use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

use crate::models::QuotaUsage;

const MONTH_SECS: i64 = 30 * 24 * 60 * 60;

/// Parsed Kiro `getUsageLimits` CREDIT breakdown (used/limit → used %).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KiroQuota {
    pub plan: Option<String>,
    pub period: Option<QuotaUsage>,
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

fn nonempty_str(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
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

fn breakdown_list(root: &Value) -> &[Value] {
    for key in [
        "usageBreakdownList",
        "usage_breakdown_list",
        "usageBreakdowns",
        "usage_breakdowns",
    ] {
        if let Some(list) = root.get(key).and_then(Value::as_array) {
            return list;
        }
    }
    if let Some(nested) = root.get("usageState").or_else(|| root.get("usage_state")) {
        return breakdown_list(nested);
    }
    &[]
}

fn is_credit(entry: &Value) -> bool {
    for key in [
        "type",
        "resourceType",
        "resource_type",
        "displayName",
        "display_name",
    ] {
        if let Some(s) = entry.get(key).and_then(Value::as_str) {
            if s.eq_ignore_ascii_case("credit") || s.eq_ignore_ascii_case("credits") {
                return true;
            }
        }
    }
    false
}

fn entry_used_limit(entry: &Value) -> Option<(f64, f64)> {
    let used = entry
        .get("currentUsage")
        .or_else(|| entry.get("current_usage"))
        .or_else(|| entry.get("used"))
        .and_then(json_f64)
        .filter(|n| n.is_finite())?;
    let limit = entry
        .get("usageLimit")
        .or_else(|| entry.get("usage_limit"))
        .or_else(|| entry.get("limit"))
        .and_then(json_f64)
        .filter(|n| n.is_finite() && *n > 0.0)?;
    Some((used, limit))
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

/// Maps CREDIT `currentUsage`/`usageLimit` only. Remaining-only rows are ignored.
pub fn parse_kiro_usage_limits(root: &Value) -> KiroQuota {
    let data = root
        .get("result")
        .or_else(|| root.get("data"))
        .unwrap_or(root);
    let plan = first_str(
        data,
        &[
            &["subscriptionInfo", "subscriptionTitle"],
            &["subscription_info", "subscription_title"],
            &["subscriptionInfo", "type"],
            &["plan"],
        ],
    );
    let resets_at = data
        .get("nextDateReset")
        .or_else(|| data.get("next_date_reset"))
        .and_then(parse_reset);
    let mut used_total = 0.0;
    let mut limit_total = 0.0;
    let mut found_credit = false;
    for entry in breakdown_list(data) {
        if !is_credit(entry) {
            continue;
        }
        if let Some((used, limit)) = entry_used_limit(entry) {
            used_total += used;
            limit_total += limit;
            found_credit = true;
        }
    }
    let credit_reset = breakdown_list(data).iter().find_map(|entry| {
        if !is_credit(entry) {
            return None;
        }
        entry
            .get("resetDate")
            .or_else(|| entry.get("reset_date"))
            .and_then(parse_reset)
    });
    let period = if found_credit && limit_total > 0.0 {
        Some(QuotaUsage {
            percent: ((used_total / limit_total) * 100.0).clamp(0.0, 100.0),
            resets_at: credit_reset.or(resets_at),
            window_seconds: Some(MONTH_SECS),
        })
    } else {
        None
    };
    KiroQuota {
        plan,
        period,
        detail_suffix: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn credit_used_over_limit_becomes_percent() {
        let v = json!({
            "subscriptionInfo": { "subscriptionTitle": "Kiro Pro" },
            "usageBreakdownList": [{
                "resourceType": "CREDIT",
                "currentUsage": 10,
                "usageLimit": 50,
                "resetDate": "2026-05-01T00:00:00.000Z"
            }]
        });
        let q = parse_kiro_usage_limits(&v);
        assert_eq!(q.plan.as_deref(), Some("Kiro Pro"));
        assert!((q.period.as_ref().unwrap().percent - 20.0).abs() < 0.001);
        assert!(q.period.as_ref().unwrap().resets_at.is_some());
    }

    #[test]
    fn usage_state_cache_shape() {
        let v = json!({
            "usageState": {
                "usageBreakdowns": [{
                    "type": "CREDIT",
                    "currentUsage": 0,
                    "usageLimit": 50
                }]
            }
        });
        let q = parse_kiro_usage_limits(&v);
        assert_eq!(q.period.as_ref().unwrap().percent, 0.0);
    }

    #[test]
    fn ignores_non_credit_rows() {
        let v = json!({
            "usageBreakdownList": [{
                "type": "AGENTIC_REQUEST",
                "currentUsage": 99,
                "usageLimit": 100
            }]
        });
        let q = parse_kiro_usage_limits(&v);
        assert!(q.period.is_none());
    }

    #[test]
    fn usage_without_limit_does_not_invent_percent() {
        let v = json!({
            "usageBreakdowns": [{
                "type": "CREDIT",
                "currentUsage": 12
            }]
        });
        let q = parse_kiro_usage_limits(&v);
        assert!(q.period.is_none());
    }

    #[test]
    fn empty_json_has_no_quota() {
        let q = parse_kiro_usage_limits(&json!({}));
        assert!(q.period.is_none());
        assert!(q.plan.is_none());
    }

    #[test]
    fn fully_used_credit_is_100() {
        let v = json!({
            "usageBreakdownList": [{
                "type": "Credits",
                "used": 50,
                "limit": 50
            }]
        });
        let q = parse_kiro_usage_limits(&v);
        assert_eq!(q.period.as_ref().unwrap().percent, 100.0);
    }
}
