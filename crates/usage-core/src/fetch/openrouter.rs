use serde_json::Value;

use crate::models::QuotaUsage;

/// Parsed OpenRouter `GET /api/v1/key` credit usage.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpenRouterUsage {
    pub period: Option<QuotaUsage>,
    pub detail_suffix: Option<String>,
}

fn json_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Null => None,
        other => other
            .as_f64()
            .or_else(|| other.as_i64().map(|n| n as f64))
            .or_else(|| other.as_str().and_then(|s| s.trim().parse().ok())),
    }
}

fn money_suffix(amount: f64, leftover: bool) -> String {
    if leftover {
        format!("${amount:.2} left")
    } else {
        format!("${amount:.2} used")
    }
}

/// When `limit` and `limit_remaining` are both numbers, used % is
/// `(limit - limit_remaining) / limit * 100`. A null `limit` must not invent
/// a percent — remaining/usage is shown as `detail_suffix` instead.
pub fn parse_openrouter_key(root: &Value) -> OpenRouterUsage {
    let data = root.get("data").unwrap_or(root);
    let limit = data.get("limit").and_then(json_f64);
    let remaining = data.get("limit_remaining").and_then(json_f64);
    let usage = data.get("usage").and_then(json_f64);

    if let (Some(limit), Some(remaining)) = (limit, remaining) {
        if limit > 0.0 && limit.is_finite() && remaining.is_finite() {
            let percent = ((limit - remaining) / limit * 100.0).clamp(0.0, 100.0);
            return OpenRouterUsage {
                period: Some(QuotaUsage {
                    percent,
                    resets_at: None,
                    window_seconds: None,
                }),
                detail_suffix: None,
            };
        }
    }

    let detail_suffix = remaining
        .filter(|n| n.is_finite())
        .map(|n| money_suffix(n, true))
        .or_else(|| {
            usage
                .filter(|n| n.is_finite())
                .map(|n| money_suffix(n, false))
        });

    OpenRouterUsage {
        period: None,
        detail_suffix,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn computes_used_percent_when_limit_exists() {
        let v = json!({
            "data": {
                "label": "Production",
                "limit": 100,
                "limit_remaining": 74.5,
                "usage": 25.5
            }
        });
        let q = parse_openrouter_key(&v);
        assert!((q.period.as_ref().unwrap().percent - 25.5).abs() < 0.01);
        assert!(q.detail_suffix.is_none());
    }

    #[test]
    fn null_limit_shows_remaining_suffix() {
        let v = json!({
            "data": {
                "limit": null,
                "limit_remaining": 74.5,
                "usage": 25.5
            }
        });
        let q = parse_openrouter_key(&v);
        assert!(q.period.is_none());
        assert_eq!(q.detail_suffix.as_deref(), Some("$74.50 left"));
    }

    #[test]
    fn null_limit_falls_back_to_usage_suffix() {
        let v = json!({
            "data": {
                "limit": null,
                "limit_remaining": null,
                "usage": 25.5
            }
        });
        let q = parse_openrouter_key(&v);
        assert!(q.period.is_none());
        assert_eq!(q.detail_suffix.as_deref(), Some("$25.50 used"));
    }
}
