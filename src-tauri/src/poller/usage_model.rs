use serde::Serialize;

use usage_core::account::{Account, Provider};
#[cfg(test)]
use usage_core::account::AuthSource;
use usage_core::fetch::agy::{compact_windows, AgyQuota, AgyQuotaPool};
use usage_core::fetch::claude::ClaudeQuota;
use usage_core::fetch::codex::CodexQuota;
#[cfg(feature = "edition-pro")]
use usage_core::fetch::cursor::CursorQuota;
#[cfg(feature = "edition-pro")]
use usage_core::fetch::grok::GrokPrepaid;
#[cfg(feature = "edition-pro")]
use usage_core::fetch::higgsfield::HiggsfieldCredits;
use usage_core::models::{LocalProvenance, LocalUsage, QuotaUsage, WindowTotals};

/// A single account's usage snapshot: live quota (when available) plus
/// local-log token totals (Codex/Claude fallback only), ready for the tray.
#[derive(Clone, Debug, Serialize)]
pub struct AccountUsage {
    pub account: Account,
    /// Prefer email / plan-aware name over the stored label when available.
    pub display_name: String,
    pub plan: Option<String>,
    pub five_hour: Option<QuotaUsage>,
    pub week: Option<QuotaUsage>,
    pub totals: WindowTotals,
    /// Agy: Gemini / Claude+GPT pool rows (used %, like Codex/Claude).
    pub pool_breakdown: Vec<AgyQuotaPool>,
    /// Pro providers: secondary label (`$12 left`, `809 credits left`).
    pub detail_suffix: Option<String>,
    pub status: String,
    /// Local-aggregation provenance label when the locally-summed token totals
    /// are not a clean `Ok` (e.g. `unavailable`, `partial`, `truncated`,
    /// `ambiguous`, `conflict`, `assumed`, `no_local_profile`). `None` means the
    /// local totals are fully trustworthy. Lets the tray/DTO distinguish a real
    /// zero from a failed/ambiguous local scan.
    pub local_status: Option<String>,
}

pub(super) fn display_name_for(account: &Account, email: Option<&str>, plan: Option<&str>) -> String {
    if let Some(email) = email.filter(|s| !s.is_empty()) {
        return email.to_string();
    }
    let label = account.label.trim();
    if !label.is_empty()
        && !label.ends_with(" (CLI import)")
        && !label.ends_with(" account")
        && label != "agy (local logs)"
        && label != "Gemini (local logs)"
        && label != "Gemini"
        && label != "agy"
    {
        return label.to_string();
    }
    if let Some(plan) = plan.filter(|s| !s.is_empty()) {
        return format!("{} · {}", provider_short(account.provider), plan);
    }
    if !label.is_empty()
        && label != "Gemini"
        && label != "agy"
        && label != "agy (local logs)"
        && label != "Gemini (local logs)"
    {
        return label.to_string();
    }
    provider_short(account.provider).to_string()
}

fn provider_short(p: Provider) -> &'static str {
    p.display_name()
}

/// Maps Antigravity Model Quota pools into tray `AccountUsage`.
pub fn account_usage_from_agy(account: &Account, quota: &AgyQuota, status: &str) -> AccountUsage {
    let (five_hour, week) = compact_windows(&quota.pools);
    AccountUsage {
        display_name: display_name_for(account, quota.email.as_deref(), quota.plan.as_deref()),
        plan: quota.plan.clone(),
        account: account.clone(),
        five_hour,
        week,
        totals: WindowTotals::default(),
        pool_breakdown: quota.pools.clone(),
        detail_suffix: None,
        status: status.to_string(),
        local_status: None,
    }
}

#[cfg(feature = "edition-pro")]
pub(super) fn account_usage_from_cursor(account: &Account, quota: &CursorQuota, status: &str) -> AccountUsage {
    AccountUsage {
        display_name: display_name_for(account, quota.email.as_deref(), quota.plan.as_deref()),
        plan: quota.plan.clone(),
        account: account.clone(),
        five_hour: None,
        week: quota.period.clone(),
        totals: WindowTotals::default(),
        pool_breakdown: Vec::new(),
        detail_suffix: quota.detail_suffix.clone(),
        status: status.to_string(),
        local_status: None,
    }
}

#[cfg(feature = "edition-pro")]
pub(super) fn account_usage_from_grok(account: &Account, prepaid: &GrokPrepaid, status: &str) -> AccountUsage {
    AccountUsage {
        display_name: display_name_for(account, None, Some("API credits")),
        plan: Some("API credits".into()),
        account: account.clone(),
        five_hour: None,
        week: prepaid.period.clone(),
        totals: WindowTotals::default(),
        pool_breakdown: Vec::new(),
        detail_suffix: prepaid.detail_suffix.clone(),
        status: status.to_string(),
        local_status: None,
    }
}

