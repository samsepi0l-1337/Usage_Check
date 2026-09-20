use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

use crate::models::QuotaUsage;

/// Parsed Factory `POST /api/organization/subscription/usage` (`usedRatio`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FactoryUsage {
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

fn unix_ms(v: Option<f64>) -> Option<DateTime<Utc>> {
    let ms = v.filter(|n| *n > 0.0)?;
    let seconds = if ms.abs() >= 1_000_000_000_000.0 {
        ms / 1000.0
    } else {
        ms
    };
    Utc.timestamp_opt(seconds as i64, 0).single()
}

fn used_ratio_percent(raw: f64) -> Option<f64> {
    if !raw.is_finite() {
        return None;
    }
    let percent = if (0.0..=1.0).contains(&raw) {
        raw * 100.0
    } else {
        raw
    };
    (0.0..=100.0)
        .contains(&percent)
        .then_some(percent.clamp(0.0, 100.0))
}

fn plan_from_allowance(allowance: f64) -> Option<String> {
    if allowance >= 200_000_000.0 {
        Some("Max".into())
    } else if allowance >= 20_000_000.0 {
        Some("Pro".into())
    } else if allowance > 0.0 {
        Some("Basic".into())
    } else {
        None
    }
}

fn window_seconds(start: Option<DateTime<Utc>>, end: Option<DateTime<Utc>>) -> Option<i64> {
    let start = start?;
    let end = end?;
    let secs = end.signed_duration_since(start).num_seconds();
    (secs > 0).then_some(secs)
}

/// `usedRatio` is used % (0–1 ratio, or already 0–100). Tokens-only without
/// a ratio/limit never invent a percent.
pub fn parse_factory_usage(root: &Value) -> FactoryUsage {
    let usage = root.get("usage").unwrap_or(root);
    let standard = usage
        .get("standard")
        .or_else(|| usage.get("Standard"))
        .unwrap_or(usage);

    let start = unix_ms(first_f64(
        usage,
        &[&["startDate"], &["start_date"], &["start"]],
    ));
    let end = unix_ms(first_f64(usage, &[&["endDate"], &["end_date"], &["end"]]));
    let window_seconds = window_seconds(start, end);

    let ratio = first_f64(
        standard,
        &[
            &["usedRatio"],
            &["used_ratio"],
            &["usageRatio"],
            &["usage_ratio"],
        ],
    )
    .and_then(used_ratio_percent);

    let used = first_f64(
        standard,
        &[
            &["orgTotalTokensUsed"],
            &["org_total_tokens_used"],
            &["userTokens"],
            &["user_tokens"],
            &["used"],
        ],
    );
    let limit = first_f64(
        standard,
        &[
            &["totalAllowance"],
            &["total_allowance"],
            &["basicAllowance"],
            &["basic_allowance"],
            &["limit"],
        ],
    )
    .filter(|n| *n > 0.0);

    let percent = ratio.or_else(|| match (used, limit) {
        (Some(used), Some(limit)) => Some(((used / limit) * 100.0).clamp(0.0, 100.0)),
        _ => None,
    });

    let plan = limit.and_then(plan_from_allowance);
    let period = percent.map(|percent| QuotaUsage {
        percent,
        resets_at: end,
        window_seconds,
    });

    FactoryUsage {
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
    fn used_ratio_fraction_becomes_percent() {
        let v = json!({
            "usage": {
                "startDate": 1770623326000i64,
                "endDate": 1772956800000i64,
                "standard": {
                    "orgTotalTokensUsed": 5_000_000,
                    "totalAllowance": 20_000_000,
                    "usedRatio": 0.25
                }
            }
        });
        let q = parse_factory_usage(&v);
        assert!((q.period.as_ref().unwrap().percent - 25.0).abs() < 0.001);
        assert_eq!(q.plan.as_deref(), Some("Pro"));
        assert!(q.period.as_ref().unwrap().resets_at.is_some());
        assert!(q.period.as_ref().unwrap().window_seconds.unwrap() > 0);
    }

    #[test]
    fn used_ratio_already_percent() {
        let v = json!({ "standard": { "usedRatio": 40 } });
        let q = parse_factory_usage(&v);
        assert!((q.period.as_ref().unwrap().percent - 40.0).abs() < 0.001);
    }

    #[test]
    fn tokens_over_allowance_when_ratio_missing() {
        let v = json!({
            "usage": {
                "standard": {
                    "orgTotalTokensUsed": 5_000_000,
                    "totalAllowance": 20_000_000
                }
            }
        });
        let q = parse_factory_usage(&v);
        assert!((q.period.as_ref().unwrap().percent - 25.0).abs() < 0.001);
    }

    #[test]
    fn remaining_only_does_not_invent_percent() {
        let q = parse_factory_usage(&json!({
            "usage": { "standard": { "orgTotalTokensUsed": 100 } }
        }));
        assert!(q.period.is_none());
    }

    #[test]
    fn empty_json_has_no_usage() {
        let q = parse_factory_usage(&json!({}));
        assert!(q.period.is_none());
        assert!(q.plan.is_none());
    }

    #[test]
    fn max_plan_from_large_allowance() {
        let v = json!({
            "usage": { "standard": { "usedRatio": 0, "totalAllowance": 200_000_000 } }
        });
        let q = parse_factory_usage(&v);
        assert_eq!(q.plan.as_deref(), Some("Max"));
        assert_eq!(q.period.as_ref().unwrap().percent, 0.0);
    }
}
