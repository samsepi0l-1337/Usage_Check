//! Prometheus text-exposition (`text/plain; version=0.0.4`) rendering of the
//! current usage snapshot, served at `GET /metrics`.
//!
//! Kept in a sibling module so `api.rs` stays within the file-size budget and
//! the pure rendering is unit-testable without a running server.

use crate::api::UsageResponse;

/// Content-Type for the Prometheus text exposition format.
pub const METRICS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Escapes a Prometheus label value: backslash, double-quote, and newline.
fn escape_label(v: &str) -> String {
    v.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

/// Appends one `usagecheck_used_percent` gauge line. Non-finite values (NaN /
/// ±inf) are skipped so the exposition stays valid for scrapers.
fn push_metric(
    out: &mut String,
    provider: &str,
    account: &str,
    pool: Option<&str>,
    window: &str,
    value: f64,
) {
    if !value.is_finite() {
        return;
    }
    let pool_label = match pool {
        Some(p) => format!(",pool=\"{}\"", escape_label(p)),
        None => String::new(),
    };
    out.push_str(&format!(
        "usagecheck_used_percent{{provider=\"{}\",account=\"{}\"{},window=\"{}\"}} {}\n",
        escape_label(provider),
        escape_label(account),
        pool_label,
        window,
        value,
    ));
}

/// Renders the usage snapshot as Prometheus text-format metrics: an
/// `usagecheck_account_count` gauge plus one `usagecheck_used_percent` gauge
/// per account/window (and per agy pool/window), labeled by provider/account.
pub fn metrics_body(resp: &UsageResponse) -> String {
    let mut out = String::new();
    out.push_str("# HELP usagecheck_account_count Number of accounts in the snapshot.\n");
    out.push_str("# TYPE usagecheck_account_count gauge\n");
    out.push_str(&format!("usagecheck_account_count {}\n", resp.count));
    out.push_str(
        "# HELP usagecheck_used_percent Percent of a provider quota window consumed (0-100).\n",
    );
    out.push_str("# TYPE usagecheck_used_percent gauge\n");
    for a in &resp.accounts {
        let (provider, account) = (a.provider.as_str(), a.display_name.as_str());
        if let Some(q) = &a.five_hour {
            push_metric(&mut out, provider, account, None, "5h", q.used_percent);
        }
        if let Some(q) = &a.week {
            push_metric(&mut out, provider, account, None, "7d", q.used_percent);
        }
        for pool in &a.pools {
            if let Some(q) = &pool.five_hour {
                push_metric(&mut out, provider, account, Some(&pool.name), "5h", q.used_percent);
            }
            if let Some(q) = &pool.week {
                push_metric(&mut out, provider, account, Some(&pool.name), "7d", q.used_percent);
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "api_metrics_tests.rs"]
mod tests;
