use crate::models::QuotaUsage;
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct HiggsfieldCredits {
    pub email: Option<String>,
    pub plan: Option<String>,
    pub credits_remaining: Option<f64>,
    pub credits_total: Option<f64>,
    pub renews_at: Option<DateTime<Utc>>,
}

impl HiggsfieldCredits {
    fn usable_total(&self, remaining: f64) -> Option<f64> {
        self.credits_total.filter(|total| {
            total.is_finite() && *total > 0.0 && remaining.is_finite() && *total >= remaining
        })
    }

    /// Returns a real used-percent quota only when the CLI supplies both a
    /// remaining balance and a coherent total. A bare balance remains
    /// intentionally unconverted: inventing its percentage would mislead
    /// every renderer that treats `QuotaUsage::percent` as used percent.
    pub fn to_quota(&self) -> Option<QuotaUsage> {
        let remaining = self.credits_remaining?;
        let total = self.usable_total(remaining)?;
        let percent = ((total - remaining) / total * 100.0).clamp(0.0, 100.0);
        Some(QuotaUsage {
            percent,
            resets_at: self.renews_at,
            window_seconds: None,
        })
    }

    pub fn detail_suffix(&self) -> Option<String> {
        let remaining = self.credits_remaining?;
        if let Some(total) = self.usable_total(remaining) {
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
    // Format f64 smartly: 100.0 → "100", 12.75 → "12.75".
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
            return Some(n);
        } else if let Some(s) = current.as_str() {
            if let Ok(f) = s.trim().parse::<f64>() {
                return Some(f);
            }
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

    let total = first_f64(
        root,
        &[
            &["credits_total"],
            &["total_credits"],
            &["credits_limit"],
            &["credit_limit"],
            &["plan_credits"],
            &["monthly_credits"],
            &["quota"],
            &["balance", "total"],
            &["account", "credits_total"],
            &["data", "credits_total"],
            &["subscription", "credits_total"],
        ],
    )
    .filter(|total| total.is_finite() && *total > 0.0)
    .filter(|total| {
        remaining.is_none_or(|remaining| remaining.is_finite() && *total >= remaining)
    });

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
            &["subscription", "current_period_end"],
            &["expires_at"],
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
        credits_total: total,
        renews_at,
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

    #[test]
    fn remaining_and_total_produce_used_quota_and_credit_pair() {
        let renews_at = Utc::now() + Duration::days(30);
        let v = json!({
            "credits": 12.75,
            "credits_total": "100",
            "renews_at": renews_at.to_rfc3339()
        });
        let h = parse_higgsfield_account(&v);
        assert_eq!(h.credits_total, Some(100.0));
        assert_eq!(h.detail_suffix(), Some("12.75/100 credits".to_string()));

        let quota = h.to_quota().unwrap();
        assert_eq!(quota.percent, 87.25);
        assert_eq!(quota.resets_at.unwrap(), renews_at);
        assert_eq!(quota.window_seconds, None);
    }

    #[test]
    fn renewal_instant_accepts_rfc3339() {
        let renews_at = Utc::now() + Duration::days(30);
        let rfc3339 = parse_higgsfield_account(&json!({
            "credits": 1,
            "credits_total": 2,
            "reset_at": renews_at.to_rfc3339()
        }));
        assert_eq!(rfc3339.renews_at.unwrap(), renews_at);
    }

    #[test]
    fn renewal_instant_accepts_epoch_seconds() {
        let seconds = Utc::now().timestamp() + 30 * 24 * 60 * 60;
        let parsed = parse_higgsfield_account(&json!({
            "credits": 1,
            "credits_total": 2,
            "reset_at": seconds
        }));
        assert_eq!(parsed.renews_at.unwrap().timestamp(), seconds);
    }

    #[test]
    fn renewal_instant_accepts_epoch_milliseconds() {
        let seconds = Utc::now().timestamp() + 30 * 24 * 60 * 60;
        let parsed = parse_higgsfield_account(&json!({
            "credits": 1,
            "credits_total": 2,
            "reset_at": seconds * 1000
        }));
        assert_eq!(parsed.renews_at.unwrap().timestamp(), seconds);
    }

    #[test]
    fn renewal_instant_rejects_out_of_range_epoch() {
        let far_future = Utc::now().timestamp() + 100 * 365 * 24 * 60 * 60;
        let parsed = parse_higgsfield_account(&json!({
            "credits": 1,
            "credits_total": 2,
            "reset_at": far_future
        }));
        assert!(parsed.renews_at.is_none());
    }

    #[test]
    fn renewal_instant_rejects_garbage() {
        let parsed = parse_higgsfield_account(&json!({
            "credits": 1,
            "credits_total": 2,
            "reset_at": "not a timestamp"
        }));
        assert!(parsed.renews_at.is_none());
    }

    #[test]
    fn unusable_credit_totals_do_not_produce_a_quota() {
        for total in [0.0, -1.0, 12.0] {
            let h = parse_higgsfield_account(&json!({
                "credits": 12.75,
                "credits_total": total
            }));
            assert!(h.credits_total.is_none());
            assert!(h.to_quota().is_none(), "total {total} must be rejected");
        }
    }
}
