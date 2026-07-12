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
pub
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

/// Account info extracted from app-server account/read response.
#[derive(Debug, Clone)]
pub struct AppServerAccount {
    pub id: String,
    pub email: Option<String>,
}

/// Parse account info from app-server account/read response.
/// Accepts chatgpt identity only; rejects null or API-key accounts.
/// 
/// FIX BUG 1: The input `value` is now the WHOLE line object ({"id":2,"result":{...}}).
/// This function unwraps "result" exactly once, avoiding double-nesting.
pub fn parse_app_server_account(value: &Value) -> Result<AppServerAccount, String> {
    let identity = value
        .get("result")
        .or_else(|| value.get("identity"))
        .ok_or("no identity in response")?;

    if identity.is_null() {
        return Err("null identity".to_string());
    }

    let id = identity
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("missing or non-string id")?
        .to_string();

    let email = identity
        .get("email")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Reject API-key accounts
    if let Some(ref e) = email {
        if e.starts_with("sk-") {
            return Err("API-key account not supported".to_string());
        }
    }

    Ok(AppServerAccount { id, email })
}

/// Parse rate-limit windows from app-server rateLimits/read response.
/// Maps usedPercent, windowDurationMins*60, resetsAt to QuotaUsage.
/// Returns (primary, secondary); missing windows are None.
/// 
/// FIX BUG 1: The input `value` is now the WHOLE line object ({"id":3,"result":{...}}).
/// This function unwraps "result" exactly once, avoiding double-nesting.
pub fn parse_app_server_rate_limits(value: &Value) -> Result<(Option<QuotaUsage>, Option<QuotaUsage>), String> {
    let rate_limits = value
        .get("result")
        .or_else(|| value.get("rate_limits"))
        .ok_or("no rate_limits in response")?;

    let parse_window = |w: &Value| -> Option<QuotaUsage> {
        let percent = w.get("usedPercent").and_then(|x| x.as_f64())?;
        let window_minutes = w.get("windowDurationMins").and_then(|x| x.as_i64())?;
        let window_seconds = window_minutes.checked_mul(60)?;
        let resets_at = w
            .get("resetsAt")
            .and_then(|x| x.as_f64())
            .and_then(|s| Utc.timestamp_opt(s as i64, 0).single());
        Some(QuotaUsage {
            percent,
            resets_at,
            window_seconds: Some(window_seconds),
        })
    };

    let primary = rate_limits
        .get("primary_window")
        .or_else(|| rate_limits.get("primaryWindow"))
        .and_then(parse_window);
    let secondary = rate_limits
        .get("secondary_window")
        .or_else(|| rate_limits.get("secondaryWindow"))
        .and_then(parse_window);

    Ok((primary, secondary))
}


#[cfg(test)]
pub
mod app_server {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_chatgpt_identity_accepted() {
        let value = json!({
            "result": {
                "id": "user-123",
                "email": "user@example.com"
            }
        });
        let result = parse_app_server_account(&value);
        assert!(result.is_ok());
        let account = result.unwrap();
        assert_eq!(account.id, "user-123");
        assert_eq!(account.email, Some("user@example.com".to_string()));
    }

    #[test]
    fn test_parse_null_identity_rejected() {
        let value = json!({"result": null});
        let result = parse_app_server_account(&value);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_api_key_identity_rejected() {
        let value = json!({
            "result": {
                "id": "sk-123abc",
                "email": "sk-project@openai.com"
            }
        });
        let result = parse_app_server_account(&value);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_rate_limits_primary_and_secondary() {
        let value = json!({
            "result": {
                "primaryWindow": {
                    "usedPercent": 42.5,
                    "windowDurationMins": 300,
                    "resetsAt": 1_900_000_000.0
                },
                "secondaryWindow": {
                    "usedPercent": 12.0,
                    "windowDurationMins": 10080,
                    "resetsAt": 1_900_700_000.0
                }
            }
        });
        let result = parse_app_server_rate_limits(&value);
        assert!(result.is_ok());
        let (primary, secondary) = result.unwrap();
        assert!(primary.is_some());
        let p = primary.unwrap();
        assert_eq!(p.percent, 42.5);
        assert_eq!(p.window_seconds, Some(18000)); // 300 * 60
        assert!(p.resets_at.is_some());

        assert!(secondary.is_some());
        let s = secondary.unwrap();
        assert_eq!(s.percent, 12.0);
        assert_eq!(s.window_seconds, Some(604800)); // 10080 * 60
    }

    #[test]
    fn test_parse_rate_limits_missing_windows_none() {
        let value = json!({"result": {}});
        let result = parse_app_server_rate_limits(&value);
        assert!(result.is_ok());
        let (primary, secondary) = result.unwrap();
        assert!(primary.is_none());
        assert!(secondary.is_none());
    }

    #[test]
    fn test_parse_rate_limits_interleaved_notifications() {
        // In practice, notifications come as separate lines with no id.
        // This test verifies the parsing function itself handles missing windows.
        let value = json!({
            "result": {
                "primaryWindow": {
                    "usedPercent": 50.0,
                    "windowDurationMins": 300
                }
            }
        });
        let (primary, secondary) = parse_app_server_rate_limits(&value).unwrap();
        assert!(primary.is_some());
        assert!(secondary.is_none());
    }
}
