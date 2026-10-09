//! Official Moonshot API wallet (USD); distinct from Kimi Code subscription quota.
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MoonshotBalance {
    pub available_usd: Option<f64>,
    pub detail_suffix: Option<String>,
}

/// Only successful documented payloads expose an available USD balance.
pub fn parse_moonshot_balance(root: &Value) -> Option<MoonshotBalance> {
    if !root.get("status")?.as_bool()? || root.get("code")?.as_i64()? != 0 {
        return None;
    }
    let available_usd = root.get("data")?.get("available_balance")?.as_f64()?;
    if !available_usd.is_finite() {
        return None;
    }
    Some(MoonshotBalance {
        available_usd: Some(available_usd),
        detail_suffix: Some(format!("${available_usd:.2} left")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn uses_available_balance_without_double_counting_cash_and_vouchers() {
        let balance = parse_moonshot_balance(&json!({
            "status": true, "code": 0,
            "data": {"available_balance": 49.58894, "voucher_balance": 46.58893, "cash_balance": 3.00001}
        })).unwrap();
        assert_eq!(balance.available_usd, Some(49.58894));
        assert_eq!(balance.detail_suffix.as_deref(), Some("$49.59 left"));
    }

    #[test]
    fn distinguishes_exhausted_wallet_from_error_or_missing_data() {
        assert_eq!(
            parse_moonshot_balance(&json!({"status": true, "code": 0,
            "data": {"available_balance": 0}}))
            .unwrap()
            .available_usd,
            Some(0.0)
        );
        for root in [
            json!({}),
            json!({"status": false, "code": 1, "data": {"available_balance": 12}}),
            json!({"status": true, "code": 0, "data": {"cash_balance": 12}}),
        ] {
            assert!(parse_moonshot_balance(&root).is_none());
        }
    }
}
