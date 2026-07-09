use chrono::{TimeZone, Utc};
use serde_json::Value;
use crate::models::QuotaUsage;

pub struct CodexQuota {
    pub plan: Option<String>,
    pub email: Option<String>,
    pub five_hour: Option<QuotaUsage>,
    pub week: Option<QuotaUsage>,
}

fn window(v: &Value) -> Option<QuotaUsage> {
    // wham/usage reports *used* percent (0–100), not remaining.
    let percent = v
        .get("used_percent")
        .or_else(|| v.get("usedPercent"))
        .and_then(|x| x.as_f64())?;
    let resets_at = v
        .get("reset_at")
        .or_else(|| v.get("resetAt"))
        .and_then(|x| x.as_f64())
        .and_then(|s| Utc.timestamp_opt(s as i64, 0).single());
    let window_seconds = v
        .get("limit_window_seconds")
        .or_else(|| v.get("limitWindowSeconds"))
        .and_then(|x| x.as_i64())
        .filter(|s| *s > 0);
    Some(QuotaUsage {
        percent,
        resets_at,
        window_seconds,
    })
}

/// Human label for a Codex rate-limit window from its duration.
pub fn window_label(window_seconds: Option<i64>, fallback: &str) -> String {
    match window_seconds {
        Some(s) if (4 * 3600..6 * 3600).contains(&s) => "5h".into(),
        Some(s) if (6 * 24 * 3600..8 * 24 * 3600).contains(&s) => "7d".into(),
        Some(s) if s >= 3600 && s % 3600 == 0 => format!("{}h", s / 3600),
        Some(s) if s >= 86400 && s % 86400 == 0 => format!("{}d", s / 86400),
        Some(s) if s > 0 => format!("{s}s"),
        _ => fallback.into(),
    }
}

pub fn parse_codex_usage(root: &Value) -> CodexQuota {
    let rl = root.get("rate_limit");
    let get = |k: &str| rl.and_then(|r| r.get(k)).and_then(window);
    CodexQuota {
        plan: root
            .get("plan_type")
            .and_then(|x| x.as_str())
            .map(String::from),
        email: root
            .get("email")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from),
        five_hour: get("primary_window"),
        week: get("secondary_window"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_primary_and_secondary() {
        let v = json!({
            "plan_type": "prolite",
            "email": "user@example.com",
            "rate_limit": {
                "primary_window": {"used_percent": 42.5, "reset_at": 1_900_000_000.0, "limit_window_seconds": 18000},
                "secondary_window": {"used_percent": 12.0, "limit_window_seconds": 604800}
            }
        });
        let q = parse_codex_usage(&v);
        assert_eq!(q.plan.as_deref(), Some("prolite"));
        assert_eq!(q.email.as_deref(), Some("user@example.com"));
        assert_eq!(q.five_hour.as_ref().unwrap().percent, 42.5);
        assert_eq!(q.five_hour.as_ref().unwrap().window_seconds, Some(18000));
        assert!(q.five_hour.as_ref().unwrap().resets_at.is_some());
        assert_eq!(q.week.as_ref().unwrap().percent, 12.0);
        assert_eq!(q.week.as_ref().unwrap().window_seconds, Some(604800));
        assert_eq!(window_label(Some(18000), "5h"), "5h");
        assert_eq!(window_label(Some(604800), "7d"), "7d");
    }

    #[test]
    fn missing_percent_yields_none() {
        let v = serde_json::json!({"rate_limit": {"primary_window": {}}});
        let q = parse_codex_usage(&v);
        assert!(q.five_hour.is_none());
    }
}
