use chrono::{DateTime, Local, Utc};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    AppHandle, Wry,
};
use tauri_plugin_autostart::ManagerExt;
use usage_core::account::Provider;
use crate::edition;
use crate::poller::AccountUsage;
use super::actions::auth_action_specs;
use super::format::{account_name_line, account_usage_line, format_pool_detail, format_usage_detail, vendor_title};
use super::TRAY_ID;

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

/// Highest finite used-percent across an account's windows (5h/week and every
/// agy pool window), or `None` when the account has no finite usage sample.
pub(crate) fn account_max_percent(usage: &AccountUsage) -> Option<f64> {
    let mut windows: Vec<f64> = Vec::new();
    if let Some(q) = &usage.five_hour {
        windows.push(q.percent);
    }
    if let Some(q) = &usage.week {
        windows.push(q.percent);
    }
    for pool in &usage.pool_breakdown {
        if let Some(q) = &pool.five_hour {
            windows.push(q.percent);
        }
        if let Some(q) = &pool.week {
            windows.push(q.percent);
        }
    }
    windows
        .into_iter()
        .filter(|p| p.is_finite())
        .fold(None, |acc, p| Some(acc.map_or(p, |m: f64| m.max(p))))
}

/// Number of accounts whose highest window is at or above `threshold`.
pub(crate) fn near_limit_count(usages: &[AccountUsage], threshold: f64) -> usize {
    usages
        .iter()
        .filter(|u| account_max_percent(u).is_some_and(|p| p >= threshold))
        .count()
}

/// Formats a poll timestamp as a local `Updated HH:MM:SS` label.
pub(crate) fn updated_label(updated_at: DateTime<Utc>) -> String {
    format!("Updated {}", updated_at.with_timezone(&Local).format("%H:%M:%S"))
}

/// Builds the full tray menu from the latest usage snapshot. `updated_at` is the
/// poll time shown as an informational row (None for the pre-first-poll menu).
pub fn build_menu(
    app: &AppHandle,
    usages: &[AccountUsage],
    updated_at: Option<DateTime<Utc>>,
) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::new(app)?;

    // Prominent near-limit banner (disabled row) when any account is at/above
    // the alert threshold (USAGECHECK_ALERT_THRESHOLD, default 90%).
    let threshold = crate::api_alerts::current_alert_threshold();
    let near = near_limit_count(usages, threshold);
    if near > 0 {
        menu.append(&MenuItem::with_id(
            app,
            "near-limit",
            format!("⚠ {near} account(s) near limit (≥{threshold:.0}%)"),
            false,
            None::<&str>,
        )?)?;
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }

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
    let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart_label = if autostart_enabled {
        "✔ Launch at Login"
    } else {
        "Launch at Login"
    };
    menu.append(&MenuItem::with_id(
        app,
        "toggle-autostart",
        autostart_label,
        true,
        None::<&str>,
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    // Informational last-updated + version rows (disabled) + open-API shortcut.
    if let Some(ts) = updated_at {
        menu.append(&MenuItem::with_id(
            app,
            "updated",
            updated_label(ts),
            false,
            None::<&str>,
        )?)?;
    }
    menu.append(&MenuItem::with_id(
        app,
        "about",
        format!("{} v{}", edition::product_name(), env!("CARGO_PKG_VERSION")),
        false,
        None::<&str>,
    )?)?;
    if crate::api::public_url().is_some() {
        menu.append(&MenuItem::with_id(
            app,
            "open-api",
            "Open Usage API",
            true,
            None::<&str>,
        )?)?;
    }
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

pub fn apply_menu(app: &AppHandle, usages: &[AccountUsage], updated_at: Option<DateTime<Utc>>) {
    let Ok(menu) = build_menu(app, usages, updated_at) else {
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
