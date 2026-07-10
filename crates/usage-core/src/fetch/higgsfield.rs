use serde_json::Value;

use crate::models::QuotaUsage;

/// Parsed Higgsfield account credits (CLI `account --json` shape).
#[derive(Clone, Debug, PartialEq)]
pub struct HiggsfieldCredits {
    pub email: Option<String>,
    pub plan: Option<String>,
    pub credits_remaining: Option<i64>,
    pub credits_total: Option<i64>,
}

impl HiggsfieldCredits {
    /// Used % when both total and remaining credits are known.
    pub fn used_percent(&self) -> Option<f64> {
        let total = self.credits_total?;
        let remaining = self.credits_remaining?;
        if total <= 0 {
            return None;
        }
        let used = (total - remaining).max(0);
        Some((used as f64 / total as f64) * 100.0)
    }

    pub fn detail_suffix(&self) -> Option<String> {
        self.credits_remaining
            .map(|n| format!("{n} credits left"))
    }

    pub fn to_quota(&self) -> Option<QuotaUsage> {
        let percent = self.used_percent()?;
        Some(QuotaUsage {
            percent,
            resets_at: None,
            window_seconds: None,
        })
    }
}

fn nested_i64(v: &Value, keys: &[&str]) -> Option<i64> {
    let mut cur = v;
    for key in keys {
        cur = cur.get(*key)?;
    }
    cur.as_i64()
        .or_else(|| cur.as_f64().map(|f| f as i64))
        .or_else(|| cur.as_str().and_then(|s| s.parse().ok()))
}

fn first_i64(v: &Value, paths: &[&[&str]]) -> Option<i64> {
    paths.iter().find_map(|path| nested_i64(v, path))
}

/// Parses `higgsfield account --json` (or similar) into credit balances.
pub fn parse_higgsfield_account(root: &Value) -> HiggsfieldCredits {
    let remaining = first_i64(
        root,
        &[
            &["credits"],
            &["credit_balance"],
            &["balance", "credits"],
            &["account", "credits"],
            &["data", "credits"],
        ],
    );
    let total = first_i64(
        root,
        &[
            &["credits_total"],
            &["total_credits"],
            &["balance", "total"],
            &["account", "credits_total"],
        ],
    );

    let email = root
        .get("email")
        .or_else(|| root.get("user_email"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let plan = root
        .get("plan")
        .or_else(|| root.get("subscription"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    HiggsfieldCredits {
        email,
        plan,
        credits_remaining: remaining,
        credits_total: total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_flat_credits_fields() {
        let v = json!({
            "email": "user@example.com",
            "credits": 809,
            "credits_total": 1000,
            "plan": "pro"
        });
        let h = parse_higgsfield_account(&v);
        assert_eq!(h.credits_remaining, Some(809));
        assert_eq!(h.credits_total, Some(1000));
        assert!((h.used_percent().unwrap() - 19.1).abs() < 0.2);
        assert_eq!(h.detail_suffix().as_deref(), Some("809 credits left"));
    }

    #[test]
    fn parses_nested_balance() {
        let v = json!({ "balance": { "credits": 42 } });
        let h = parse_higgsfield_account(&v);
        assert_eq!(h.credits_remaining, Some(42));
        assert!(h.used_percent().is_none());
    }
}
