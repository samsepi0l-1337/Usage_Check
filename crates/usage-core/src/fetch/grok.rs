use serde_json::Value;

use crate::models::QuotaUsage;

/// Resolves the billing team ID from `GET /auth/management-keys/validation`.
///
/// Prefers `scopeId` when `scope` is `SCOPE_TEAM` (or absent); falls back to
/// deprecated `teamId`.
pub fn team_id_from_validation(root: &Value) -> Option<String> {
    let scope = root
        .get("scope")
        .and_then(|v| v.as_str())
        .unwrap_or("SCOPE_TEAM");
    if let Some(scope_id) = root
        .get("scopeId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        if scope == "SCOPE_TEAM" || scope == "SCOPE_UNSPECIFIED" {
            return Some(scope_id.to_string());
        }
    }
    root.get("teamId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// True iff `team_id` is a plausible xAI team/scope id: non-empty after trimming and containing no
/// internal whitespace. xAI team/scope ids are single tokens (UUID-like or `team-…`); clipboard prose
/// (e.g. "Translated Report (Full Report Below)") has spaces and must be rejected so a malformed
/// `/teams/{team_id}/…` request can never be built.
pub fn is_valid_team_id(team_id: &str) -> bool {
    let trimmed = team_id.trim();
    !trimmed.is_empty() && !trimmed.chars().any(char::is_whitespace)
}

/// Parses clipboard paste text into `(management_key, optional_team_id)`.
///
/// Accepts a single key line, or key + team ID on separate non-empty lines.
pub fn parse_grok_paste(text: &str) -> (String, Option<String>) {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    match lines.as_slice() {
        [] => (String::new(), None),
        [key] => (key.to_string(), None),
        [key, team, ..] => (key.to_string(), Some(team.to_string())),
    }
}

/// Parsed xAI prepaid credit balance (Management API).
#[derive(Clone, Debug, PartialEq)]
pub struct GrokPrepaid {
    /// Used % since the most recent top-up (0–100), when computable.
    pub period: Option<QuotaUsage>,
    /// Secondary tray label, e.g. `"$23.17 left"` or `"API credits"`.
    pub detail_suffix: Option<String>,
}

fn cents_val(obj: Option<&Value>) -> Option<i64> {
    obj?.get("val")?.as_str()?.parse().ok()
}

fn abs_cents(cents: i64) -> i64 {
    cents.unsigned_abs() as i64
}

/// Computes spend-since-last-top-up used % from ledger `changes`.
pub fn grok_used_percent_from_changes(changes: &[Value]) -> Option<f64> {
    let mut spend_since = 0i64;
    let mut purchase_limit: Option<i64> = None;

    for change in changes.iter().rev() {
        let Some(origin) = change.get("changeOrigin").and_then(Value::as_str) else {
            continue;
        };
        let Some(amount) = cents_val(change.get("amount")) else {
            continue;
        };
        match origin {
            "SPEND" => spend_since += amount.max(0),
            "PURCHASE" | "AUTO_PURCHASE" => {
                purchase_limit = Some(abs_cents(amount));
                break;
            }
            _ => {}
        }
    }

    let limit = purchase_limit?;
    if limit <= 0 {
        return None;
    }
    Some((spend_since as f64 / limit as f64) * 100.0)
}

/// Parses `GET /v1/billing/teams/{team_id}/prepaid/balance` JSON.
pub fn parse_grok_prepaid_balance(root: &Value) -> GrokPrepaid {
    let remaining_cents = root
        .get("total")
        .and_then(|t| cents_val(Some(t)))
        .map(abs_cents);

    let changes = root
        .get("changes")
        .and_then(|c| c.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[]);

    let percent = grok_used_percent_from_changes(changes);
    let detail_suffix = remaining_cents.map(|c| format!("${:.2} left", c as f64 / 100.0));

    let period = percent.map(|p| QuotaUsage {
        percent: p,
        resets_at: None,
        window_seconds: None,
    });

    GrokPrepaid {
        period,
        detail_suffix: detail_suffix.or_else(|| Some("API credits".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_remaining_balance_and_spend_percent() {
        let v = json!({
            "total": { "val": "-2317" },
            "changes": [
                { "changeOrigin": "PURCHASE", "amount": { "val": "-2500" }, "createTs": "2026-03-23T12:56:21Z" },
                { "changeOrigin": "SPEND", "amount": { "val": "183" }, "createTs": "2026-04-10T21:40:00Z" }
            ]
        });
        let q = parse_grok_prepaid_balance(&v);
        assert!((q.period.as_ref().unwrap().percent - 7.32).abs() < 0.1);
        assert_eq!(q.detail_suffix.as_deref(), Some("$23.17 left"));
    }

    #[test]
    fn total_only_still_formats_suffix() {
        let v = json!({ "total": { "val": "-500" }, "changes": [] });
        let q = parse_grok_prepaid_balance(&v);
        assert!(q.period.is_none());
        assert_eq!(q.detail_suffix.as_deref(), Some("$5.00 left"));
    }

    #[test]
    fn malformed_entry_does_not_abort_percent_computation() {
        let changes = vec![
            json!({ "changeOrigin": "PURCHASE", "amount": { "val": "-2500" } }),
            json!({ "changeOrigin": "ADJUSTMENT", "amount": {} }),
            json!({ "changeOrigin": "SPEND", "amount": { "val": "183" } }),
        ];

        let percent = grok_used_percent_from_changes(&changes);
        assert!((percent.unwrap() - 7.32).abs() < 0.1);
    }

    #[test]
    fn team_id_prefers_scope_id_for_team_scope() {
        let v = json!({
            "scope": "SCOPE_TEAM",
            "scopeId": "team-from-scope",
            "teamId": "legacy-team"
        });
        assert_eq!(team_id_from_validation(&v).as_deref(), Some("team-from-scope"));
    }

    #[test]
    fn team_id_falls_back_to_deprecated_team_id() {
        let v = json!({ "teamId": "legacy-only" });
        assert_eq!(team_id_from_validation(&v).as_deref(), Some("legacy-only"));
    }

    #[test]
    fn validates_plausible_team_ids() {
        assert!(is_valid_team_id("team-abc123"));
        assert!(is_valid_team_id("550e8400-e29b-41d4-a716-446655440000"));
        assert!(is_valid_team_id("scope_9f..."));
        assert!(is_valid_team_id("  team-abc  "));
    }

    #[test]
    fn rejects_invalid_team_ids() {
        assert!(!is_valid_team_id("Translated Report (Full Report Below)"));
        assert!(!is_valid_team_id(""));
        assert!(!is_valid_team_id("   "));
        assert!(!is_valid_team_id("a b"));
        assert!(!is_valid_team_id("line1\nline2"));
    }

    #[test]
    fn parse_grok_paste_single_line() {
        let (key, team) = parse_grok_paste("  xai-mgmt-key-abc  \n");
        assert_eq!(key, "xai-mgmt-key-abc");
        assert!(team.is_none());
    }

    #[test]
    fn parse_grok_paste_key_and_team_lines() {
        let (key, team) = parse_grok_paste("key-line\nteam-uuid\n");
        assert_eq!(key, "key-line");
        assert_eq!(team.as_deref(), Some("team-uuid"));
    }
}