#[cfg(feature = "edition-pro")]
pub(super) fn account_usage_from_higgsfield(
    account: &Account,
    credits: &HiggsfieldCredits,
    status: &str,
) -> AccountUsage {
    AccountUsage {
        display_name: display_name_for(account, credits.email.as_deref(), credits.plan.as_deref()),
        plan: credits.plan.clone(),
        account: account.clone(),
        five_hour: None,
        week: credits.to_quota(),
        totals: WindowTotals::default(),
        pool_breakdown: Vec::new(),
        detail_suffix: credits.detail_suffix(),
        status: status.to_string(),
        local_status: None,
    }
}

/// Maps an HTTP failure to a status string: 401/403 (expired/invalid token)
/// -> "needs_login", 429 -> "rate_limited", anything else -> "error".
pub(super) fn status_for_failure(status: Option<u16>) -> &'static str {
    match status {
        Some(401) | Some(403) => "needs_login",
        Some(429) => "rate_limited",
        _ => "error",
    }
}

pub(super) fn codex_fetch_outcome(quota: CodexQuota) -> FetchOutcome {
    FetchOutcome::Live {
        five_hour: quota.five_hour,
        week: quota.week,
        plan: quota.plan,
        email: quota.email,
    }
}

pub(super) fn claude_fetch_outcome(quota: ClaudeQuota, email: Option<&str>) -> FetchOutcome {
    FetchOutcome::Live {
        five_hour: quota.five_hour,
        week: quota.week,
        plan: None,
        email: email.map(str::to_string),
    }
}

pub(super) fn codex_identity_status(probe_identity: &str, expected: &str) -> Option<&'static str> {
    if probe_identity == expected {
        None
    } else {
        Some("identity_changed")
    }
}

pub(super) fn claude_identity_status(snapshot_identity: &str, expected: &str) -> Option<&'static str> {
    if snapshot_identity != expected {
        Some("identity_changed")
    } else {
        None
    }
}

/// Outcome of a provider HTTP fetch.
#[derive(Clone, Debug)]
pub enum FetchOutcome {
    Live {
        five_hour: Option<QuotaUsage>,
        week: Option<QuotaUsage>,
        plan: Option<String>,
        email: Option<String>,
    },
    Failed {
        status: Option<u16>,
    },
}

/// Assemble a single AccountUsage from account, fetch outcome, and local usage.
pub fn assemble_account_usage(
    account: &Account,
    outcome: FetchOutcome,
    local: LocalUsage,
) -> AccountUsage {
    let local_status = local_status_label(local.provenance).map(str::to_string);
    let (five_hour, week, plan, email, status) = match outcome {
        FetchOutcome::Live {
            five_hour,
            week,
            plan,
            email,
        } => {
            // A 2xx with no usable quota windows (empty/unparseable body, schema
            // drift) must not masquerade as a healthy "ok" with blank bars —
            // report it as `waiting_for_usage`, mirroring the CLI-snapshot path.
            let status = if five_hour.is_none() && week.is_none() {
                "waiting_for_usage"
            } else {
                "ok"
            };
            (five_hour, week, plan, email, status.to_string())
        }
        FetchOutcome::Failed { status } => (
            None,
            None,
            None,
            None,
            status_for_failure(status).to_string(),
        ),
    };
    let mut account = account.clone();
    if let Some(email) = email.as_deref().filter(|email| !email.is_empty()) {
        account.label = email.to_string();
    }

    AccountUsage {
        display_name: display_name_for(&account, email.as_deref(), plan.as_deref()),
        plan,
        account,
        five_hour,
        week,
        totals: local.totals,
        pool_breakdown: Vec::new(),
        detail_suffix: None,
        status,
        local_status,
    }
}

fn local_status_label(provenance: LocalProvenance) -> Option<&'static str> {
    match provenance {
        LocalProvenance::Ok | LocalProvenance::NoEvents | LocalProvenance::SharedProfileOther => {
            None
        }
        LocalProvenance::NoLocalProfile => Some("no_local_profile"),
        LocalProvenance::Assumed => Some("assumed"),
        LocalProvenance::Ambiguous => Some("ambiguous"),
        LocalProvenance::Conflict => Some("conflict"),
        LocalProvenance::Partial => Some("partial"),
        LocalProvenance::Unavailable => Some("unavailable"),
        LocalProvenance::Truncated => Some("truncated"),
    }
}

#[cfg(test)]
#[path = "usage_model_tests.rs"]
mod tests;
