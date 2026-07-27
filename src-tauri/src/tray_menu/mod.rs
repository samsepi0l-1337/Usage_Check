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
pub(crate) use actions::{AuthActionSpec, auth_action_specs, spec_for_event};
#[allow(unused_imports)]
pub(crate) use format::{account_usage_lines, format_breakdown_row, format_usage_detail};
#[allow(unused_imports)]
pub(crate) use menu::{
    account_max_percent, apply_menu, build_menu, near_limit_count, tooltip_for, updated_label,
};

pub fn tray_id() -> &'static str {
    TRAY_ID
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
