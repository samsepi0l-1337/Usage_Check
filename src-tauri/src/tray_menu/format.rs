use chrono::{DateTime, Utc};
use usage_core::fetch::agy::AgyQuotaPool;
use usage_core::fetch::codex::window_label;
use usage_core::models::{QuotaUsage, UsageBreakdownRow};
use usage_core::account::Provider;
use crate::license::{ActivationErrorClass, LicenseStatus};
use crate::poller::AccountUsage;

fn status_dot(status: &str) -> &'static str {
    match status {
        "ok" => "●",
        "needs_login" => "○",
        _ => "◐",
    }
}

pub(crate) fn vendor_title(p: Provider) -> &'static str {
    p.display_name()
}

fn format_tokens(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn format_percent(p: f64) -> String {
    if (p - p.round()).abs() < 0.05 {
        format!("{:.0}%", p)
    } else {
        format!("{:.1}%", p)
    }
}

/// Relative time until `at` (e.g. `"5d 22h"`, `"2h 16m"`, `"40m"`) — the
/// single formatting rule shared by every "resets"/"expires" row in the
/// tray, so a license expiry reads exactly like a quota reset.
pub(crate) fn relative_time(at: DateTime<Utc>) -> String {
    let secs = (at - Utc::now()).num_seconds().max(0);
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

fn relative_reset(q: &QuotaUsage) -> Option<String> {
    Some(relative_time(q.resets_at?))
}

/// Row 1 of the tray license section: the current [`LicenseStatus`] as a
/// single line. Never includes the key or token — `LicenseStatus` carries
/// nothing but an optional expiry timestamp.
pub(crate) fn license_status_line(status: LicenseStatus) -> String {
    match status {
        LicenseStatus::Free => "License: Free".to_string(),
        LicenseStatus::Pro { expires_at: None } => "License: Pro".to_string(),
        LicenseStatus::Pro {
            expires_at: Some(at),
        } => format!("License: Pro · expires {}", relative_time(at)),
        LicenseStatus::Expired => "License: expired — reactivate".to_string(),
        LicenseStatus::GracePeriodEnded => "License: verification needed".to_string(),
    }
}

/// Row 2 of the tray license section: the outcome of the most recent
/// activation attempt this process. Built ONLY from the coarse
/// [`ActivationErrorClass`] (H4) — never from `ActivationError`'s `Display`,
/// which for `Server{..}`/`InvalidToken` embeds server-provided or
/// attacker-influenced text — so nothing this function can produce ever
/// contains the license key, the token, or raw server text: `Result<(),
/// ActivationErrorClass>` carries no string payload at all.
pub(crate) fn activation_result_line(result: &Result<(), ActivationErrorClass>) -> String {
    let detail = match result {
        Ok(()) => "activated",
        Err(ActivationErrorClass::Network) => "no network",
        Err(ActivationErrorClass::ServerRejected) => "invalid key",
        Err(ActivationErrorClass::InvalidToken) => "invalid key",
        Err(ActivationErrorClass::DeviceMismatch) => "bound to a different device",
        Err(ActivationErrorClass::EndpointMissing) => "server not available yet",
        Err(ActivationErrorClass::Persist) => "could not save license",
        Err(ActivationErrorClass::NoStoredLicense) => "no license to refresh",
        Err(ActivationErrorClass::ReplayedToken) => "try again",
        Err(ActivationErrorClass::NotEntitled) => "key not currently valid",
        Err(ActivationErrorClass::DeviceNotPersisted) => "could not save device id",
    };
    format!("Last attempt: {detail}")
}

fn format_quota_window(q: &QuotaUsage, fallback_label: &str) -> String {
    let label = window_label(q.window_seconds, fallback_label);
    format!("{label} {}", format_percent(q.percent))
}

fn format_token_windows(totals: &usage_core::models::WindowTotals) -> String {
    format!(
        "5h {} · 7d {}",
        format_tokens(totals.five_hours),
        format_tokens(totals.week)
    )
}

fn short_pool_name(name: &str) -> &str {
    let l = name.to_ascii_lowercase();
    if l.contains("gemini") {
        "Gemini"
    } else if l.contains("claude") || l.contains("gpt") {
        "Claude+GPT"
    } else {
        name
    }
}

fn format_pool_compact(pool: &AgyQuotaPool) -> String {
    let mut parts = Vec::new();
    if let Some(q) = &pool.five_hour {
        parts.push(format_quota_window(q, "5h"));
    }
    if let Some(q) = &pool.week {
        parts.push(format_quota_window(q, "7d"));
    }
    if parts.is_empty() {
        return format!("{} —", short_pool_name(&pool.name));
    }
    format!("{} {}", short_pool_name(&pool.name), parts.join(" · "))
}

pub(crate) fn format_pool_detail(pool: &AgyQuotaPool) -> String {
    let mut parts = Vec::new();
    if let Some(q) = &pool.five_hour {
        parts.push(format_quota_window(q, "5h"));
    }
    if let Some(q) = &pool.week {
        parts.push(format_quota_window(q, "7d"));
    }
    let mut line = format!("{}  {}", pool.name, parts.join(" · "));
    if let Some(reset) = pool
        .five_hour
        .as_ref()
        .and_then(relative_reset)
        .or_else(|| pool.week.as_ref().and_then(relative_reset))
    {
        line.push_str(" · resets ");
        line.push_str(&reset);
    }
    line
}

/// Usage detail line under the account name.
pub fn format_usage_detail(usage: &AccountUsage) -> String {
    if !usage.pool_breakdown.is_empty() {
        let mut line = usage
            .pool_breakdown
            .iter()
            .map(format_pool_compact)
            .collect::<Vec<_>>()
            .join(" · ");
        if usage.status != "ok" {
            line.push_str(" (");
            line.push_str(&usage.status);
            line.push(')');
        }
        return line;
    }

    let has_five = usage.five_hour.is_some();
    let has_week = usage.week.is_some();
    let mut line = if has_five && has_week {
        let mut parts = Vec::new();
        if let Some(q) = &usage.five_hour {
            parts.push(format_quota_window(q, "5h"));
        } else {
            parts.push("5h —".into());
        }
        if let Some(q) = &usage.week {
            parts.push(format_quota_window(q, "7d"));
        } else {
            parts.push("7d —".into());
        }
        parts.join(" · ")
    } else if let Some(q) = usage.week.as_ref().or(usage.five_hour.as_ref()) {
        let mut single = format_percent(q.percent);
        if let Some(suffix) = &usage.detail_suffix {
            single.push_str(" · ");
            single.push_str(suffix);
        }
        single
    } else if let Some(suffix) = &usage.detail_suffix {
        suffix.clone()
    } else if has_five || has_week {
        "—".into()
    } else {
        format_token_windows(&usage.totals)
    };

    if has_five || has_week {
        if let Some(reset) = usage
            .five_hour
            .as_ref()
            .and_then(relative_reset)
            .or_else(|| usage.week.as_ref().and_then(relative_reset))
        {
            line.push_str(" · resets ");
            line.push_str(&reset);
        }
    }
    if usage.status != "ok" {
        line.push_str(" (");
        line.push_str(&usage.status);
        line.push(')');
    }
    if let Some(local_status) = usage.local_status.as_deref() {
        line.push_str(" (local: ");
        line.push_str(local_status);
        line.push(')');
    }
    line
}

pub(crate) fn account_name_line(usage: &AccountUsage) -> String {
    format!("  {} {}", status_dot(&usage.status), usage.display_name)
}

/// Usage row(s) for an account, indented to match the account name line.
///
/// When an account has BOTH a 5h and a 7d window (Claude today), each window
/// gets its own row with its own reset time. Every other case (single
/// window, `detail_suffix`-only, agy `pool_breakdown`, or the token-totals
/// fallback) collapses to the single `format_usage_detail` line, unchanged.
pub(crate) fn account_usage_lines(usage: &AccountUsage) -> Vec<String> {
    if usage.pool_breakdown.is_empty() {
        if let (Some(five), Some(week)) = (&usage.five_hour, &usage.week) {
            let mut row_5h = format!("     {}", format_quota_window(five, "5h"));
            if let Some(reset) = relative_reset(five) {
                row_5h.push_str(" · resets ");
                row_5h.push_str(&reset);
            }
            let mut row_7d = format!("     {}", format_quota_window(week, "7d"));
            if let Some(reset) = relative_reset(week) {
                row_7d.push_str(" · resets ");
                row_7d.push_str(&reset);
            }
            if usage.status != "ok" {
                row_7d.push_str(" (");
                row_7d.push_str(&usage.status);
                row_7d.push(')');
            }
            if let Some(local_status) = usage.local_status.as_deref() {
                row_7d.push_str(" (local: ");
                row_7d.push_str(local_status);
                row_7d.push(')');
            }
            return vec![row_5h, row_7d];
        }
    }
    vec![format!("     {}", format_usage_detail(usage))]
}

/// Formats a single per-model breakdown row, e.g. "Fable 28%". No reset
/// suffix — the primary usage line above it already shows the reset.
pub(crate) fn format_breakdown_row(row: &UsageBreakdownRow) -> String {
    format!("{} {}", row.label, format_percent(row.usage.percent))
}
