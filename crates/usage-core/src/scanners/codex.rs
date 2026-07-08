use chrono::{DateTime, Utc};
use serde_json::Value;
use crate::models::ModelTokenEvent;

pub fn parse_codex_line(line: &str) -> Option<ModelTokenEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    let ts = DateTime::parse_from_rfc3339(v.get("timestamp")?.as_str()?)
        .ok()?.with_timezone(&Utc);
    let payload = v.get("payload")?;
    let tokens = payload.pointer("/info/total_token_usage/total_tokens")?.as_i64()?;
    let model = payload.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
    Some(ModelTokenEvent { timestamp: ts, model, tokens, dedupe_key: None })
}

pub fn latest_remaining_percent(line: &str) -> Option<f64> {
    let v: Value = serde_json::from_str(line).ok()?;
    v.pointer("/rate_limit/primary/remaining_percent").and_then(|x| x.as_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_token_usage_line() {
        let line = r#"{"timestamp":"2026-07-08T10:00:00Z","payload":{"model":"gpt-5.3-codex-spark","info":{"total_token_usage":{"total_tokens":1234}}}}"#;
        let e = parse_codex_line(line).unwrap();
        assert_eq!(e.tokens, 1234);
        assert_eq!(e.model, "gpt-5.3-codex-spark");
    }

    #[test]
    fn ignores_non_token_line() {
        assert!(parse_codex_line(r#"{"timestamp":"2026-07-08T10:00:00Z","payload":{"type":"noise"}}"#).is_none());
    }

    #[test]
    fn reads_remaining_percent() {
        let line = r#"{"rate_limit":{"primary":{"remaining_percent":73.0}}}"#;
        assert_eq!(latest_remaining_percent(line), Some(73.0));
    }
}
