use serde_json::Value;

/// Parsed DeepSeek prepaid wallet. No used-% — remaining balance only.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeepSeekBalance {
    pub currency: Option<String>,
    pub total: Option<f64>,
    pub detail_suffix: Option<String>,
}

fn json_f64(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_i64().map(|n| n as f64))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

fn currency_symbol(code: &str) -> &'static str {
    match code {
        "CNY" | "RMB" => "¥",
        "USD" => "$",
        _ => "",
    }
}

fn format_left(currency: &str, total: f64) -> String {
    let symbol = currency_symbol(currency);
    if symbol.is_empty() {
        format!("{currency} {total:.2} left")
    } else {
        format!("{symbol}{total:.2} left")
    }
}

fn pick_balance_row(infos: &[Value]) -> Option<&Value> {
    infos
        .iter()
        .find(|row| {
            row.get("currency")
                .and_then(Value::as_str)
                .is_some_and(|c| c.eq_ignore_ascii_case("USD"))
        })
        .or_else(|| infos.first())
}

/// Prefers a USD `balance_infos` row when both CNY and USD exist, else first.
/// Never invents used percent from a prepaid wallet.
pub fn parse_deepseek_balance(root: &Value) -> DeepSeekBalance {
    let infos = root
        .get("balance_infos")
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[]);
    let Some(row) = pick_balance_row(infos) else {
        return DeepSeekBalance::default();
    };
    let currency = row
        .get("currency")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let total = row.get("total_balance").and_then(json_f64);
    let detail_suffix = match (currency.as_deref(), total) {
        (Some(code), Some(amount)) if amount.is_finite() => Some(format_left(code, amount)),
        _ => None,
    };
    DeepSeekBalance {
        currency,
        total,
        detail_suffix,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prefers_usd_over_cny() {
        let v = json!({
            "is_available": true,
            "balance_infos": [
                { "currency": "CNY", "total_balance": "110.00", "granted_balance": "10.00", "topped_up_balance": "100.00" },
                { "currency": "USD", "total_balance": "12.34", "granted_balance": "0.00", "topped_up_balance": "12.34" }
            ]
        });
        let b = parse_deepseek_balance(&v);
        assert_eq!(b.currency.as_deref(), Some("USD"));
        assert_eq!(b.total, Some(12.34));
        assert_eq!(b.detail_suffix.as_deref(), Some("$12.34 left"));
    }

    #[test]
    fn cny_only_uses_yen_suffix() {
        let v = json!({
            "is_available": false,
            "balance_infos": [
                { "currency": "CNY", "total_balance": "110.00" }
            ]
        });
        let b = parse_deepseek_balance(&v);
        assert_eq!(b.detail_suffix.as_deref(), Some("¥110.00 left"));
        assert_eq!(b.total, Some(110.0));
    }

    #[test]
    fn zero_total_is_still_a_balance() {
        let v = json!({
            "is_available": false,
            "balance_infos": [
                { "currency": "USD", "total_balance": "0.00" }
            ]
        });
        let b = parse_deepseek_balance(&v);
        assert_eq!(b.total, Some(0.0));
        assert_eq!(b.detail_suffix.as_deref(), Some("$0.00 left"));
    }
}
