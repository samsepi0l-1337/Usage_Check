use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;
use crate::models::ModelTokenEvent;

fn parse_ts(v: &Value) -> Option<DateTime<Utc>> {
    match v.get("timestamp")? {
        Value::String(s) => DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc)),
        Value::Number(n) => {
            let ms = n.as_f64()?;
            let secs = if ms > 10_000_000_000.0 { ms / 1000.0 } else { ms };
            Utc.timestamp_opt(secs as i64, 0).single()
        }
        _ => None,
    }
}

pub fn parse_gemini_line(line: &str) -> Option<ModelTokenEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    let ts = parse_ts(&v)?;
    let tokens = v.pointer("/usageMetadata/totalTokenCount")?.as_i64()?;
    if tokens <= 0 { return None; }
    let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("gemini").to_string();
    let dedupe_key = Some(format!("gemini:{line}"));
    Some(ModelTokenEvent {
        timestamp: ts,
        model,
        tokens,
        dedupe_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn parses_total_token_count() {
        let line = r#"{"timestamp":"2026-07-08T10:00:00Z","model":"gemini-3.5-flash","usageMetadata":{"totalTokenCount":900}}"#;
        let e = parse_gemini_line(line).unwrap();
        assert_eq!(e.tokens, 900);
        assert_eq!(e.model, "gemini-3.5-flash");
    }

    #[test]
    fn parses_timestamp_unix_seconds() {
        let line = r#"{"timestamp":1751970000,"model":"gemini-3.5-flash","usageMetadata":{"totalTokenCount":500}}"#;
        let e = parse_gemini_line(line).unwrap();
        let expected_ts = Utc.timestamp_opt(1751970000, 0).single().unwrap();
        assert_eq!(e.timestamp, expected_ts);
        assert_eq!(e.tokens, 500);
    }

    #[test]
    fn parses_timestamp_unix_milliseconds() {
        let line = r#"{"timestamp":1751970000000,"model":"gemini-3.5-flash","usageMetadata":{"totalTokenCount":750}}"#;
        let e = parse_gemini_line(line).unwrap();
        // 1751970000000 ms / 1000 = 1751970000 s
        let expected_ts = Utc.timestamp_opt(1751970000, 0).single().unwrap();
        assert_eq!(e.timestamp, expected_ts);
        assert_eq!(e.tokens, 750);
    }

    #[test]
    fn defaults_model_to_gemini_when_absent() {
        let line = r#"{"timestamp":"2026-07-08T10:00:00Z","usageMetadata":{"totalTokenCount":300}}"#;
        let e = parse_gemini_line(line).unwrap();
        assert_eq!(e.model, "gemini");
        assert_eq!(e.tokens, 300);
    }

    #[test]
    fn sets_stable_distinct_dedupe_keys() {
        let line = r#"{"timestamp":"2026-07-08T10:00:00Z","model":"gemini-3.5-flash","usageMetadata":{"totalTokenCount":900}}"#;
        let different_line = r#"{"timestamp":"2026-07-08T10:00:00Z","model":"gemini-3.5-flash","usageMetadata":{"totalTokenCount":901}}"#;

        let first = parse_gemini_line(line).unwrap().dedupe_key;
        let second = parse_gemini_line(line).unwrap().dedupe_key;
        let different = parse_gemini_line(different_line).unwrap().dedupe_key;

        assert!(first.is_some());
        assert_eq!(first, second);
        assert_ne!(first, different);
    }
}
