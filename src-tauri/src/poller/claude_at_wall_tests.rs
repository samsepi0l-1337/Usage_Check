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
    assert_eq!(
        calls.get(),
        1,
        "a suppressed account must not issue the at_wall request"
    );
}

#[tokio::test]
async fn an_http_error_on_retry_starts_a_backoff_and_the_next_poll_skips_the_fetch() {
    for (key, status) in [("gate-error-429", Some(429)), ("gate-error-other", None)] {
        let t0 = Instant::now();
        let calls = Cell::new(0);

        let quota = with_at_wall_retry(empty_quota(), key, t0, || {
            calls.set(calls.get() + 1);
            async move { Err::<serde_json::Value, _>(status) }
        })
        .await;
        assert!(quota.breakdown.is_empty());
        assert_eq!(calls.get(), 1);
        assert!(at_wall_retry_suppressed(key, t0));

        let _ = with_at_wall_retry(empty_quota(), key, t0, || {
            calls.set(calls.get() + 1);
            async move { Err::<serde_json::Value, _>(status) }
        })
        .await;
        assert_eq!(calls.get(), 1, "a backed-off account must not be retried");
    }
}

#[tokio::test]
async fn a_successful_retry_after_the_backoff_clears_the_error_state() {
    let key = "gate-error-then-fable";
    let t0 = Instant::now();
    let _ = with_at_wall_retry(empty_quota(), key, t0, || async {
        Err::<serde_json::Value, _>(Some(429))
    })
    .await;
    assert!(at_wall_retry_suppressed(key, t0));
    let t1 = t0 + AT_WALL_ERROR_BASE;
    assert!(!at_wall_retry_suppressed(key, t1));

    let quota = with_at_wall_retry(empty_quota(), key, t1, || async { Ok(fable_body()) }).await;

    assert_eq!(quota.breakdown.len(), 1);
    assert!(!at_wall_retry_suppressed(key, t1));
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

#[test]
fn an_error_backs_off_for_the_base_delay_then_allows_a_retry() {
    let mut cache = AtWallNegativeCache::default();
    let t0 = Instant::now();

    cache.record_error("acct-a", t0);

    assert!(cache.is_suppressed("acct-a", t0));
    assert!(cache.is_suppressed("acct-a", t0 + AT_WALL_ERROR_BASE - Duration::from_secs(1)));
    assert!(!cache.is_suppressed("acct-a", t0 + AT_WALL_ERROR_BASE));
}

#[test]
fn consecutive_errors_double_the_backoff() {
    let mut cache = AtWallNegativeCache::default();
    let t0 = Instant::now();
    cache.record_error("acct-a", t0);

    let t1 = t0 + AT_WALL_ERROR_BASE;
    cache.record_error("acct-a", t1);
    assert!(cache.is_suppressed(
        "acct-a",
        t1 + AT_WALL_ERROR_BASE * 2 - Duration::from_secs(1)
    ));
    assert!(!cache.is_suppressed("acct-a", t1 + AT_WALL_ERROR_BASE * 2));

    let t2 = t1 + AT_WALL_ERROR_BASE * 2;
    cache.record_error("acct-a", t2);
    assert!(cache.is_suppressed(
        "acct-a",
        t2 + AT_WALL_ERROR_BASE * 4 - Duration::from_secs(1)
    ));
    assert!(!cache.is_suppressed("acct-a", t2 + AT_WALL_ERROR_BASE * 4));
}

#[test]
fn the_error_backoff_is_capped_and_never_overflows() {
    let mut cache = AtWallNegativeCache::default();
    let mut now = Instant::now();
    for _ in 0..200 {
        cache.record_error("acct-a", now);
        now += AT_WALL_ERROR_CAP;
    }
    let last = now - AT_WALL_ERROR_CAP;

    assert!(cache.is_suppressed("acct-a", last + AT_WALL_ERROR_CAP - Duration::from_secs(1)));
    assert!(!cache.is_suppressed("acct-a", last + AT_WALL_ERROR_CAP));
}

#[test]
fn clearing_the_error_resets_the_backoff_sequence() {
    let mut cache = AtWallNegativeCache::default();
    let t0 = Instant::now();
    cache.record_error("acct-a", t0);
    cache.record_error("acct-a", t0 + AT_WALL_ERROR_BASE);

    cache.clear_error("acct-a");
    let t2 = t0 + AT_WALL_ERROR_BASE * 4;
    cache.record_error("acct-a", t2);

    assert!(cache.is_suppressed("acct-a", t2));
    assert!(!cache.is_suppressed("acct-a", t2 + AT_WALL_ERROR_BASE));
}

#[test]
fn the_error_backoff_is_per_account() {
    let mut cache = AtWallNegativeCache::default();
    let t0 = Instant::now();

    cache.record_error("acct-a", t0);

    assert!(cache.is_suppressed("acct-a", t0));
    assert!(!cache.is_suppressed("acct-b", t0));
}
