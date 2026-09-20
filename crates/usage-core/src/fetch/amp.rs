use serde_json::Value;

use crate::models::QuotaUsage;

/// Parsed Amp `userDisplayBalanceInfo` JSON-RPC result.
/// Used % only when remaining AND total are both present — never invent %
/// from a remaining-only credits line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AmpBalance {
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

fn first_f64(root: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| root.get(*key).and_then(json_f64))
        .filter(|n| n.is_finite())
}

fn dollars_remaining(amount: f64) -> String {
    if amount.fract() == 0.0 {
        format!("${amount:.0} remaining")
    } else {
        format!("${amount:.2} remaining")
    }
}

fn is_money_token(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// `$remaining/$total remaining` (Amp Free).
fn parse_remaining_over_total(text: &str) -> Option<(f64, f64)> {
    let mut rest = text;
    while let Some(idx) = rest.find('$') {
        rest = &rest[idx + 1..];
        let Some(slash) = rest.find('/') else {
            break;
        };
        let remaining_s = rest[..slash].trim();
        let after_slash = rest[slash + 1..].trim_start();
        rest = &rest[slash + 1..];
        if !is_money_token(remaining_s) || !after_slash.starts_with('$') {
            continue;
        }
        let after_dollar = &after_slash[1..];
        let total_end = after_dollar
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(after_dollar.len());
        let total_s = &after_dollar[..total_end];
        if !is_money_token(total_s) {
            continue;
        }
        let tail = after_dollar[total_end..].trim_start();
        if !tail.to_ascii_lowercase().starts_with("remaining") {
            continue;
        }
        let remaining: f64 = remaining_s.parse().ok()?;
        let total: f64 = total_s.parse().ok()?;
        if remaining.is_finite() && total.is_finite() && total > 0.0 {
            return Some((remaining, total));
        }
    }
    None
}

/// `$N remaining` that is not a `$N/$M remaining` pair.
fn parse_lone_remaining(text: &str) -> Option<f64> {
    let hay = if let Some(idx) = text.to_ascii_lowercase().find("individual credits") {
        &text[idx..]
    } else {
        text
    };
    let mut rest = hay;
    while let Some(idx) = rest.find('$') {
        rest = &rest[idx + 1..];
        if rest.starts_with('/') {
            continue;
        }
        let end = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(rest.len());
        let amount_s = &rest[..end];
        if !is_money_token(amount_s) {
            continue;
        }
        let tail = rest[end..].trim_start();
        if tail.starts_with('/') {
            continue;
        }
        if !tail.to_ascii_lowercase().starts_with("remaining") {
            continue;
        }
        let amount: f64 = amount_s.parse().ok()?;
        if amount.is_finite() {
            return Some(amount);
        }
    }
    None
}

fn unwrap_rpc(root: &Value) -> &Value {
    root.get("result")
        .or_else(|| root.get("data"))
        .unwrap_or(root)
}

fn display_text(root: &Value) -> Option<&str> {
    first_str(
        root,
        &[
            "displayText",
            "display_text",
            "text",
            "message",
            "formatted",
        ],
    )
}

fn first_str<'a>(root: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| root.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn used_from_remaining_total(remaining: f64, total: f64) -> Option<QuotaUsage> {
    if !remaining.is_finite() || !total.is_finite() || total <= 0.0 {
        return None;
    }
    Some(QuotaUsage {
        percent: ((total - remaining) / total * 100.0).clamp(0.0, 100.0),
        resets_at: None,
        window_seconds: None,
    })
}

/// Reads `displayText` (and optional remaining/total fields). Remaining-only
/// balances stay suffix-only.
pub fn parse_amp_balance(root: &Value) -> AmpBalance {
    let data = unwrap_rpc(root);
    let nested = data
        .get("balance")
        .or_else(|| data.get("info"))
        .unwrap_or(data);

    let structured_remaining = first_f64(
        nested,
        &[
            "remaining",
            "remainingBalance",
            "remaining_balance",
            "creditsRemaining",
            "credits_remaining",
        ],
    );
    let structured_total = first_f64(
        nested,
        &["total", "limit", "totalBalance", "total_balance", "credits"],
    )
    .filter(|n| *n > 0.0);

    let text = display_text(data).or_else(|| display_text(nested));
    let text_pair = text.and_then(parse_remaining_over_total);
    let text_lone = text.and_then(parse_lone_remaining);

    if let Some((remaining, total)) = text_pair {
        return AmpBalance {
            period: used_from_remaining_total(remaining, total),
            detail_suffix: None,
        };
    }
    if let (Some(remaining), Some(total)) = (structured_remaining, structured_total) {
        return AmpBalance {
            period: used_from_remaining_total(remaining, total),
            detail_suffix: None,
        };
    }
    let remaining = text_lone.or(structured_remaining);
    AmpBalance {
        period: None,
        detail_suffix: remaining.map(dollars_remaining),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn remaining_and_total_from_display_text_produce_used_percent() {
        let q = parse_amp_balance(&json!({
            "result": {
                "displayText": "Signed in as a@b\nAmp Free: $12/$20 remaining (replenishes +$2/hour)"
            }
        }));
        assert!((q.period.as_ref().unwrap().percent - 40.0).abs() < 0.01);
        assert!(q.detail_suffix.is_none());
    }

    #[test]
    fn remaining_only_credits_do_not_invent_percent() {
        let q = parse_amp_balance(&json!({
            "displayText": "Signed in as a@b\nIndividual credits: $5 remaining - https://ampcode.com/settings"
        }));
        assert!(q.period.is_none());
        assert_eq!(q.detail_suffix.as_deref(), Some("$5 remaining"));
    }

    #[test]
    fn remaining_and_total_from_structured_fields() {
        let q = parse_amp_balance(&json!({
            "remaining": 25,
            "total": 100
        }));
        assert!((q.period.as_ref().unwrap().percent - 75.0).abs() < 0.01);
        assert!(q.detail_suffix.is_none());
    }

    #[test]
    fn structured_remaining_only_is_suffix() {
        let q = parse_amp_balance(&json!({ "remaining": 8.5 }));
        assert!(q.period.is_none());
        assert_eq!(q.detail_suffix.as_deref(), Some("$8.50 remaining"));
    }

    #[test]
    fn zero_remaining_with_total_is_fully_used() {
        let q = parse_amp_balance(&json!({
            "displayText": "Amp Free: $0/$10 remaining"
        }));
        assert_eq!(q.period.as_ref().unwrap().percent, 100.0);
    }

    #[test]
    fn empty_json_has_no_balance() {
        let q = parse_amp_balance(&json!({}));
        assert!(q.period.is_none());
        assert!(q.detail_suffix.is_none());
    }

    #[test]
    fn amp_free_pair_wins_over_credits_line() {
        let q = parse_amp_balance(&json!({
            "displayText": "Amp Free: $4/$10 remaining\nIndividual credits: $2 remaining"
        }));
        assert!((q.period.as_ref().unwrap().percent - 60.0).abs() < 0.01);
        assert!(q.detail_suffix.is_none());
    }
}
