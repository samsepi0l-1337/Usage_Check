use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::AccountUsage;
use usage_core::fetch::agy::AgyQuotaPool;
use usage_core::models::{QuotaUsage, UsageBreakdownRow};

#[derive(Clone)]
pub(super) struct LastSuccess {
    display_name: String,
    plan: Option<String>,
    five_hour: Option<QuotaUsage>,
    week: Option<QuotaUsage>,
    /// Agy per-pool rows — restored on stale so the tray keeps the pool
    /// breakdown instead of showing an empty, inconsistent snapshot.
    pool_breakdown: Vec<AgyQuotaPool>,
    /// Per-model/per-scope extra usage rows — restored on stale, mirroring
    /// `pool_breakdown`.
    breakdown: Vec<UsageBreakdownRow>,
    /// Pro-provider secondary label (`$12 left`, `809 credits left`).
    detail_suffix: Option<String>,
}

pub(super) fn last_success_cache() -> &'static Mutex<HashMap<String, LastSuccess>> {
    static CACHE: OnceLock<Mutex<HashMap<String, LastSuccess>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Post-step applied to every assembled usage. On success, remember the good
/// windows. On a transient failure (`error`/`throttled`) with a remembered
/// success, serve the cached windows as `stale`. Non-transient statuses
/// (`needs_login`, `identity_changed`, `waiting_for_usage`) are never masked.
pub(super) fn apply_last_success(
    cache: &mut HashMap<String, LastSuccess>,
    id: &str,
    mut usage: AccountUsage,
) -> AccountUsage {
    if usage.status == "ok" {
        cache.insert(
            id.to_string(),
            LastSuccess {
                display_name: usage.display_name.clone(),
                plan: usage.plan.clone(),
                five_hour: usage.five_hour.clone(),
                week: usage.week.clone(),
                pool_breakdown: usage.pool_breakdown.clone(),
                breakdown: usage.breakdown.clone(),
                detail_suffix: usage.detail_suffix.clone(),
            },
        );
    } else if usage.status == "error" || usage.status == "throttled" {
        if let Some(previous) = cache.get(id) {
            usage.display_name = previous.display_name.clone();
            usage.plan = previous.plan.clone();
            usage.five_hour = previous.five_hour.clone();
            usage.week = previous.week.clone();
            usage.pool_breakdown = previous.pool_breakdown.clone();
            usage.breakdown = previous.breakdown.clone();
            usage.detail_suffix = previous.detail_suffix.clone();
            usage.status = "stale".to_string();
        }
    }
    usage
}

#[cfg(test)]
fn clear_last_success_cache() {
    last_success_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clear();
}

/// Evict a single account's remembered last success (called on account removal
/// so a re-added account with the same id never inherits stale quota).
pub fn evict_last_success(id: &str) {
    last_success_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(id);
}

#[cfg(test)]
#[path = "last_success_tests.rs"]
mod tests;
