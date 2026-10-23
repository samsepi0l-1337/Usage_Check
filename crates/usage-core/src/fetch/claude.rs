use crate::models::{QuotaUsage, UsageBreakdownRow};
use chrono::{DateTime, Utc};
use serde_json::Value;

pub struct ClaudeQuota {
    pub five_hour: Option<QuotaUsage>,
    pub week: Option<QuotaUsage>,
    /// Extra per-model breakdown rows (e.g. "Fable" weekly usage), sourced
    /// from `limits[]` entries scoped to a specific model and/or the
    /// `seven_day_overage_included` window Claude Code labels "Fable limit".
    pub breakdown: Vec<UsageBreakdownRow>,
}

fn parse_resets_at(v: &Value) -> Option<DateTime<Utc>> {
    let raw = v.get("resets_at").or_else(|| v.get("resetsAt"))?;
    match raw {
        Value::String(s) => DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Utc)),
        Value::Number(n) => n
            .as_f64()
            .and_then(|s| chrono::TimeZone::timestamp_opt(&Utc, s as i64, 0).single()),
        _ => None,
    }
}

fn percent_field(v: &Value) -> Option<f64> {
    v.get("utilization")
        .or_else(|| v.get("percent"))
        .or_else(|| v.get("used_percentage"))
        .and_then(Value::as_f64)
        .filter(|p| p.is_finite())
}

fn window(v: &Value) -> Option<QuotaUsage> {
    Some(QuotaUsage {
        percent: percent_field(v)?,
        resets_at: parse_resets_at(v),
        window_seconds: None,
    })
}

/// Weekly per-model window duration for Fable / `limits[]` entries (matches
/// the `seven_day` window the app already surfaces).
const WEEKLY_SCOPED_SECONDS: i64 = 7 * 24 * 60 * 60;

const FABLE_LABEL: &str = "Fable";

/// Unwrap `{ utilization: { five_hour, seven_day, limits } }` if the live
/// body (or a Claude Code cache) nests windows under `utilization`.
fn usage_object(root: &Value) -> &Value {
    let Some(inner) = root.get("utilization") else {
        return root;
    };
    if inner.is_object()
        && (inner.get("five_hour").is_some()
            || inner.get("seven_day").is_some()
            || inner.get("limits").is_some()
            || inner.get("seven_day_overage_included").is_some())
    {
        inner
    } else {
        root
    }
}

fn is_fable_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    lower == "fable"
        || lower.starts_with("fable ")
        || lower.contains("fable")
        || lower.contains("claude-fable")
}

fn scoped_model_name(entry: &Value) -> Option<&str> {
    let model = entry.get("scope")?.get("model")?;
    model
        .get("display_name")
        .and_then(Value::as_str)
        .or_else(|| model.get("id").and_then(Value::as_str))
        .filter(|name| !name.is_empty())
}

fn fable_row(percent: f64, resets_at: Option<DateTime<Utc>>) -> UsageBreakdownRow {
    UsageBreakdownRow {
        label: FABLE_LABEL.to_string(),
        usage: QuotaUsage {
            percent,
            resets_at,
            window_seconds: Some(WEEKLY_SCOPED_SECONDS),
        },
    }
}

/// Fable's own weekly cap from `limits[]` (`kind: weekly_scoped`,
/// `scope.model.display_name` like `"Fable"` / `"Fable 5"`). Percent may be
/// `percent`, `utilization`, or `used_percentage`.
fn parse_fable_from_limits(root: &Value) -> Option<UsageBreakdownRow> {
    let limits = root.get("limits")?.as_array()?;
    let entry = limits
        .iter()
        .find(|entry| scoped_model_name(entry).is_some_and(is_fable_name))?;
    Some(fable_row(percent_field(entry)?, parse_resets_at(entry)))
}

/// Claude Code maps `seven_day_overage_included` to "Fable limit". Same
/// window shape as `five_hour` / `seven_day` (`utilization` + `resets_at`).
fn parse_fable_from_overage_included(root: &Value) -> Option<UsageBreakdownRow> {
    let entry = root.get("seven_day_overage_included")?;
    if entry.is_null() {
        return None;
    }
    Some(fable_row(percent_field(entry)?, parse_resets_at(entry)))
}

fn parse_fable_breakdown(root: &Value) -> Option<UsageBreakdownRow> {
    parse_fable_from_limits(root).or_else(|| parse_fable_from_overage_included(root))
}

pub fn parse_claude_usage(root: &Value) -> ClaudeQuota {
    let root = usage_object(root);
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

    #[test]
    fn parses_fable_from_seven_day_overage_included() {
        let v = json!({
            "five_hour": {"utilization": 10.0},
            "seven_day": {"utilization": 20.0},
            "seven_day_overage_included": {
                "utilization": 44.0,
                "resets_at": "2026-09-09T05:00:00Z"
            }
        });
        let q = parse_claude_usage(&v);
        assert_eq!(q.breakdown.len(), 1);
        assert_eq!(q.breakdown[0].label, "Fable");
        assert_eq!(q.breakdown[0].usage.percent, 44.0);
        assert!(q.breakdown[0].usage.resets_at.is_some());
        assert_eq!(q.breakdown[0].usage.window_seconds, Some(604_800));
    }

    #[test]
    fn fable_limits_entry_wins_over_overage_included() {
        let v = json!({
            "seven_day_overage_included": {"utilization": 99.0},
            "limits": [{
                "kind": "weekly_scoped",
                "percent": 12.0,
                "scope": { "model": { "display_name": "Fable 5.1" } }
            }]
        });
        let q = parse_claude_usage(&v);
        assert_eq!(q.breakdown.len(), 1);
        assert_eq!(q.breakdown[0].label, "Fable");
        assert_eq!(q.breakdown[0].usage.percent, 12.0);
    }

    #[test]
    fn parses_fable_utilization_alias_and_nested_wrapper() {
        let v = json!({
            "utilization": {
                "five_hour": {"utilization": 1.0},
                "seven_day": {"used_percentage": 2.0},
                "limits": [{
                    "kind": "weekly_scoped",
                    "utilization": 33.0,
                    "scope": { "model": { "id": "claude-fable-5", "display_name": null } }
                }]
            }
        });
        let q = parse_claude_usage(&v);
        assert_eq!(q.five_hour.as_ref().unwrap().percent, 1.0);
        assert_eq!(q.week.as_ref().unwrap().percent, 2.0);
        assert_eq!(q.breakdown.len(), 1);
        assert_eq!(q.breakdown[0].label, "Fable");
        assert_eq!(q.breakdown[0].usage.percent, 33.0);
    }

    #[test]
    fn null_overage_included_is_not_a_fable_row() {
        let v = json!({
            "five_hour": {"utilization": 1.0},
            "seven_day_overage_included": null
        });
        let q = parse_claude_usage(&v);
        assert!(q.breakdown.is_empty());
    }
}
