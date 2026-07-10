use serde_json::Value;

use crate::models::QuotaUsage;

/// Parsed xAI prepaid credit balance (Management API).
#[derive(Clone, Debug, PartialEq)]
pub struct GrokPrepaid {
    /// Used % since the most recent top-up (0–100), when computable.
    pub period: Option<QuotaUsage>,
    /// Secondary tray label, e.g. `"$23.17 left"` or `"API credits"`.
    pub detail_suffix: Option<String>,
}

fn cents_val(obj: Option<&Value>) -> Option<i64> {
    obj?.get("val")?.as_str()?.parse().ok()
}

fn abs_cents(cents: i64) -> i64 {
    cents.unsigned_abs() as i64
}

/// Computes spend-since-last-top-up used % from ledger `changes`.
pub fn grok_used_percent_from_changes(changes: &[Value]) -> Option<f64> {
    let mut spend_since = 0i64;
    let mut purchase_limit: Option<i64> = None;

    for change in changes.iter().rev() {
        let origin = change.get("changeOrigin")?.as_str()?;
        let amount = cents_val(change.get("amount"))?;
        match origin {
            "SPEND" => spend_since += amount.max(0),
            "PURCHASE" | "AUTO_PURCHASE" => {
                purchase_limit = Some(abs_cents(amount));
                break;
            }
            _ => {}
        }
    }

    let limit = purchase_limit?;
    if limit <= 0 {
        return None;
    }
    Some((spend_since as f64 / limit as f64) * 100.0)
}

/// Parses `GET /v1/billing/teams/{team_id}/prepaid/balance` JSON.
pub fn parse_grok_prepaid_balance(root: &Value) -> GrokPrepaid {
    let remaining_cents = root
        .get("total")
        .and_then(|t| cents_val(Some(t)))
        .map(abs_cents);

    let changes = root
        .get("changes")
        .and_then(|c| c.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[]);

    let percent = grok_used_percent_from_changes(changes);
    let detail_suffix = remaining_cents.map(|c| format!("${:.2} left", c as f64 / 100.0));

    let period = percent.map(|p| QuotaUsage {
        percent: p,
        resets_at: None,
        window_seconds: None,
    });

    GrokPrepaid {
        period,
        detail_suffix: detail_suffix.or_else(|| Some("API credits".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_remaining_balance_and_spend_percent() {
        let v = json!({
            "total": { "val": "-2317" },
            "changes": [
                { "changeOrigin": "PURCHASE", "amount": { "val": "-2500" }, "createTs": "2026-03-23T12:56:21Z" },
                { "changeOrigin": "SPEND", "amount": { "val": "183" }, "createTs": "2026-04-10T21:40:00Z" }
            ]
        });
        let q = parse_grok_prepaid_balance(&v);
        assert!((q.period.as_ref().unwrap().percent - 7.32).abs() < 0.1);
        assert_eq!(q.detail_suffix.as_deref(), Some("$23.17 left"));
    }

    #[test]
    fn total_only_still_formats_suffix() {
        let v = json!({ "total": { "val": "-500" }, "changes": [] });
        let q = parse_grok_prepaid_balance(&v);
        assert!(q.period.is_none());
        assert_eq!(q.detail_suffix.as_deref(), Some("$5.00 left"));
    }
}
