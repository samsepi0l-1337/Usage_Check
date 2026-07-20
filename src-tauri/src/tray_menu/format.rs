use chrono::Utc;
use usage_core::fetch::agy::AgyQuotaPool;
use usage_core::fetch::codex::window_label;
use usage_core::models::QuotaUsage;
use usage_core::account::Provider;
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

fn relative_reset(q: &QuotaUsage) -> Option<String> {
    let resets_at = q.resets_at?;
    let secs = (resets_at - Utc::now()).num_seconds().max(0);
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    if days > 0 {
        Some(format!("{days}d {hours}h"))
    } else if hours > 0 {
        Some(format!("{hours}h {minutes}m"))
    } else {
        Some(format!("{minutes}m"))
    }
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
        if usage.totals.five_hours > 0 || usage.totals.week > 0 || usage.totals.month > 0 {
            line.push_str(" · ⟨");
            line.push_str(&format_token_windows(&usage.totals));
            line.push('⟩');
        }
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

pub(crate) fn account_usage_line(usage: &AccountUsage) -> String {
    format!("     {}", format_usage_detail(usage))
}
