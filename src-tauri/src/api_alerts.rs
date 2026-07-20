//! `GET /v1/alerts`: accounts/windows at or above an alert threshold, so a
//! consumer can poll for "who is near their quota limit" instead of computing
//! it from the full usage payload.
//!
//! Threshold via `USAGECHECK_ALERT_THRESHOLD` (percent, default 90). Kept in a
//! sibling module so `api.rs` stays within the file-size budget.

use serde::Serialize;
use usage_core::account::Provider;

use crate::api::UsageResponse;

/// Default near-limit threshold in percent.
const DEFAULT_ALERT_THRESHOLD: f64 = 90.0;

/// Resolves the alert threshold from `raw` (the `USAGECHECK_ALERT_THRESHOLD`
/// env value): a finite number clamped to `[0, 100]`, else [`DEFAULT_ALERT_THRESHOLD`].
pub(crate) fn alert_threshold(raw: Option<&str>) -> f64 {
    raw.and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .map(|v| v.clamp(0.0, 100.0))
        .unwrap_or(DEFAULT_ALERT_THRESHOLD)
}

/// Resolves the alert threshold from the live `USAGECHECK_ALERT_THRESHOLD` env
/// value. Single source shared by the `/v1/alerts` route and the tray banner.
pub(crate) fn current_alert_threshold() -> f64 {
    alert_threshold(std::env::var("USAGECHECK_ALERT_THRESHOLD").ok().as_deref())
}

#[derive(Serialize)]
struct Alert<'a> {
    provider: Provider,
    account: &'a str,
    window: &'a str,
    pool: Option<&'a str>,
    used_percent: f64,
}

#[derive(Serialize)]
pub struct AlertsResponse<'a> {
    threshold: f64,
    count: usize,
    alerts: Vec<Alert<'a>>,
}

/// Collects every account/window (and agy pool/window) whose used-percent is at
/// or above `threshold`. Non-finite percents never trip the `>=` comparison.
pub fn alerts_response(resp: &UsageResponse, threshold: f64) -> AlertsResponse<'_> {
    let mut alerts = Vec::new();
    for a in &resp.accounts {
        let account = a.display_name.as_str();
        // Account-level windows (pool = None), then each agy pool's windows.
        let account_windows = [
            ("5h", a.five_hour.as_ref()),
            ("7d", a.week.as_ref()),
        ];
        for (window, quota) in account_windows {
            if let Some(q) = quota {
                if q.used_percent >= threshold {
                    alerts.push(Alert {
                        provider: a.provider,
                        account,
                        window,
                        pool: None,
                        used_percent: q.used_percent,
                    });
                }
            }
        }
        for pool in &a.pools {
            let pool_windows = [
                ("5h", pool.five_hour.as_ref()),
                ("7d", pool.week.as_ref()),
            ];
            for (window, quota) in pool_windows {
                if let Some(q) = quota {
                    if q.used_percent >= threshold {
                        alerts.push(Alert {
                            provider: a.provider,
                            account,
                            window,
                            pool: Some(&pool.name),
                            used_percent: q.used_percent,
                        });
                    }
                }
            }
        }
    }
    AlertsResponse {
        threshold,
        count: alerts.len(),
        alerts,
    }
}

#[cfg(test)]
#[path = "api_alerts_tests.rs"]
mod tests;
