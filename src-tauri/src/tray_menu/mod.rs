//! Native macOS/Windows tray menu (Docker-style) — no separate popup window.
//!
//! Menu layout:
//!   Codex
//!     ● user@example.com
//!        5h 38% · 7d 6%
//!   ────────
//!   Claude
//!     ● …
//!   ────────
//!   Antigravity (agy)
//!     ● user@…
//!        Gemini 0% · Claude+GPT 18%
//!        Gemini Models  7d 0%
//!        Claude and GPT models  7d 18%
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
pub(crate) use format::format_usage_detail;
#[allow(unused_imports)]
pub(crate) use menu::{apply_menu, build_menu, tooltip_for};

pub fn tray_id() -> &'static str {
    TRAY_ID
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
