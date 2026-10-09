//! Official NanoGPT wallet; USD and Nano (XNO) remain distinct currencies.
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NanoGptBalance {
    pub usd: Option<f64>,
    pub nano: Option<f64>,
    pub detail_suffix: Option<String>,
}

fn amount(value: &Value) -> Option<f64> {
    let amount = value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok())?;
    amount.is_finite().then_some(amount)
}

/// Never converts XNO into USD or invents a used percentage from wallet balances.
pub fn parse_nanogpt_balance(root: &Value) -> Option<NanoGptBalance> {
    if root.get("error").is_some() {
        return None;
    }
    let usd = amount(root.get("usd_balance")?)?;
    let nano = amount(root.get("nano_balance")?)?;
    Some(NanoGptBalance {
        usd: Some(usd),
        nano: Some(nano),
        detail_suffix: Some(format!("${usd:.2} / {nano:.4} XNO left")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preserves_documented_string_balances_and_units() {
        let balance = parse_nanogpt_balance(&json!({"usd_balance": "129.46956147",
            "nano_balance": "26.71801147", "nanoDepositAddress": "nano_example"}))
        .unwrap();
        assert_eq!(balance.usd, Some(129.46956147));
        assert_eq!(balance.nano, Some(26.71801147));
        assert_eq!(
            balance.detail_suffix.as_deref(),
            Some("$129.47 / 26.7180 XNO left")
        );
    }

    #[test]
    fn zero_is_valid_but_missing_invalid_and_error_payloads_are_not_balances() {
        assert_eq!(
            parse_nanogpt_balance(&json!({"usd_balance": "0", "nano_balance": "0"}))
                .unwrap()
                .usd,
            Some(0.0)
        );
        for root in [
            json!({}),
            json!({"usd_balance": "NaN", "nano_balance": "0"}),
            json!({"usd_balance": "0", "nano_balance": "Infinity"}),
            json!({"error": 123, "message": "Invalid API key"}),
        ] {
            assert!(parse_nanogpt_balance(&root).is_none());
        }
    }
}
