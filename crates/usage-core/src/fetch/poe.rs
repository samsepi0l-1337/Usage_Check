use serde_json::Value;

use crate::models::QuotaUsage;

/// Parsed Poe `GET /usage/current_balance`. Remaining points only unless a
/// limit/total is present — never invent used % from a bare balance.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PoeBalance {
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

fn first_f64(root: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| root.get(*key).and_then(json_f64))
        .filter(|n| n.is_finite())
}

fn points_suffix(points: f64) -> String {
    if points.fract() == 0.0 {
        format!("{:.0} points left", points)
    } else {
        format!("{points} points left")
    }
}

/// Reads `current_point_balance`. Used % only when a positive limit/total is
/// in the same payload; a remaining-only wallet stays suffix-only.
pub fn parse_poe_balance(root: &Value) -> PoeBalance {
    let data = root.get("data").unwrap_or(root);
    let remaining = first_f64(
        data,
        &[
            "current_point_balance",
            "currentPointBalance",
            "point_balance",
            "points",
        ],
    );
    let limit = first_f64(
        data,
        &[
            "point_limit",
            "pointLimit",
            "total_points",
            "totalPoints",
            "plan_points",
            "planPoints",
            "limit",
        ],
    )
    .filter(|total| *total > 0.0);

    let period = match (remaining, limit) {
        (Some(remaining), Some(total)) if remaining.is_finite() => {
            let percent = ((total - remaining) / total * 100.0).clamp(0.0, 100.0);
            Some(QuotaUsage {
                percent,
                resets_at: None,
                window_seconds: None,
            })
        }
        _ => None,
    };

    PoeBalance {
        period,
        detail_suffix: remaining.map(points_suffix),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn remaining_only_does_not_invent_percent() {
        let q = parse_poe_balance(&json!({ "current_point_balance": 1500 }));
        assert!(q.period.is_none());
        assert_eq!(q.detail_suffix.as_deref(), Some("1500 points left"));
    }

    #[test]
    fn remaining_and_limit_produce_used_percent() {
        let q = parse_poe_balance(&json!({
            "current_point_balance": 250,
            "point_limit": 1000
        }));
        assert!((q.period.as_ref().unwrap().percent - 75.0).abs() < 0.01);
        assert_eq!(q.detail_suffix.as_deref(), Some("250 points left"));
    }

    #[test]
    fn zero_balance_is_still_a_balance() {
        let q = parse_poe_balance(&json!({ "current_point_balance": 0 }));
        assert!(q.period.is_none());
        assert_eq!(q.detail_suffix.as_deref(), Some("0 points left"));
    }

    #[test]
    fn nested_data_and_string_points() {
        let q = parse_poe_balance(&json!({
            "data": { "current_point_balance": "12.5" }
        }));
        assert_eq!(q.detail_suffix.as_deref(), Some("12.5 points left"));
    }

    #[test]
    fn empty_json_has_no_balance() {
        let q = parse_poe_balance(&json!({}));
        assert!(q.period.is_none());
        assert!(q.detail_suffix.is_none());
    }
}
