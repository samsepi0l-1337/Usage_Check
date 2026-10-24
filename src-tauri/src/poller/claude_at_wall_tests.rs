use super::*;
use serde_json::json;
use std::cell::Cell;

fn fable_body() -> serde_json::Value {
    json!({ "seven_day_overage_included": { "utilization": 12.0 } })
}

fn empty_quota() -> ClaudeQuota {
    parse_claude_usage(&json!({}))
}

#[test]
fn an_empty_at_wall_result_suppresses_the_retry_within_the_ttl() {
    let mut cache = AtWallNegativeCache::default();
    let t0 = Instant::now();

    cache.record_empty("acct-a", t0);

    assert!(cache.is_suppressed("acct-a", t0));
    assert!(cache.is_suppressed("acct-a", t0 + AT_WALL_EMPTY_TTL - Duration::from_secs(1)));
}

#[test]
fn the_suppression_expires_once_the_ttl_has_elapsed() {
    let mut cache = AtWallNegativeCache::default();
    let t0 = Instant::now();

    cache.record_empty("acct-a", t0);

    assert!(!cache.is_suppressed("acct-a", t0 + AT_WALL_EMPTY_TTL));
    assert!(!cache.is_suppressed("acct-a", t0 + AT_WALL_EMPTY_TTL + Duration::from_secs(60)));
}

#[test]
fn suppression_is_per_account() {
    let mut cache = AtWallNegativeCache::default();
    let t0 = Instant::now();

    cache.record_empty("acct-a", t0);

    assert!(!cache.is_suppressed("acct-b", t0));
    assert!(!cache.is_suppressed("never-seen", t0));
}

#[tokio::test]
async fn a_retry_that_returns_fable_is_merged_and_not_recorded() {
    let key = "gate-fable-merged";
    let calls = Cell::new(0);

    let quota = with_at_wall_retry(empty_quota(), key, Instant::now(), || {
        calls.set(calls.get() + 1);
        async { Ok(fable_body()) }
    })
    .await;

    assert_eq!(calls.get(), 1);
    assert_eq!(quota.breakdown.len(), 1);
    assert!(!at_wall_retry_suppressed(key, Instant::now()));
}

#[tokio::test]
async fn an_empty_retry_is_recorded_and_the_next_poll_skips_the_fetch() {
    let key = "gate-empty-recorded";
    let t0 = Instant::now();
    let calls = Cell::new(0);

    let first = with_at_wall_retry(empty_quota(), key, t0, || {
        calls.set(calls.get() + 1);
        async { Ok(json!({})) }
    })
    .await;
    assert!(first.breakdown.is_empty());
    assert_eq!(calls.get(), 1);
    assert!(at_wall_retry_suppressed(key, t0));

    let second = with_at_wall_retry(empty_quota(), key, t0, || {
        calls.set(calls.get() + 1);
        async { Ok(json!({})) }
    })
    .await;
    assert!(second.breakdown.is_empty());
    assert_eq!(calls.get(), 1, "a suppressed account must not issue the at_wall request");
}

#[tokio::test]
async fn an_http_error_on_retry_is_not_recorded() {
    let key = "gate-error-not-recorded";

    let quota = with_at_wall_retry(empty_quota(), key, Instant::now(), || async {
        Err::<serde_json::Value, _>(Some(429))
    })
    .await;

    assert!(quota.breakdown.is_empty());
    assert!(!at_wall_retry_suppressed(key, Instant::now()));
}

#[tokio::test]
async fn a_default_payload_with_fable_never_issues_the_retry() {
    let key = "gate-fable-present";
    let calls = Cell::new(0);
    let quota = parse_claude_usage(&fable_body());

    let quota = with_at_wall_retry(quota, key, Instant::now(), || {
        calls.set(calls.get() + 1);
        async { Ok(json!({})) }
    })
    .await;

    assert_eq!(calls.get(), 0);
    assert_eq!(quota.breakdown.len(), 1);
    assert!(!at_wall_retry_suppressed(key, Instant::now()));
}

#[test]
fn the_shared_cache_round_trips_through_the_public_helpers() {
    let t0 = Instant::now();
    let key = "shared-roundtrip-account";

    assert!(!at_wall_retry_suppressed(key, t0));
    record_at_wall_empty(key, t0);
    assert!(at_wall_retry_suppressed(key, t0));
}
