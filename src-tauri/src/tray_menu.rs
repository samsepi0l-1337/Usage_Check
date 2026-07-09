//! Native macOS/Windows tray menu (Docker-style) — no separate popup window.
//!
//! Menu layout:
//!   Codex
//!     ● user@example.com
//!        5h 38% · 7d 6%
//!     ● other@example.com
//!        5h 12% · 7d 40%
//!   ────────
//!   Claude
//!     ● …
//!   ────────
//!   Gemini
//!     ● …
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
use usage_core::fetch::codex::window_label;
use usage_core::models::QuotaUsage;

use crate::poller::AccountUsage;

const TRAY_ID: &str = "main";

fn status_dot(status: &str) -> &'static str {
    match status {
        "ok" => "●",
        "needs_login" => "○",
        _ => "◐",
    }
}

fn vendor_title(p: Provider) -> &'static str {
    match p {
        Provider::Codex => "Codex",
        Provider::Claude => "Claude",
        Provider::Agy => "Gemini",
    }
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
    let secs = (resets_at - Utc::now()).num_seconds().max(0) as i64;
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

/// Usage detail line under the account name.
pub fn format_usage_detail(usage: &AccountUsage) -> String {
    let has_quota = usage.five_hour.is_some() || usage.week.is_some();
    let mut parts = Vec::new();
    if has_quota {
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
    } else {
        parts.push(format!("5h {}", format_tokens(usage.totals.five_hours)));
        parts.push(format!("7d {}", format_tokens(usage.totals.week)));
    }

    let mut line = parts.join(" · ");
    if let Some(reset) = usage
        .five_hour
        .as_ref()
        .and_then(relative_reset)
        .or_else(|| usage.week.as_ref().and_then(relative_reset))
    {
        line.push_str(" · resets ");
        line.push_str(&reset);
    }
    if usage.status != "ok" {
        line.push_str(" (");
        line.push_str(&usage.status);
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
        let order = [Provider::Codex, Provider::Claude, Provider::Agy];
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
    add_submenu.append(&MenuItem::with_id(
        app,
        "add-codex-cli",
        "Import Codex (CLI)",
        true,
        None::<&str>,
    )?)?;
    add_submenu.append(&MenuItem::with_id(
        app,
        "add-claude-cli",
        "Import Claude (CLI)",
        true,
        None::<&str>,
    )?)?;
    add_submenu.append(&MenuItem::with_id(
        app,
        "add-agy",
        "Add Gemini (local logs)",
        true,
        None::<&str>,
    )?)?;
    add_submenu.append(&PredefinedMenuItem::separator(app)?)?;
    add_submenu.append(&MenuItem::with_id(
        app,
        "add-codex-oauth",
        "Login Codex (browser)",
        true,
        None::<&str>,
    )?)?;
    add_submenu.append(&MenuItem::with_id(
        app,
        "add-claude-oauth",
        "Login Claude (browser)",
        true,
        None::<&str>,
    )?)?;
    menu.append(&add_submenu)?;

    if !usages.is_empty() {
        let remove_submenu = Submenu::with_id(app, "remove-account", "Remove", true)?;
        for provider in [Provider::Codex, Provider::Claude, Provider::Agy] {
            for usage in usages.iter().filter(|u| u.account.provider == provider) {
                let id = format!("remove-{}", usage.account.id);
                let label = format!(
                    "{} — {}",
                    vendor_title(usage.account.provider),
                    usage.display_name
                );
                remove_submenu.append(&MenuItem::with_id(
                    app,
                    &id,
                    label,
                    true,
                    None::<&str>,
                )?)?;
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
        "Quit UsageCheck",
        true,
        Some("CmdOrCtrl+Q"),
    )?)?;

    Ok(menu)
}

/// Rebuilds and applies the tray menu + tooltip from `usages`.
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
        return "UsageCheck — no accounts".into();
    }
    usages
        .iter()
        .take(3)
        .map(|u| format!("{} {}", u.display_name, format_usage_detail(u)))
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use usage_core::account::Account;
    use usage_core::models::WindowTotals;

    fn sample(provider: Provider, name: &str, five: f64, week: f64) -> AccountUsage {
        AccountUsage {
            account: Account {
                id: name.into(),
                provider,
                label: name.into(),
            },
            display_name: name.into(),
            plan: None,
            five_hour: Some(QuotaUsage {
                percent: five,
                resets_at: None,
                window_seconds: Some(18_000),
            }),
            week: Some(QuotaUsage {
                percent: week,
                resets_at: None,
                window_seconds: Some(604_800),
            }),
            totals: WindowTotals::default(),
            status: "ok".into(),
        }
    }

    #[test]
    fn usage_detail_uses_window_labels() {
        let u = sample(Provider::Codex, "a@b.com", 38.0, 6.0);
        let detail = format_usage_detail(&u);
        assert!(detail.contains("5h 38%"), "{detail}");
        assert!(detail.contains("7d 6%"), "{detail}");
    }

    #[test]
    fn name_line_includes_display_name() {
        let u = sample(Provider::Claude, "c@d.com", 10.0, 20.0);
        assert_eq!(account_name_line(&u), "  ● c@d.com");
        assert!(account_usage_line(&u).starts_with("     "));
    }
}
