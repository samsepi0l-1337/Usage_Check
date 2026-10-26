//! In-memory negative cache for Claude's `at_wall` usage retry.
//!
//! The default usage payload is fetched first. When it has no Fable row, the
//! `at_wall` URL is requested as a second call. An account whose retry also
//! comes back without a Fable row is recorded here, so later polls skip that
//! second call until the entry expires instead of doubling the usage API
//! requests on every poll. A retry that fails (HTTP 429 or any other error)
//! starts a per-account exponential backoff, so a rate-limited account is not
//! hit again on every poll either.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use usage_core::fetch::claude::{parse_claude_usage, ClaudeQuota};

/// How long an account's empty `at_wall` result suppresses the retry.
pub(super) const AT_WALL_EMPTY_TTL: Duration = Duration::from_secs(10 * 60);

/// First backoff after a failed `at_wall` retry; doubles per consecutive failure.
pub(super) const AT_WALL_ERROR_BASE: Duration = Duration::from_secs(30);

/// Upper bound for the error backoff (same cap as the empty-result TTL).
pub(super) const AT_WALL_ERROR_CAP: Duration = AT_WALL_EMPTY_TTL;

/// Consecutive `at_wall` failures for one account and when its backoff ends.
struct ErrorBackoff {
    failures: u32,
    until: Instant,
}

/// Per-account expiry instants for empty `at_wall` results and error backoffs.
#[derive(Default)]
pub(super) struct AtWallNegativeCache {
    empty_until: HashMap<String, Instant>,
    errors: HashMap<String, ErrorBackoff>,
}

/// Backoff after `failures` consecutive errors: base, 2x, 4x, ... capped.
fn error_delay(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    AT_WALL_ERROR_BASE
        .saturating_mul(1u32 << doublings)
        .min(AT_WALL_ERROR_CAP)
}

impl AtWallNegativeCache {
    pub(super) fn is_suppressed(&self, account_key: &str, now: Instant) -> bool {
        self.empty_until
            .get(account_key)
            .is_some_and(|until| now < *until)
            || self
                .errors
                .get(account_key)
                .is_some_and(|backoff| now < backoff.until)
    }

    /// Records a failed retry and doubles the account's backoff (capped).
    pub(super) fn record_error(&mut self, account_key: &str, now: Instant) {
        self.errors
            .retain(|_, backoff| now < backoff.until + AT_WALL_ERROR_CAP);
        let failures = self
            .errors
            .get(account_key)
            .map_or(1, |backoff| backoff.failures.saturating_add(1));
        self.errors.insert(
            account_key.to_string(),
            ErrorBackoff {
                failures,
                until: now + error_delay(failures),
            },
        );
    }

    /// Forgets the account's failures after a successful retry.
    pub(super) fn clear_error(&mut self, account_key: &str) {
        self.errors.remove(account_key);
    }

    pub(super) fn record_empty(&mut self, account_key: &str, now: Instant) {
        self.empty_until.retain(|_, until| now < *until);
        self.empty_until
            .insert(account_key.to_string(), now + AT_WALL_EMPTY_TTL);
    }
}

fn shared_cache() -> &'static Mutex<AtWallNegativeCache> {
    static CACHE: OnceLock<Mutex<AtWallNegativeCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(AtWallNegativeCache::default()))
}

/// Whether the `at_wall` retry for `account_key` is suppressed at `now`.
pub(super) fn at_wall_retry_suppressed(account_key: &str, now: Instant) -> bool {
    shared_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_suppressed(account_key, now)
}

/// Records that the `at_wall` retry for `account_key` failed.
pub(super) fn record_at_wall_error(account_key: &str, now: Instant) {
    shared_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .record_error(account_key, now);
}

/// Clears the failure streak after a successful `at_wall` retry.
pub(super) fn clear_at_wall_error(account_key: &str) {
    shared_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear_error(account_key);
}

/// Records that the `at_wall` retry for `account_key` returned no Fable row.
pub(super) fn record_at_wall_empty(account_key: &str, now: Instant) {
    shared_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .record_empty(account_key, now);
}

/// Applies the `at_wall` retry to a default-payload quota. The retry runs
/// only when the default payload has no Fable row and the account is not
/// suppressed. An empty successful retry records suppression; a Fable row
/// merges in; an error leaves the quota unchanged and starts (or extends) the
/// account's capped exponential backoff. Any successful retry resets the
/// failure streak.
pub(super) async fn with_at_wall_retry<F, Fut>(
    mut quota: ClaudeQuota,
    account_key: &str,
    now: Instant,
    fetch_at_wall: F,
) -> ClaudeQuota
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<serde_json::Value, Option<u16>>>,
{
    if !quota.breakdown.is_empty() || at_wall_retry_suppressed(account_key, now) {
        return quota;
    }
    match fetch_at_wall().await {
        Ok(body) => {
            clear_at_wall_error(account_key);
            let extra = parse_claude_usage(&body);
            if extra.breakdown.is_empty() {
                record_at_wall_empty(account_key, now);
            } else {
                quota.breakdown = extra.breakdown;
            }
        }
        Err(_) => record_at_wall_error(account_key, now),
    }
    quota
}

#[cfg(test)]
#[path = "claude_at_wall_tests.rs"]
mod tests;
