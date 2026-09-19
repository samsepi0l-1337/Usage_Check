use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::Value;

use crate::models::QuotaUsage;

#[derive(Debug, Clone, Default)]
pub struct AugmentCredits {
    pub email: Option<String>,
    pub plan: Option<String>,
    pub credits_remaining: Option<f64>,
    pub credits_included: Option<f64>,
    pub renews_at: Option<DateTime<Utc>>,
}

impl AugmentCredits {
    fn usable_included(&self, remaining: f64) -> Option<f64> {
        self.credits_included.filter(|total| {
            total.is_finite() && *total > 0.0 && remaining.is_finite() && *total >= remaining
        })
    }

    /// Used-percent quota only when remaining and included credits are both
    /// present and coherent. A bare remaining balance is never converted.
    pub fn to_quota(&self) -> Option<QuotaUsage> {
        let remaining = self.credits_remaining?;
        let total = self.usable_included(remaining)?;
        let percent = ((total - remaining) / total * 100.0).clamp(0.0, 100.0);
        Some(QuotaUsage {
            percent,
            resets_at: self.renews_at,
            window_seconds: None,
        })
    }

    pub fn detail_suffix(&self) -> Option<String> {
        let remaining = self.credits_remaining?;
        if let Some(total) = self.usable_included(remaining) {
            Some(format!(
                "{}/{} credits",
                format_credits(remaining),
                format_credits(total)
            ))
        } else {
            Some(format!("{} credits remaining", format_credits(remaining)))
        }
    }
}

fn format_credits(credits: f64) -> String {
    if credits.fract() == 0.0 {
        format!("{credits:.0}")
    } else {
        format!("{credits}")
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
            if n.is_finite() {
                return Some(n);
            }
        } else if let Some(s) = current.as_str() {
            if let Ok(f) = s.trim().parse::<f64>() {
                if f.is_finite() {
                    return Some(f);
                }
            }
        }
    }
    None
}

fn first_str(root: &Value, paths: &[&[&str]]) -> Option<String> {
    for path in paths {
        let mut current = root;
        for key in *path {
            current = &current[key];
            if current.is_null() {
                break;
            }
        }
        if let Some(s) = current.as_str().filter(|s| !s.is_empty()) {
            return Some(s.to_string());
        }
    }
    None
}

fn first_datetime(root: &Value, paths: &[&[&str]]) -> Option<DateTime<Utc>> {
    const EPOCH_MILLISECONDS_THRESHOLD: f64 = 100_000_000_000.0;
    const PLAUSIBLE_RESET_SPAN_DAYS: i64 = 10 * 365;

    fn is_plausible(datetime: &DateTime<Utc>) -> bool {
        let now = Utc::now();
        let span = Duration::days(PLAUSIBLE_RESET_SPAN_DAYS);
        *datetime >= now - span && *datetime <= now + span
    }

    for path in paths {
        let mut current = root;
        for key in *path {
            current = &current[key];
            if current.is_null() {
                break;
            }
        }

        if let Some(s) = current.as_str() {
            if let Ok(datetime) = DateTime::parse_from_rfc3339(s.trim()) {
                let datetime = datetime.with_timezone(&Utc);
                if is_plausible(&datetime) {
                    return Some(datetime);
                }
            }
        }
        let seconds = current
            .as_f64()
            .or_else(|| current.as_str().and_then(|s| s.trim().parse::<f64>().ok()));
        if let Some(mut seconds) = seconds.filter(|seconds| seconds.is_finite()) {
            if seconds.abs() >= EPOCH_MILLISECONDS_THRESHOLD {
                seconds /= 1000.0;
            }
            if let Some(datetime) = Utc.timestamp_opt(seconds as i64, 0).single() {
                if is_plausible(&datetime) {
                    return Some(datetime);
                }
            }
        }
    }
    None
}

