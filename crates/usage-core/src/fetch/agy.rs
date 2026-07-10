//! Antigravity (`agy`) quota parsing for `RetrieveUserQuotaSummary`.
//!
//! The Antigravity Model Quota UI exposes two shared pools:
//! - Gemini Models (weekly / optional 5h)
//! - Claude and GPT models (weekly / optional 5h)
//!
//! API fractions are *remaining* (1.0 = 100% left). UsageCheck stores and
//! displays *used* percent (0–100), matching Codex/Claude tray semantics.

use chrono::{DateTime, TimeZone, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::models::QuotaUsage;

/// One Antigravity quota pool (e.g. "Gemini Models").
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AgyQuotaPool {
    pub name: String,
    pub five_hour: Option<QuotaUsage>,
    pub week: Option<QuotaUsage>,
}

/// Parsed Antigravity quota summary.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AgyQuota {
    pub email: Option<String>,
    pub plan: Option<String>,
    pub pools: Vec<AgyQuotaPool>,
}

fn parse_reset_time(v: &Value) -> Option<DateTime<Utc>> {
    if let Some(s) = v.as_str() {
        return DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.with_timezone(&Utc));
    }
    if let Some(n) = v.as_f64() {
        return Utc.timestamp_opt(n as i64, 0).single();
    }
    // google.protobuf.Timestamp JSON: { "seconds": "...", "nanos": ... }
    if let Some(obj) = v.as_object() {
        if let Some(secs) = obj
            .get("seconds")
            .and_then(|x| x.as_i64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
        {
            let nanos = obj
                .get("nanos")
                .and_then(|x| x.as_u64())
                .unwrap_or(0) as u32;
            return Utc.timestamp_opt(secs, nanos).single();
        }
    }
    None
}

fn window_seconds_for(bucket_id: Option<&str>, window: Option<&str>, display: Option<&str>) -> Option<i64> {
    let blob = format!(
        "{} {} {}",
        bucket_id.unwrap_or(""),
        window.unwrap_or(""),
        display.unwrap_or("")
    )
    .to_ascii_lowercase();
    if blob.contains("5h") || blob.contains("five") || blob.contains("session") {
        Some(5 * 3600)
    } else if blob.contains("week") {
        Some(7 * 24 * 3600)
    } else {
        None
    }
}

fn remaining_to_quota(bucket: &Value) -> Option<(bool /*is_week*/, QuotaUsage)> {
    let remaining = bucket
        .get("remainingFraction")
        .or_else(|| bucket.get("remaining_fraction"))
        .or_else(|| {
            bucket
                .get("remaining")
                .and_then(|r| r.get("remainingFraction").or_else(|| r.get("remaining_fraction")))
        })
        .and_then(|x| x.as_f64())?;

    // API remainingFraction → used % (0 unused … 100 exhausted), like Codex/Claude.
    let percent = ((1.0 - remaining) * 100.0).clamp(0.0, 100.0);
    let bucket_id = bucket
        .get("bucketId")
        .or_else(|| bucket.get("bucket_id"))
        .and_then(|x| x.as_str());
    let window = bucket.get("window").and_then(|x| x.as_str());
    let display = bucket
        .get("displayName")
        .or_else(|| bucket.get("display_name"))
        .and_then(|x| x.as_str());
    let resets_at = bucket
        .get("resetTime")
        .or_else(|| bucket.get("reset_time"))
        .or_else(|| {
            bucket
                .get("remaining")
                .and_then(|r| r.get("resetTime").or_else(|| r.get("reset_time")))
        })
        .and_then(parse_reset_time);
    let window_seconds = window_seconds_for(bucket_id, window, display);
    let is_week = matches!(window_seconds, Some(s) if s >= 24 * 3600)
        || window.map(|w| w.to_ascii_lowercase().contains("week")).unwrap_or(false)
        || bucket_id
            .map(|id| id.to_ascii_lowercase().contains("week"))
            .unwrap_or(false);

    Some((
        is_week,
        QuotaUsage {
            percent,
            resets_at,
            window_seconds,
        },
    ))
}

fn parse_group(group: &Value) -> Option<AgyQuotaPool> {
    let name = group
        .get("displayName")
        .or_else(|| group.get("display_name"))
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())?
        .to_string();
    let buckets = group
        .get("buckets")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();

    let mut five_hour = None;
    let mut week = None;
    for bucket in &buckets {
        let Some((is_week, quota)) = remaining_to_quota(bucket) else {
            continue;
        };
        if is_week {
            week = Some(pick_higher_used(week, quota));
        } else {
            five_hour = Some(pick_higher_used(five_hour, quota));
        }
    }
    if five_hour.is_none() && week.is_none() {
        return None;
    }
    Some(AgyQuotaPool {
        name,
        five_hour,
        week,
    })
}

