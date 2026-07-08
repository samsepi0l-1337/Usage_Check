use chrono::{DateTime, Utc};
use serde_json::Value;
use crate::models::QuotaUsage;

pub struct ClaudeQuota {
    pub five_hour: Option<QuotaUsage>,
    pub week: Option<QuotaUsage>,
}

fn parse_resets_at(v: &Value) -> Option<DateTime<Utc>> {
    match v.get("resets_at") {
        Some(Value::String(s)) => DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc)),
        Some(Value::Number(n)) => n.as_f64()
            .and_then(|s| chrono::TimeZone::timestamp_opt(&Utc, s as i64, 0).single()),
        _ => None,
    }
}

fn window(v: &Value) -> Option<QuotaUsage> {
    let percent = v.get("utilization")?.as_f64()?;
    Some(QuotaUsage { percent, resets_at: parse_resets_at(v), window_seconds: None })
}

pub fn parse_claude_usage(root: &Value) -> ClaudeQuota {
    ClaudeQuota {
        five_hour: root.get("five_hour").and_then(window),
        week: root.get("seven_day").and_then(window),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_five_hour_and_seven_day() {
        let v = json!({
            "five_hour": {"utilization": 30.0, "resets_at": "2026-07-08T12:00:00Z"},
            "seven_day": {"utilization": 55.5}
        });
        let q = parse_claude_usage(&v);
        assert_eq!(q.five_hour.as_ref().unwrap().percent, 30.0);
        assert!(q.five_hour.as_ref().unwrap().resets_at.is_some());
        assert_eq!(q.week.as_ref().unwrap().percent, 55.5);
    }
}
