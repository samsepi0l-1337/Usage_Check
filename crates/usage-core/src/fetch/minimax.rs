use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::Value;

use crate::models::QuotaUsage;

#[derive(Debug, Clone, Default)]
pub struct MiniMaxQuota {
    pub email: Option<String>,
    pub plan: Option<String>,
    pub five_hour: Option<QuotaUsage>,
    pub week: Option<QuotaUsage>,
}

fn first_f64(root: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        let current = &root[key];
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

fn first_str(root: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = root
            .get(*key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            return Some(s.to_string());
        }
    }
    None
}

fn used_percent_from_remaining(remaining: f64) -> Option<f64> {
    if (0.0..=100.0).contains(&remaining) {
        Some((100.0 - remaining).clamp(0.0, 100.0))
    } else {
        None
    }
}

fn resets_from_remaining_ms(ms: Option<f64>) -> Option<DateTime<Utc>> {
    let ms = ms.filter(|v| *v >= 0.0)?;
    Utc::now().checked_add_signed(Duration::milliseconds(ms as i64))
}

fn resets_from_epoch_ms(ms: Option<f64>) -> Option<DateTime<Utc>> {
    let ms = ms.filter(|v| *v > 0.0)?;
    let seconds = if ms.abs() >= 100_000_000_000.0 {
        ms / 1000.0
    } else {
        ms
    };
    Utc.timestamp_opt(seconds as i64, 0).single()
}

fn window_seconds_from_bounds(start_ms: Option<f64>, end_ms: Option<f64>) -> Option<i64> {
    let start = start_ms?;
    let end = end_ms?;
    if end > start {
        let seconds = ((end - start) / 1000.0) as i64;
        (seconds > 0).then_some(seconds)
    } else {
        None
    }
}

fn quota_from_remaining(
    remaining_percent: Option<f64>,
    remaining_ms: Option<f64>,
    end_ms: Option<f64>,
    window_seconds: Option<i64>,
) -> Option<QuotaUsage> {
    let percent = used_percent_from_remaining(remaining_percent?)?;
    Some(QuotaUsage {
        percent,
        resets_at: resets_from_remaining_ms(remaining_ms).or_else(|| resets_from_epoch_ms(end_ms)),
        window_seconds,
    })
}

