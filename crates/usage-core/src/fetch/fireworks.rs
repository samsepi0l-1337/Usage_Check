use serde_json::Value;

use crate::models::QuotaUsage;

/// Parsed Fireworks `GET /v1/accounts/{id}/billing/summary`.
/// Period spend is a `$ billed` suffix; used % only when spend and a limit
/// both exist.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FireworksBilling {
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

/// protobuf `type.Money`: `units` + `nanos` / 1e9.
fn money_f64(v: &Value) -> Option<f64> {
    if let Some(n) = json_f64(v).filter(|n| n.is_finite()) {
        return Some(n);
    }
    let obj = v.as_object()?;
    let units = obj.get("units").and_then(json_f64).unwrap_or(0.0);
    let nanos = obj.get("nanos").and_then(json_f64).unwrap_or(0.0);
    let amount = units + nanos / 1_000_000_000.0;
    amount.is_finite().then_some(amount)
}

fn sum_line_item_costs(root: &Value) -> Option<f64> {
    let items = root.get("lineItems").and_then(Value::as_array)?;
    let mut total = 0.0;
    let mut any = false;
    for item in items {
        if let Some(cost) = item.get("totalCost").and_then(money_f64) {
            total += cost;
            any = true;
        }
    }
    any.then_some(total)
}

fn spend(root: &Value) -> Option<f64> {
    [
        "totalCost",
        "total_cost",
        "totalSpend",
        "total_spend",
        "spend",
        "billed",
    ]
    .into_iter()
    .find_map(|key| root.get(key).and_then(money_f64))
    .or_else(|| sum_line_item_costs(root))
    .filter(|n| n.is_finite())
}

fn spend_limit(root: &Value) -> Option<f64> {
    [
        "spendLimit",
        "spend_limit",
        "monthlySpendLimit",
        "monthly_spend_limit",
        "limit",
        "budget",
    ]
    .into_iter()
    .find_map(|key| root.get(key).and_then(money_f64))
    .filter(|n| n.is_finite() && *n > 0.0)
}

fn billed_suffix(amount: f64) -> String {
    format!("${amount:.2} billed")
}

/// Sums line-item `totalCost` (or a top-level spend field). Used % requires
/// a positive spend limit in the same JSON — spend alone is never a percent.
pub fn parse_fireworks_billing(root: &Value) -> FireworksBilling {
    let data = root.get("data").unwrap_or(root);
    let spend = spend(data);
    let limit = spend_limit(data);
    let period = match (spend, limit) {
        (Some(spend), Some(limit)) => {
            let percent = (spend / limit * 100.0).clamp(0.0, 100.0);
            Some(QuotaUsage {
                percent,
                resets_at: None,
                window_seconds: None,
            })
        }
        _ => None,
    };
    FireworksBilling {
        period,
        detail_suffix: spend.map(billed_suffix),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sums_line_items_without_inventing_percent() {
        let q = parse_fireworks_billing(&json!({
            "lineItems": [
                { "totalCost": { "currencyCode": "USD", "units": "10", "nanos": 0 } },
                { "totalCost": { "currencyCode": "USD", "units": "2", "nanos": 340000000 } }
            ]
        }));
        assert!(q.period.is_none());
        assert_eq!(q.detail_suffix.as_deref(), Some("$12.34 billed"));
    }

    #[test]
    fn spend_and_limit_produce_used_percent() {
        let q = parse_fireworks_billing(&json!({
            "lineItems": [
                { "totalCost": { "currencyCode": "USD", "units": "25", "nanos": 0 } }
            ],
            "spendLimit": { "currencyCode": "USD", "units": "100", "nanos": 0 }
        }));
        assert!((q.period.as_ref().unwrap().percent - 25.0).abs() < 0.01);
        assert_eq!(q.detail_suffix.as_deref(), Some("$25.00 billed"));
    }

    #[test]
    fn numeric_spend_without_limit_is_suffix_only() {
        let q = parse_fireworks_billing(&json!({ "spend": 0 }));
        assert!(q.period.is_none());
        assert_eq!(q.detail_suffix.as_deref(), Some("$0.00 billed"));
    }

    #[test]
    fn zero_or_missing_limit_does_not_invent_percent() {
        for limit in [json!(0), json!(null)] {
            let q = parse_fireworks_billing(&json!({
                "spend": 12.34,
                "limit": limit
            }));
            assert!(q.period.is_none(), "limit {limit} must not yield %");
            assert_eq!(q.detail_suffix.as_deref(), Some("$12.34 billed"));
        }
    }

    #[test]
    fn empty_json_has_no_billing() {
        let q = parse_fireworks_billing(&json!({ "lineItems": [] }));
        assert!(q.period.is_none());
        assert!(q.detail_suffix.is_none());
    }
}
