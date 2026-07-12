use serde_json::Value;
use crate::models::QuotaUsage;

#[derive(Debug, Clone)]
pub struct HiggsfieldCredits {
    pub email: Option<String>,
    pub plan: Option<String>,
    pub credits_remaining: Option<f64>,
}

impl HiggsfieldCredits {
    pub fn to_quota(&self) -> Option<QuotaUsage> {
        // Higgsfield exposes only a raw balance with no window/percent model
        // Do not fabricate usage percentages
        None
    }

    pub fn detail_suffix(&self) -> Option<String> {
        self.credits_remaining.map(|credits| {
            // Format f64 smartly: 100.0 → "100", 12.75 → "12.75"
            let formatted = if credits.fract() == 0.0 {
                format!("{:.0}", credits)
            } else {
                format!("{}", credits)
            };
            format!("{} credits remaining", formatted)
        })
    }
}

fn first_f64(root: &Value, paths: &[&[&str]]) -> Option<f64> {
    for path in paths {
        let mut current = root;
        for key in *path {
            current = &current[key];
            if current.is_null() {
                break;
            }
        }

        if let Some(n) = current.as_f64() {
            return Some(n);
        } else if let Some(s) = current.as_str() {
            if let Ok(f) = s.trim().parse::<f64>() {
                return Some(f);
            }
        }
    }
    None
}

pub fn parse_higgsfield_account(root: &Value) -> HiggsfieldCredits {
    let remaining = first_f64(
        root,
        &[
            &["credits"],
            &["credit_balance"],
            &["balance", "credits"],
            &["account", "credits"],
            &["data", "credits"],
        ],
    );

    let email = root
        .get("email")
        .or_else(|| root.get("user_email"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // Parse plan from subscription_plan_type first, then fallback to plan/subscription
    let plan = root
        .get("subscription_plan_type")
        .or_else(|| root.get("plan"))
        .or_else(|| root.get("subscription"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    HiggsfieldCredits {
        email,
        plan,
        credits_remaining: remaining,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_creator_account_f64() {
        let v = json!({
            "email": "person@example.com",
            "credits": 12.75,
            "subscription_plan_type": "Creator"
        });
        let h = parse_higgsfield_account(&v);
        assert_eq!(h.credits_remaining, Some(12.75));
        assert_eq!(h.plan, Some("Creator".to_string()));
        assert_eq!(h.detail_suffix(), Some("12.75 credits remaining".to_string()));
        assert!(h.to_quota().is_none());
    }

    #[test]
    fn parses_numeric_string_credits() {
        let v = json!({ "credits": "12.75" });
        let h = parse_higgsfield_account(&v);
        assert_eq!(h.credits_remaining, Some(12.75));
    }

    #[test]
    fn nonnumeric_credits_is_none() {
        let v1 = json!({ "credits": "abc" });
        let h1 = parse_higgsfield_account(&v1);
        assert!(h1.credits_remaining.is_none());

        let v2 = json!({ "credits": serde_json::Value::Null });
        let h2 = parse_higgsfield_account(&v2);
        assert!(h2.credits_remaining.is_none());
    }

    #[test]
    fn missing_or_empty_email_is_none() {
        let v1 = json!({});
        let h1 = parse_higgsfield_account(&v1);
        assert!(h1.email.is_none());

        let v2 = json!({ "email": "" });
        let h2 = parse_higgsfield_account(&v2);
        assert!(h2.email.is_none());
    }

    #[test]
    fn whole_number_detail_has_no_trailing_zero() {
        let v = json!({ "credits": 100 });
        let h = parse_higgsfield_account(&v);
        assert_eq!(h.detail_suffix(), Some("100 credits remaining".to_string()));
    }
}
