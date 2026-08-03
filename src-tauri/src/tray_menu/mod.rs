//! Native macOS/Windows tray menu (Docker-style) — no separate popup window.
//!
//! Menu layout:
//!   Codex
//!     ● user@example.com
//!        1% · resets 5d 22h
//!        Spark 0%
//!   ────────
//!   Claude
//!     ● user@…
//!        5h 12% · resets 2h 16m
//!        7d 66% · resets 6d 3h
//!        Fable 28%
//!   ────────
//!   Antigravity (agy)
//!     ● user@…
//!        Gemini 7d 0% · Claude+GPT 7d 18%
//!           Gemini Models  7d 0% · resets 6d 3h
//!           Claude and GPT models  7d 18% · resets 3d 9h
//!   ────────
//!   Add Account ▸
//!   Remove ▸
//!   Refresh Now
//!   ────────
//!   Quit UsageCheck

const TRAY_ID: &str = "main";

#[cfg(test)]
use usage_core::account::Provider;
#[cfg(test)]
use usage_core::AuthMethod;

mod actions;
mod format;
mod menu;

#[allow(unused_imports)]
pub(crate) use actions::{
    add_entry_label, auth_action_specs, auth_action_specs_with, is_add_enabled,
    is_dispatch_allowed, spec_for_event, AuthActionSpec,
};
#[allow(unused_imports)]
pub(crate) use format::{
    account_usage_lines, activation_result_line, add_account_result_line, format_breakdown_row,
    format_usage_detail, license_status_line,
};
#[allow(unused_imports)]
pub(crate) use menu::{
    account_max_percent, apply_menu, build_menu, license_rows, near_limit_count,
    should_show_deactivate, tooltip_for, updated_label, LicenseRow,
};

pub fn tray_id() -> &'static str {
    TRAY_ID
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
