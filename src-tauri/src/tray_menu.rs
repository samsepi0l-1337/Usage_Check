//! Native macOS/Windows tray menu (Docker-style) — no separate popup window.
//!
//! Menu layout:
//!   ● Codex  5h 12% · 7d 45%
//!   ● Claude …
//!   ────────
//!   Add Account ▸
//!   Remove ▸
//!   Refresh Now
//!   ────────
//!   Quit UsageCheck

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    AppHandle, Wry,
};

use usage_core::account::Provider;

use crate::poller::AccountUsage;

const TRAY_ID: &str = "main";

fn status_dot(status: &str) -> &'static str {
    match status {
        "ok" => "●",
        "needs_login" => "○",
        _ => "◐",
    }
}

fn provider_title(p: Provider) -> &'static str {
    match p {
        Provider::Codex => "Codex",
        Provider::Claude => "Claude",
        Provider::Agy => "agy",
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

fn format_quota_or_tokens(usage: &AccountUsage) -> String {
    let has_quota = usage.five_hour.is_some() || usage.week.is_some();
    if has_quota {
        let five = usage
            .five_hour
            .as_ref()
            .map(|q| format!("5h {:.0}%", q.percent))
            .unwrap_or_else(|| "5h —".into());
        let week = usage
            .week
            .as_ref()
            .map(|q| format!("7d {:.0}%", q.percent))
            .unwrap_or_else(|| "7d —".into());
        format!("{five} · {week}")
    } else {
        format!(
            "5h {} · 7d {}",
            format_tokens(usage.totals.five_hours),
            format_tokens(usage.totals.week)
        )
    }
}

fn usage_row_label(usage: &AccountUsage) -> String {
    let name = provider_title(usage.account.provider);
    let detail = format_quota_or_tokens(usage);
    let status = if usage.status == "ok" {
        String::new()
    } else {
        format!(" ({})", usage.status)
    };
    format!(
        "{} {}  {}{}",
        status_dot(&usage.status),
        name,
        detail,
        status
    )
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
        for usage in usages {
            let id = format!("status-{}", usage.account.id);
            menu.append(&MenuItem::with_id(
                app,
                &id,
                usage_row_label(usage),
                false,
                None::<&str>,
            )?)?;
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
        "Add agy (local logs)",
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
        for usage in usages {
            let id = format!("remove-{}", usage.account.id);
            let label = format!(
                "Remove {} — {}",
                provider_title(usage.account.provider),
                usage.account.label
            );
            remove_submenu.append(&MenuItem::with_id(
                app,
                &id,
                label,
                true,
                None::<&str>,
            )?)?;
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
        .map(|u| {
            format!(
                "{} {}",
                provider_title(u.account.provider),
                format_quota_or_tokens(u)
            )
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

