use chrono::{TimeZone, Utc};
use serde_json::Value;
use crate::models::QuotaUsage;

pub struct CodexQuota {
    pub plan: Option<String>,
    pub five_hour: Option<QuotaUsage>,
    pub week: Option<QuotaUsage>,
}

fn window(v: &Value) -> Option<QuotaUsage> {
    let percent = v.get("used_percent")?.as_f64()?;
    let resets_at = v.get("reset_at").and_then(|x| x.as_f64())
        .and_then(|s| Utc.timestamp_opt(s as i64, 0).single());
    let window_seconds = v.get("limit_window_seconds").and_then(|x| x.as_i64())
        .filter(|s| *s > 0);
    Some(QuotaUsage { percent, resets_at, window_seconds })
}

pub fn parse_codex_usage(root: &Value) -> CodexQuota {
    let rl = root.get("rate_limit");
    let get = |k: &str| rl.and_then(|r| r.get(k)).and_then(window);
    CodexQuota {
        plan: root.get("plan_type").and_then(|x| x.as_str()).map(String::from),
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
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {"used_percent": 42.5, "reset_at": 1_900_000_000.0, "limit_window_seconds": 18000},
                "secondary_window": {"used_percent": 12.0}
            }
        });
        let q = parse_codex_usage(&v);
        assert_eq!(q.plan.as_deref(), Some("pro"));
        assert_eq!(q.five_hour.as_ref().unwrap().percent, 42.5);
        assert_eq!(q.five_hour.as_ref().unwrap().window_seconds, Some(18000));
        assert!(q.five_hour.as_ref().unwrap().resets_at.is_some());
        assert_eq!(q.week.as_ref().unwrap().percent, 12.0);
        assert!(q.week.as_ref().unwrap().window_seconds.is_none());
    }

    #[test]
    fn missing_percent_yields_none() {
        let v = serde_json::json!({"rate_limit": {"primary_window": {}}});
        let q = parse_codex_usage(&v);
        assert!(q.five_hour.is_none());
    }
}
