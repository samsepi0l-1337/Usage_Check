use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde_json::Value;

use crate::models::QuotaUsage;

/// Parsed GitHub Copilot `GET /copilot_internal/user` quota.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CopilotQuota {
    pub plan: Option<String>,
    /// Monthly billing period, mapped onto the tray `week` slot like Cursor.
    pub period: Option<QuotaUsage>,
    pub detail_suffix: Option<String>,
}

fn json_f64(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_i64().map(|n| n as f64))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

fn nonempty_str(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn parse_reset(v: &Value) -> Option<DateTime<Utc>> {
    if let Some(s) = v.as_str() {
        let s = s.trim();
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Some(dt.with_timezone(&Utc));
        }
        if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            return date
                .and_hms_opt(0, 0, 0)
                .map(|naive| Utc.from_utc_datetime(&naive));
        }
    }
    if let Some(secs) = json_f64(v).filter(|n| n.is_finite()) {
        let secs = if secs.abs() >= 100_000_000_000.0 {
            secs / 1000.0
        } else {
            secs
        };
        return Utc.timestamp_opt(secs as i64, 0).single();
    }
    None
}

fn primary_snapshot(root: &Value) -> Option<&Value> {
    let snaps = root.get("quota_snapshots")?;
    ["premium_interactions", "premium_models", "chat"]
        .into_iter()
        .find_map(|key| snaps.get(key))
}

fn is_unlimited(snap: &Value) -> bool {
    snap.get("unlimited")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || snap.get("entitlement").and_then(json_f64) == Some(-1.0)
}

fn used_percent(snap: &Value) -> Option<f64> {
    if let Some(remaining) = snap.get("percent_remaining").and_then(json_f64) {
        if remaining.is_finite() {
            return Some((100.0 - remaining).clamp(0.0, 100.0));
        }
    }
    let entitlement = snap
        .get("entitlement")
        .and_then(json_f64)
        .filter(|n| n.is_finite() && *n > 0.0)?;
    let remaining = snap
        .get("remaining")
        .and_then(json_f64)
        .filter(|n| n.is_finite())?;
    Some(((entitlement - remaining) / entitlement * 100.0).clamp(0.0, 100.0))
}

fn snapshot_reset(root: &Value, snap: &Value) -> Option<DateTime<Utc>> {
    root.get("quota_reset_date")
        .and_then(parse_reset)
        .or_else(|| root.get("quota_reset_date_utc").and_then(parse_reset))
        .or_else(|| snap.get("quota_reset_at").and_then(parse_reset))
}

/// Completions are ignored as the primary bar. Unlimited / entitlement -1
/// yields no percent (suffix `"unlimited"`); otherwise used % from remaining.
pub fn parse_copilot_user(root: &Value) -> CopilotQuota {
    let plan = root.get("copilot_plan").and_then(nonempty_str);
    let Some(snap) = primary_snapshot(root) else {
        return CopilotQuota {
            plan,
            ..CopilotQuota::default()
        };
    };
    let resets_at = snapshot_reset(root, snap);
    if is_unlimited(snap) {
        return CopilotQuota {
            plan,
            period: None,
            detail_suffix: Some("unlimited".into()),
        };
    }
    CopilotQuota {
        plan,
        period: used_percent(snap).map(|percent| QuotaUsage {
            percent,
            resets_at,
            window_seconds: None,
        }),
        detail_suffix: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_premium_interactions_fixture() {
        let v = json!({
            "copilot_plan": "individual",
            "quota_reset_date": "2026-10-01",
            "quota_snapshots": {
                "premium_interactions": {
                    "entitlement": 300,
                    "remaining": 240,
                    "percent_remaining": 80.0,
                    "unlimited": false,
                    "overage_count": 0,
                    "overage_permitted": true
                },
                "chat": {
                    "unlimited": true,
                    "entitlement": 0,
                    "remaining": 0,
                    "percent_remaining": 100.0
                },
                "completions": {
                    "unlimited": true,
                    "entitlement": 0,
                    "remaining": 0,
                    "percent_remaining": 100.0
                }
            }
        });
        let q = parse_copilot_user(&v);
        assert_eq!(q.plan.as_deref(), Some("individual"));
        let period = q.period.as_ref().expect("premium used %");
        assert!((period.percent - 20.0).abs() < 0.001);
        assert_eq!(
            period.resets_at.unwrap().to_rfc3339(),
            "2026-10-01T00:00:00+00:00"
        );
        assert!(q.detail_suffix.is_none());
    }

    #[test]
    fn unlimited_or_negative_entitlement_has_no_percent_bar() {
        let unlimited = json!({
            "quota_snapshots": {
                "premium_interactions": { "unlimited": true, "percent_remaining": 100.0 }
            }
        });
        let q = parse_copilot_user(&unlimited);
        assert!(q.period.is_none());
        assert_eq!(q.detail_suffix.as_deref(), Some("unlimited"));

        let open = json!({
            "quota_snapshots": {
                "premium_interactions": { "entitlement": -1, "remaining": -1, "unlimited": false }
            }
        });
        let q = parse_copilot_user(&open);
        assert!(q.period.is_none());
        assert_eq!(q.detail_suffix.as_deref(), Some("unlimited"));
    }

    #[test]
    fn falls_back_to_chat_when_premium_missing() {
        let v = json!({
            "quota_snapshots": {
                "completions": { "unlimited": false, "percent_remaining": 10.0 },
                "chat": { "unlimited": false, "percent_remaining": 60.0 }
            }
        });
        let q = parse_copilot_user(&v);
        assert!((q.period.as_ref().unwrap().percent - 40.0).abs() < 0.001);
    }

    #[test]
    fn uses_entitlement_remaining_when_percent_missing() {
        let v = json!({
            "quota_reset_date_utc": "2026-11-01T00:00:00Z",
            "quota_snapshots": {
                "premium_models": { "entitlement": 50, "remaining": 20, "unlimited": false }
            }
        });
        let q = parse_copilot_user(&v);
        assert!((q.period.as_ref().unwrap().percent - 60.0).abs() < 0.001);
        assert_eq!(
            q.period.as_ref().unwrap().resets_at.unwrap().to_rfc3339(),
            "2026-11-01T00:00:00+00:00"
        );
    }
}
