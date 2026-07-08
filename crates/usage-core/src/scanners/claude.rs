use chrono::{DateTime, Utc};
use serde_json::Value;
use crate::models::ModelTokenEvent;

pub fn parse_claude_line(line: &str) -> Option<ModelTokenEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    let ts = DateTime::parse_from_rfc3339(v.get("timestamp")?.as_str()?)
        .ok()?.with_timezone(&Utc);
    let msg = v.get("message")?;
    let usage = msg.get("usage")?;
    let field = |k: &str| usage.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
    let tokens = field("input_tokens") + field("output_tokens")
        + field("cache_creation_input_tokens") + field("cache_read_input_tokens");
    if tokens <= 0 { return None; }
    let model = msg.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let dedupe_key = msg.get("id").and_then(|i| i.as_str()).map(String::from);
    Some(ModelTokenEvent { timestamp: ts, model, tokens, dedupe_key })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_usage_fields_and_sets_dedupe() {
        let line = r#"{"timestamp":"2026-07-08T10:00:00Z","message":{"id":"msg_1","model":"claude-sonnet-5","usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":5}}}"#;
        let e = parse_claude_line(line).unwrap();
        assert_eq!(e.tokens, 35);
        assert_eq!(e.dedupe_key.as_deref(), Some("msg_1"));
        assert_eq!(e.model, "claude-sonnet-5");
    }

    #[test]
    fn ignores_line_without_usage() {
        assert!(parse_claude_line(r#"{"timestamp":"2026-07-08T10:00:00Z","message":{"role":"user"}}"#).is_none());
    }
}
