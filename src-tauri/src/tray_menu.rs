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

use chrono::Utc;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    AppHandle, Wry,
};

use usage_core::account::Provider;
use usage_core::fetch::agy::AgyQuotaPool;
use usage_core::fetch::codex::window_label;
use usage_core::models::QuotaUsage;

use crate::edition;
use crate::poller::AccountUsage;

use usage_core::AuthMethod;

#[derive(Clone, Copy, Debug)]
pub struct AuthActionSpec {
    pub provider: Provider,
    pub method: AuthMethod,
    pub event_id: &'static str,
    pub label: &'static str,
}

pub fn auth_action_specs() -> &'static [AuthActionSpec] {
    &[
        AuthActionSpec {
            provider: Provider::Codex,
            method: AuthMethod::Cli,
            event_id: "add-codex-cli",
            label: "Add Codex (CLI)",
        },
        AuthActionSpec {
            provider: Provider::Codex,
            method: AuthMethod::BrowserOAuth,
            event_id: "add-codex-oauth",
            label: "Login Codex (browser)",
        },
        AuthActionSpec {
            provider: Provider::Claude,
            method: AuthMethod::Cli,
            event_id: "add-claude-cli",
            label: "Add Claude (CLI)",
        },
        AuthActionSpec {
            provider: Provider::Claude,
            method: AuthMethod::BrowserOAuth,
            event_id: "add-claude-oauth",
            label: "Login Claude (browser)",
        },
        AuthActionSpec {
            provider: Provider::Agy,
            method: AuthMethod::BrowserOAuth,
            event_id: "add-agy-oauth",
            label: "Login Antigravity (browser)",
        },
        #[cfg(feature = "edition-pro")]
        AuthActionSpec {
            provider: Provider::Cursor,
            method: AuthMethod::LocalDatabase,
            event_id: "add-cursor-local",
            label: "Import Cursor (local, Experimental)",
        },
        #[cfg(feature = "edition-pro")]
        AuthActionSpec {
            provider: Provider::Grok,
            method: AuthMethod::ManagementKeyClipboard,
            event_id: "add-grok-clipboard",
            label: "Import xAI API credits (clipboard)",
        },
        #[cfg(feature = "edition-pro")]
        AuthActionSpec {
            provider: Provider::Grok,
            method: AuthMethod::ManagementKeyEnvironment,
            event_id: "add-grok-env",
            label: "Import xAI API credits (env vars)",
        },
        #[cfg(feature = "edition-pro")]
        AuthActionSpec {
            provider: Provider::Higgsfield,
            method: AuthMethod::Cli,
            event_id: "add-higgsfield-cli",
            label: "Add Higgsfield (CLI)",
        },
    ]
}


const TRAY_ID: &str = "main";

fn status_dot(status: &str) -> &'static str {
    match status {
        "ok" => "●",
        "needs_login" => "○",
        _ => "◐",
    }
}

fn vendor_title(p: Provider) -> &'static str {
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

