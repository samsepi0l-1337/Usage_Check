//! `GET /health` body rendering: service status enriched with snapshot
//! staleness and a per-status account breakdown, for monitoring consumers.
//!
//! Kept in a sibling module so `api.rs` stays within the file-size budget and
//! the aggregation is unit-testable without a running server. The impure
//! `Utc::now()` read stays in `api.rs`; everything here is pure.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Serialize)]
struct HealthDto<'a> {
    status: &'a str,
    version: &'a str,
    updated_at: Option<DateTime<Utc>>,
    /// Age of the served snapshot in seconds, or null before the first poll.
    stale_seconds: Option<i64>,
    account_count: usize,
    /// Count of accounts by their `status` string (e.g. ok/stale/needs_login).
    status_counts: BTreeMap<String, usize>,
}

/// Seconds between `updated_at` and `now` (never negative), or `None` when the
/// snapshot has not been published yet.
fn stale_seconds(updated_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Option<i64> {
    updated_at.map(|ts| (now - ts).num_seconds().max(0))
}

/// Folds account status strings into a stable, sorted count map.
fn status_counts(statuses: &[&str]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for s in statuses {
        *counts.entry((*s).to_string()).or_insert(0) += 1;
    }
    counts
}

/// Renders the `/health` JSON body.
pub fn health_body(
    version: &str,
    updated_at: Option<DateTime<Utc>>,
    statuses: &[&str],
    now: DateTime<Utc>,
) -> String {
    let dto = HealthDto {
        status: "ok",
        version,
        updated_at,
        stale_seconds: stale_seconds(updated_at, now),
        account_count: statuses.len(),
        status_counts: status_counts(statuses),
    };
    serde_json::to_string(&dto).unwrap_or_else(|_| r#"{"status":"ok"}"#.to_string())
}

#[cfg(test)]
#[path = "api_health_tests.rs"]
mod tests;