/// Most constrained window = highest used %.
fn pick_higher_used(existing: Option<QuotaUsage>, next: QuotaUsage) -> QuotaUsage {
    match existing {
        Some(prev) if prev.percent >= next.percent => prev,
        _ => next,
    }
}

/// Parses a `RetrieveUserQuotaSummary` JSON body (local Connect-RPC or remote
/// Cloud Code). Accepts both `{ "response": { "groups": [...] } }` and a bare
/// `{ "groups": [...] }` shape.
pub fn parse_agy_quota_summary(root: &Value) -> AgyQuota {
    let response = root.get("response").unwrap_or(root);
    let groups = response
        .get("groups")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();

    let mut pools: Vec<AgyQuotaPool> = groups.iter().filter_map(parse_group).collect();

    // Prefer Gemini pool first, then Claude/GPT — matches Antigravity UI order.
    pools.sort_by(|a, b| {
        let rank = |n: &str| {
            let l = n.to_ascii_lowercase();
            if l.contains("gemini") {
                0
            } else if l.contains("claude") || l.contains("gpt") {
                1
            } else {
                2
            }
        };
        rank(&a.name).cmp(&rank(&b.name))
    });

    AgyQuota {
        email: None,
        plan: None,
        pools,
    }
}

/// Extracts email / plan from a `GetUserStatus` JSON body when available.
pub fn parse_agy_user_status(root: &Value) -> (Option<String>, Option<String>) {
    let status = root
        .get("userStatus")
        .or_else(|| root.get("user_status"))
        .unwrap_or(root);
    let email = status
        .get("email")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let plan = status
        .pointer("/planStatus/planInfo/planName")
        .or_else(|| status.pointer("/plan_status/plan_info/plan_name"))
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    (email, plan)
}

/// Compact account-level windows: most constrained 5h / week across pools.
pub fn compact_windows(pools: &[AgyQuotaPool]) -> (Option<QuotaUsage>, Option<QuotaUsage>) {
    let mut five = None;
    let mut week = None;
    for pool in pools {
        if let Some(q) = &pool.five_hour {
            five = Some(pick_higher_used(five.clone(), q.clone()));
        }
        if let Some(q) = &pool.week {
            week = Some(pick_higher_used(week.clone(), q.clone()));
        }
    }
    (five, week)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_two_pool_weekly_summary() {
        let v = json!({
            "response": {
                "groups": [
                    {
                        "displayName": "Gemini Models",
                        "buckets": [{
                            "bucketId": "gemini-weekly",
                            "displayName": "Weekly Limit",
                            "window": "weekly",
                            "remainingFraction": 1.0,
                            "resetTime": "2026-07-16T11:44:14Z"
                        }]
                    },
                    {
                        "displayName": "Claude and GPT models",
                        "buckets": [{
                            "bucketId": "3p-weekly",
                            "window": "weekly",
                            "remainingFraction": 0.815893,
                            "resetTime": "2026-07-13T08:39:51Z"
                        }]
                    }
                ]
            }
        });
        let q = parse_agy_quota_summary(&v);
        assert_eq!(q.pools.len(), 2);
        assert_eq!(q.pools[0].name, "Gemini Models");
        // remaining 1.0 → used 0%; remaining 0.815893 → used ~18.4107%
        assert!((q.pools[0].week.as_ref().unwrap().percent - 0.0).abs() < 0.01);
        assert!((q.pools[1].week.as_ref().unwrap().percent - 18.4107).abs() < 0.01);
        let (five, week) = compact_windows(&q.pools);
        assert!(five.is_none());
        // Most constrained = highest used.
        assert!((week.unwrap().percent - 18.4107).abs() < 0.01);
    }

    #[test]
    fn parses_user_status_email_plan() {
        let v = json!({
            "userStatus": {
                "email": "a@b.com",
                "planStatus": { "planInfo": { "planName": "Pro" } }
            }
        });
        let (email, plan) = parse_agy_user_status(&v);
        assert_eq!(email.as_deref(), Some("a@b.com"));
        assert_eq!(plan.as_deref(), Some("Pro"));
    }
}
