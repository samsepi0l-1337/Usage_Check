use chrono::{DateTime, Utc};
use serde_json::Value;
use crate::models::{QuotaUsage, UsageBreakdownRow};

pub struct ClaudeQuota {
    pub five_hour: Option<QuotaUsage>,
    pub week: Option<QuotaUsage>,
    /// Extra per-model breakdown rows (e.g. "Fable" weekly usage), sourced
    /// from `limits[]` entries scoped to a specific model.
    pub breakdown: Vec<UsageBreakdownRow>,
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

/// Weekly per-model window duration for `limits[]` entries (matches the
/// `seven_day` window the app already surfaces).
const WEEKLY_SCOPED_SECONDS: i64 = 7 * 24 * 60 * 60;

/// Extracts the "Fable" breakdown row from the top-level `limits[]` array
/// (present on `/api/oauth/usage`). Only the entry whose
/// `scope.model.display_name == "Fable"` is used; other entries (including
/// ones with `scope: null`) are skipped. Missing/unparseable `percent` yields
/// no row.
fn parse_fable_breakdown(root: &Value) -> Option<UsageBreakdownRow> {
    let limits = root.get("limits")?.as_array()?;
    let entry = limits.iter().find(|entry| {
        entry
            .get("scope")
            .and_then(|scope| scope.get("model"))
            .and_then(|model| model.get("display_name"))
            .and_then(Value::as_str)
            == Some("Fable")
    })?;
    let percent = entry.get("percent")?.as_f64()?;
    Some(UsageBreakdownRow {
        label: "Fable".to_string(),
        usage: QuotaUsage {
            percent,
            resets_at: parse_resets_at(entry),
            window_seconds: Some(WEEKLY_SCOPED_SECONDS),
        },
    })
}

pub fn parse_claude_usage(root: &Value) -> ClaudeQuota {
    ClaudeQuota {
        five_hour: root.get("five_hour").and_then(window),
        week: root.get("seven_day").and_then(window),
        breakdown: parse_fable_breakdown(root).into_iter().collect(),
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
        assert!(q.breakdown.is_empty());
    }

    #[test]
    fn parses_fable_breakdown_row_from_limits() {
        let v = json!({
            "five_hour": {"utilization": 30.0},
            "seven_day": {"utilization": 55.5},
            "limits": [
                {
                    "kind": "weekly_scoped",
                    "group": "weekly",
                    "percent": 28,
                    "severity": "normal",
                    "resets_at": "2026-07-15T00:00:00Z",
                    "scope": { "model": { "id": null, "display_name": "Fable" }, "surface": null },
                    "is_active": false
                },
                { "kind": "other", "scope": null }
            ]
        });
        let q = parse_claude_usage(&v);
        assert_eq!(q.breakdown.len(), 1);
        assert_eq!(q.breakdown[0].label, "Fable");
        assert_eq!(q.breakdown[0].usage.percent, 28.0);
        assert!(q.breakdown[0].usage.resets_at.is_some());
        assert_eq!(q.breakdown[0].usage.window_seconds, Some(604_800));
    }

    #[test]
    fn no_fable_entry_yields_empty_breakdown() {
        let v = json!({
            "five_hour": {"utilization": 30.0},
            "limits": [ { "kind": "other", "scope": null } ]
        });
        let q = parse_claude_usage(&v);
        assert!(q.breakdown.is_empty());
    }

    #[test]
    fn missing_limits_yields_empty_breakdown() {
        let v = json!({ "five_hour": {"utilization": 30.0} });
        let q = parse_claude_usage(&v);
        assert!(q.breakdown.is_empty());
    }
}