fn format_pool_detail(pool: &AgyQuotaPool) -> String {
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

fn account_name_line(usage: &AccountUsage) -> String {
    format!("  {} {}", status_dot(&usage.status), usage.display_name)
}

fn account_usage_line(usage: &AccountUsage) -> String {
    format!("     {}", format_usage_detail(usage))
}

fn append_vendor_section(
    app: &AppHandle,
    menu: &Menu<Wry>,
    provider: Provider,
    usages: &[&AccountUsage],
    first_section: &mut bool,
) -> tauri::Result<()> {
    if usages.is_empty() {
        return Ok(());
    }
    if !*first_section {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    *first_section = false;

    menu.append(&MenuItem::with_id(
        app,
        format!("cat-{}", provider.as_str()),
        vendor_title(provider),
        false,
        None::<&str>,
    )?)?;

    for usage in usages {
        menu.append(&MenuItem::with_id(
            app,
            format!("name-{}", usage.account.id),
            account_name_line(usage),
            false,
            None::<&str>,
        )?)?;
        menu.append(&MenuItem::with_id(
            app,
            format!("usage-{}", usage.account.id),
            account_usage_line(usage),
            false,
            None::<&str>,
        )?)?;
        // Agy: one indented row per Model Quota pool (Gemini / Claude+GPT).
        for (i, pool) in usage.pool_breakdown.iter().enumerate() {
            menu.append(&MenuItem::with_id(
                app,
                format!("pool-{}-{i}", usage.account.id),
                format!("        {}", format_pool_detail(pool)),
                false,
                None::<&str>,
            )?)?;
        }
    }
    Ok(())
}

/// Builds the full tray menu from the latest usage snapshot.
pub fn build_menu(app: &AppHandle, usages: &[AccountUsage]) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::new(app)?;

    if usages.is_empty() {
        menu.append(&MenuItem::with_id(
            app,
            "status-empty",
            "No accounts — add one below",
            false,
            None::<&str>,
        )?)?;
    } else {
        let order = edition::all_providers();
        let mut first_section = true;
        for provider in order {
            let group: Vec<&AccountUsage> = usages
                .iter()
                .filter(|u| u.account.provider == provider)
                .collect();
            append_vendor_section(app, &menu, provider, &group, &mut first_section)?;
        }
    }

    menu.append(&PredefinedMenuItem::separator(app)?)?;
    let add_submenu = Submenu::with_id(app, "add-account", "Add Account", true)?;
    
    for spec in auth_action_specs() {
        add_submenu.append(&MenuItem::with_id(
            app,
            spec.event_id,
            spec.label,
            true,
            None::<&str>,
        )?)?;
    }

    menu.append(&add_submenu)?;

    if !usages.is_empty() {
        let remove_submenu = Submenu::with_id(app, "remove-account", "Remove", true)?;
        for provider in edition::all_providers() {
            for usage in usages.iter().filter(|u| u.account.provider == provider) {
                let id = format!("remove-{}", usage.account.id);
                let label = format!(
                    "{} — {}",
                    vendor_title(usage.account.provider),
                    usage.display_name
                );
                remove_submenu.append(&MenuItem::with_id(app, &id, label, true, None::<&str>)?)?;
            }
        }
        menu.append(&remove_submenu)?;
    }

    menu.append(&MenuItem::with_id(
        app,
        "refresh",
        "Refresh Now",
        true,
        None::<&str>,
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        "quit",
        format!("Quit {}", edition::product_name()),
        true,
        Some("CmdOrCtrl+Q"),
    )?)?;

    Ok(menu)
}

pub fn apply_menu(app: &AppHandle, usages: &[AccountUsage]) {
    let Ok(menu) = build_menu(app, usages) else {
        eprintln!("tray: failed to build menu");
        return;
    };
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        eprintln!("tray: icon '{TRAY_ID}' not found");
        return;
    };
    if let Err(e) = tray.set_menu(Some(menu)) {
        eprintln!("tray: set_menu failed: {e}");
    }
    let _ = tray.set_tooltip(Some(tooltip_for(usages)));
}

pub fn tray_id() -> &'static str {
    TRAY_ID
}

/// Tooltip summarizing the first few accounts.
pub fn tooltip_for(usages: &[AccountUsage]) -> String {
    if usages.is_empty() {
        return format!("{} — no accounts", edition::product_name());
    }
    usages
        .iter()
        .take(3)
        .map(|u| format!("{} {}", u.display_name, format_usage_detail(u)))
        .collect::<Vec<_>>()
        .join(" · ")
}


#[cfg(test)]
mod auth_specs_tests {
    use super::*;

    #[test]
    fn test_auth_specs_no_forbidden_substrings() {
        let specs = auth_action_specs();
        let forbidden = ["Gemini (CLI)", "Antigravity (CLI)", "Cursor (CLI)", "Grok (CLI)", "SuperGrok", "Higgsfield (browser)"];
        for spec in specs {
            for forbidden_str in &forbidden {
                assert!(
                    !spec.label.contains(forbidden_str),
                    "Forbidden substring '{}' in '{}'",
                    forbidden_str,
                    spec.label
                );
            }
        }
    }
}
