use serde_json::Value;

/// Parsed Novita `GET /openapi/v1/billing/balance/detail`.
/// `availableBalance` is 1/10000 USD. Remaining dollars only — never used %.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NovitaBalance {
    pub available_usd: Option<f64>,
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

const NOVITA_BALANCE_UNITS_PER_USD: f64 = 10_000.0;

fn dollars_suffix(usd: f64) -> String {
    format!("${usd:.2} left")
}

/// Converts `availableBalance` (1/10000 USD) to a `$X.XX left` suffix.
/// `creditLimit` is a credit line, not a usage cap — it is ignored for %.
pub fn parse_novita_balance(root: &Value) -> NovitaBalance {
    let data = root.get("data").unwrap_or(root);
    let Some(units) = data
        .get("availableBalance")
        .or_else(|| data.get("available_balance"))
        .and_then(json_f64)
        .filter(|n| n.is_finite())
    else {
        return NovitaBalance::default();
    };
    let usd = units / NOVITA_BALANCE_UNITS_PER_USD;
    if !usd.is_finite() {
        return NovitaBalance::default();
    }
    NovitaBalance {
        available_usd: Some(usd),
        detail_suffix: Some(dollars_suffix(usd)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn converts_ten_thousandths_to_dollars_without_percent() {
        let b = parse_novita_balance(&json!({
            "availableBalance": "1000000",
            "creditLimit": "200000"
        }));
        assert!((b.available_usd.unwrap() - 100.0).abs() < 0.0001);
        assert_eq!(b.detail_suffix.as_deref(), Some("$100.00 left"));
    }

    #[test]
    fn one_dollar_and_zero_are_valid_balances() {
        let one = parse_novita_balance(&json!({ "availableBalance": 10000 }));
        assert_eq!(one.detail_suffix.as_deref(), Some("$1.00 left"));
        let zero = parse_novita_balance(&json!({ "availableBalance": "0" }));
        assert_eq!(zero.available_usd, Some(0.0));
        assert_eq!(zero.detail_suffix.as_deref(), Some("$0.00 left"));
    }

    #[test]
    fn nested_data_object() {
        let b = parse_novita_balance(&json!({
            "data": { "available_balance": "2500" }
        }));
        assert_eq!(b.detail_suffix.as_deref(), Some("$0.25 left"));
    }

    #[test]
    fn missing_available_balance_is_empty() {
        let b = parse_novita_balance(&json!({ "cashBalance": "800000" }));
        assert!(b.available_usd.is_none());
        assert!(b.detail_suffix.is_none());
    }
}