pub fn parse_augment_account(root: &Value) -> AugmentCredits {
    let remaining = first_f64(
        root,
        &[
            &["credits_remaining"],
            &["remaining_credits"],
            &["creditsRemaining"],
            &["remainingCredits"],
            &["remaining"],
            &["credits"],
            &["credit_balance"],
            &["balance"],
            &["account", "credits_remaining"],
            &["account", "credits"],
            &["billing", "credits_remaining"],
            &["billing", "remaining"],
            &["data", "credits_remaining"],
            &["data", "credits"],
        ],
    );

    let included = first_f64(
        root,
        &[
            &["included_credits"],
            &["credits_included"],
            &["includedCredits"],
            &["creditsIncluded"],
            &["included"],
            &["credits_total"],
            &["total_credits"],
            &["credits_limit"],
            &["credit_limit"],
            &["plan_credits"],
            &["account", "included_credits"],
            &["account", "credits_total"],
            &["billing", "included_credits"],
            &["data", "included_credits"],
            &["data", "credits_total"],
        ],
    )
    .filter(|total| total.is_finite() && *total > 0.0)
    .filter(|total| remaining.is_none_or(|remaining| remaining.is_finite() && *total >= remaining));

    let renews_at = first_datetime(
        root,
        &[
            &["credits_reset_at"],
            &["reset_at"],
            &["renews_at"],
            &["renewal_at"],
            &["next_reset_at"],
            &["period_end"],
            &["current_period_end"],
            &["billing", "current_period_end"],
            &["expires_at"],
        ],
    );

    let email = first_str(
        root,
        &[
            &["email"],
            &["user_email"],
            &["userEmail"],
            &["account", "email"],
            &["user", "email"],
        ],
    );
    let plan = first_str(
        root,
        &[
            &["plan"],
            &["plan_name"],
            &["planName"],
            &["subscription_plan_type"],
            &["subscription"],
            &["account", "plan"],
            &["billing", "plan"],
        ],
    );

    AugmentCredits {
        email,
        plan,
        credits_remaining: remaining,
        credits_included: included,
        renews_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn remaining_only_does_not_invent_percent() {
        let v = json!({
            "email": "dev@example.com",
            "credits_remaining": 774450,
            "plan": "Developer"
        });
        let a = parse_augment_account(&v);
        assert_eq!(a.credits_remaining, Some(774_450.0));
        assert!(a.credits_included.is_none());
        assert!(a.to_quota().is_none());
        assert_eq!(
            a.detail_suffix(),
            Some("774450 credits remaining".to_string())
        );
        assert_eq!(a.email.as_deref(), Some("dev@example.com"));
        assert_eq!(a.plan.as_deref(), Some("Developer"));
    }

    #[test]
    fn remaining_and_included_produce_used_quota() {
        let renews_at = Utc::now() + Duration::days(20);
        let v = json!({
            "credits_remaining": 25000,
            "included_credits": 100000,
            "reset_at": renews_at.to_rfc3339()
        });
        let a = parse_augment_account(&v);
        assert_eq!(a.detail_suffix(), Some("25000/100000 credits".to_string()));
        let quota = a.to_quota().unwrap();
        assert_eq!(quota.percent, 75.0);
        assert_eq!(quota.resets_at.unwrap(), renews_at);
        assert_eq!(quota.window_seconds, None);
    }

    #[test]
    fn nested_billing_fields() {
        let v = json!({
            "account": { "email": "a@b.com" },
            "billing": {
                "remaining": "12.5",
                "included_credits": 50,
                "plan": "Pro"
            }
        });
        let a = parse_augment_account(&v);
        assert_eq!(a.email.as_deref(), Some("a@b.com"));
        assert_eq!(a.plan.as_deref(), Some("Pro"));
        assert_eq!(a.credits_remaining, Some(12.5));
        assert_eq!(a.credits_included, Some(50.0));
        assert_eq!(a.detail_suffix(), Some("12.5/50 credits".to_string()));
    }

    #[test]
    fn unusable_included_totals_do_not_produce_a_quota() {
        for total in [0.0, -1.0, 12.0] {
            let a = parse_augment_account(&json!({
                "credits_remaining": 12.75,
                "included_credits": total
            }));
            assert!(a.credits_included.is_none());
            assert!(a.to_quota().is_none(), "total {total} must be rejected");
            assert_eq!(
                a.detail_suffix(),
                Some("12.75 credits remaining".to_string())
            );
        }
    }

    #[test]
    fn empty_json_has_no_credits() {
        let a = parse_augment_account(&json!({}));
        assert!(a.credits_remaining.is_none());
        assert!(a.to_quota().is_none());
        assert!(a.detail_suffix().is_none());
    }
}
