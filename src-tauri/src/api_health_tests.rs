use super::*;
use chrono::TimeZone;

fn ts(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).single().unwrap()
}

#[test]
fn stale_seconds_is_none_before_first_publish() {
    assert_eq!(stale_seconds(None, ts(1_000)), None);
}

#[test]
fn stale_seconds_measures_age_and_clamps_negative() {
    assert_eq!(stale_seconds(Some(ts(1_000)), ts(1_042)), Some(42));
    // Clock skew (updated_at in the future) must not yield a negative age.
    assert_eq!(stale_seconds(Some(ts(1_100)), ts(1_000)), Some(0));
}

#[test]
fn status_counts_folds_by_status_string() {
    let counts = status_counts(&["ok", "ok", "stale", "needs_login"]);
    assert_eq!(counts.get("ok"), Some(&2));
    assert_eq!(counts.get("stale"), Some(&1));
    assert_eq!(counts.get("needs_login"), Some(&1));
    assert_eq!(counts.get("missing"), None);
}

#[test]
fn health_body_includes_enriched_fields() {
    let body = health_body("9.9.9", Some(ts(1_000)), &["ok", "stale"], ts(1_030));
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["status"], "ok");
    assert_eq!(v["version"], "9.9.9");
    assert_eq!(v["account_count"], 2);
    assert_eq!(v["stale_seconds"], 30);
    assert_eq!(v["status_counts"]["ok"], 1);
    assert_eq!(v["status_counts"]["stale"], 1);
    assert!(v["updated_at"].is_string());
}

#[test]
fn health_body_handles_empty_snapshot() {
    let body = health_body("9.9.9", None, &[], ts(1_000));
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["account_count"], 0);
    assert!(v["stale_seconds"].is_null());
    assert!(v["updated_at"].is_null());
    assert!(v["status_counts"].as_object().unwrap().is_empty());
}