fn model_name(row: &Value) -> Option<&str> {
    row.get("model_name")
        .or_else(|| row.get("modelName"))
        .or_else(|| row.get("model"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

fn row_has_windows(row: &Value) -> bool {
    first_f64(
        row,
        &[
            "current_interval_remaining_percent",
            "currentIntervalRemainingPercent",
            "current_weekly_remaining_percent",
            "currentWeeklyRemainingPercent",
        ],
    )
    .is_some()
}

fn pick_model_row(root: &Value) -> Option<&Value> {
    let rows = root
        .get("model_remains")
        .or_else(|| root.get("modelRemains"))
        .or_else(|| root.pointer("/data/model_remains"))
        .and_then(Value::as_array);
    if let Some(rows) = rows {
        let general = rows.iter().find(|row| {
            model_name(row)
                .map(|name| name.eq_ignore_ascii_case("general"))
                .unwrap_or(false)
        });
        if let Some(row) = general {
            return Some(row);
        }
        let text = rows.iter().find(|row| {
            model_name(row)
                .map(|name| name.to_ascii_lowercase().starts_with("minimax-m"))
                .unwrap_or(false)
        });
        if let Some(row) = text {
            return Some(row);
        }
        if let Some(row) = rows.iter().find(|row| row_has_windows(row)) {
            return Some(row);
        }
    }
    if row_has_windows(root) || root.get("model_name").is_some() || root.get("modelName").is_some()
    {
        return Some(root);
    }
    None
}

fn parse_row(row: &Value) -> (Option<QuotaUsage>, Option<QuotaUsage>) {
    let start_ms = first_f64(row, &["start_time", "startTime"]);
    let end_ms = first_f64(row, &["end_time", "endTime"]);
    let interval_window = window_seconds_from_bounds(start_ms, end_ms);
    let five_hour = quota_from_remaining(
        first_f64(
            row,
            &[
                "current_interval_remaining_percent",
                "currentIntervalRemainingPercent",
                "interval_remaining_percent",
            ],
        ),
        first_f64(
            row,
            &["remains_time", "remainsTime", "interval_remains_time"],
        ),
        end_ms,
        interval_window,
    );
    let week = quota_from_remaining(
        first_f64(
            row,
            &[
                "current_weekly_remaining_percent",
                "currentWeeklyRemainingPercent",
                "weekly_remaining_percent",
            ],
        ),
        first_f64(row, &["weekly_remains_time", "weeklyRemainsTime"]),
        None,
        None,
    );
    (five_hour, week)
}

pub fn parse_minimax_quota(root: &Value) -> MiniMaxQuota {
    let row = pick_model_row(root).unwrap_or(root);
    let (five_hour, week) = parse_row(row);
    let email = first_str(root, &["email", "user_email", "userEmail"])
        .or_else(|| {
            root.get("account")
                .and_then(|account| first_str(account, &["email", "user_email"]))
        })
        .or_else(|| {
            root.get("user")
                .and_then(|user| first_str(user, &["email", "user_email"]))
        });
    let plan = first_str(
        root,
        &[
            "plan",
            "plan_name",
            "planName",
            "subscription_plan_type",
            "token_plan",
            "tokenPlan",
        ],
    )
    .or_else(|| first_str(row, &["plan", "plan_name", "planName"]));
    MiniMaxQuota {
        email,
        plan,
        five_hour,
        week,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_token_plan_remaining_percent_as_used() {
        let v = json!({
            "model_remains": [{
                "model_name": "general",
                "current_interval_remaining_percent": 63,
                "current_weekly_remaining_percent": 96,
                "remains_time": 14_400_000,
                "weekly_remains_time": 259_200_000,
                "start_time": 1_780_848_000_000u64,
                "end_time": 1_780_866_000_000u64
            }]
        });
        let q = parse_minimax_quota(&v);
        let five = q.five_hour.expect("5h window");
        assert!((five.percent - 37.0).abs() < 0.01);
        assert_eq!(five.window_seconds, Some(18_000));
        assert!(five.resets_at.is_some());
        let week = q.week.expect("weekly window");
        assert!((week.percent - 4.0).abs() < 0.01);
        assert!(week.resets_at.is_some());
    }

    #[test]
    fn prefers_general_over_video_row() {
        let v = json!({
            "model_remains": [
                {
                    "model_name": "video",
                    "current_interval_remaining_percent": 100,
                    "current_weekly_remaining_percent": 100
                },
                {
                    "model_name": "general",
                    "current_interval_remaining_percent": 40,
                    "current_weekly_remaining_percent": 80
                }
            ]
        });
        let q = parse_minimax_quota(&v);
        assert!((q.five_hour.unwrap().percent - 60.0).abs() < 0.01);
        assert!((q.week.unwrap().percent - 20.0).abs() < 0.01);
    }

    #[test]
    fn prefers_minimax_m_when_general_absent() {
        let v = json!({
            "model_remains": [
                {
                    "model_name": "image-01",
                    "current_interval_remaining_percent": 10
                },
                {
                    "model_name": "MiniMax-M*",
                    "current_interval_remaining_percent": 70,
                    "current_weekly_remaining_percent": 90
                }
            ]
        });
        let q = parse_minimax_quota(&v);
        assert!((q.five_hour.unwrap().percent - 30.0).abs() < 0.01);
        assert!((q.week.unwrap().percent - 10.0).abs() < 0.01);
    }

    #[test]
    fn does_not_invent_percent_from_counts() {
        let v = json!({
            "model_remains": [{
                "model_name": "MiniMax-M*",
                "current_interval_total_count": 1500,
                "current_interval_usage_count": 1417,
                "current_weekly_total_count": 0,
                "current_weekly_usage_count": 0
            }]
        });
        let q = parse_minimax_quota(&v);
        assert!(q.five_hour.is_none());
        assert!(q.week.is_none());
    }

    #[test]
    fn accepts_camel_case_and_bare_row() {
        let v = json!({
            "modelName": "general",
            "currentIntervalRemainingPercent": "50",
            "currentWeeklyRemainingPercent": 25
        });
        let q = parse_minimax_quota(&v);
        assert!((q.five_hour.unwrap().percent - 50.0).abs() < 0.01);
        assert!((q.week.unwrap().percent - 75.0).abs() < 0.01);
    }

    #[test]
    fn out_of_range_remaining_percent_is_ignored() {
        let v = json!({
            "current_interval_remaining_percent": 140,
            "current_weekly_remaining_percent": -1
        });
        let q = parse_minimax_quota(&v);
        assert!(q.five_hour.is_none());
        assert!(q.week.is_none());
    }

    #[test]
    fn parses_email_and_plan_from_envelope() {
        let v = json!({
            "email": "person@example.com",
            "plan_name": "Token Plan Plus",
            "model_remains": [{
                "model_name": "general",
                "current_interval_remaining_percent": 100
            }]
        });
        let q = parse_minimax_quota(&v);
        assert_eq!(q.email.as_deref(), Some("person@example.com"));
        assert_eq!(q.plan.as_deref(), Some("Token Plan Plus"));
        assert_eq!(q.five_hour.unwrap().percent, 0.0);
    }
}
