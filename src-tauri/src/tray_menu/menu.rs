use chrono::{DateTime, Local, Utc};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    AppHandle, Manager, Runtime,
};
use tauri_plugin_autostart::ManagerExt;
use usage_core::account::Provider;
use crate::edition;
use crate::license::{ActivationErrorClass, LicenseStatus};
use crate::poller::AccountUsage;
use super::format::{account_name_line, account_usage_lines, activation_result_line, format_breakdown_row, format_pool_detail, format_usage_detail, license_status_line, vendor_title};
use super::TRAY_ID;

// Generic over `R: Runtime` (rather than hardcoded `Wry`) so
// `menu_actions::refresh_tray` — itself generic for the B0.4 dispatch-gate
// integration tests — can call `apply_menu`/`build_menu` under
// `tauri::test::mock_app()`'s `MockRuntime` as well as production's `Wry`.
fn append_vendor_section<R: Runtime>(
    app: &AppHandle<R>,
    menu: &Menu<R>,
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
        for (i, line) in account_usage_lines(usage).into_iter().enumerate() {
            menu.append(&MenuItem::with_id(
                app,
                format!("usage-{}-{i}", usage.account.id),
                line,
                false,
                None::<&str>,
            )?)?;
        }
        // Per-model breakdown rows (Claude Fable, Codex Spark, Cursor First
        // Party/API) — one MenuItem per row at the same indent as the primary
        // usage line.
        for (i, row) in usage.breakdown.iter().enumerate() {
            menu.append(&MenuItem::with_id(
                app,
                format!("breakdown-{}-{i}", usage.account.id),
                format!("     {}", format_breakdown_row(row)),
                false,
                None::<&str>,
            )?)?;
        }
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
    for row in &usage.breakdown {
        windows.push(row.usage.percent);
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

/// Whether the tray should show the "Deactivate license" row: whenever
/// there is live Pro entitlement OR a stored record exists at all (an
/// expired/grace-period/tampered record still has a file worth clearing).
/// Pure so the presence/absence rule is unit-testable without a live tray
/// `Menu` (menu construction itself requires the platform main thread).
pub(crate) fn should_show_deactivate(is_pro: bool, has_stored_license: bool) -> bool {
    is_pro || has_stored_license
}

/// Derives the Add-section entitlement from the single license-status sample
/// captured by `build_menu`. Kept as one production function so tests exercise
/// the exact rule used by the menu, including the development override.
pub(crate) fn is_pro_from(status: &LicenseStatus) -> bool {
    matches!(
        status,
        LicenseStatus::Pro { .. } | LicenseStatus::ProDevOverride
    )
}

/// (item 4 fix) One row of the tray's license section, in render order.
/// Pure decision extracted from `build_menu` so the ordered row SET — ids,
/// labels, and enabled/disabled flags — is unit-testable: a real
/// `tauri::menu::Menu` needs the platform main thread to construct, so it
/// cannot be built (or inspected) in a `cargo test` run.
pub(crate) struct LicenseRow {
    pub(crate) id: &'static str,
    pub(crate) label: String,
    pub(crate) enabled: bool,
}

/// The full, ordered license-section row spec: status (disabled) → result
/// (disabled, only once an attempt has been made this run) → the three
/// action rows — Activate from clipboard, then Deactivate license (only
/// when [`should_show_deactivate`]), then Get a license…. `build_menu`
/// renders exactly this; no row decision is left inline there. Pro entitlement
/// for the deactivate gate, including the debug override, is derived from
/// `status` itself rather than a separately-read `license::is_pro()` call, so
/// the whole row set comes from one consistent status snapshot.
pub(crate) fn license_rows(
    status: LicenseStatus,
    last_attempt: Option<&Result<(), ActivationErrorClass>>,
    has_record: bool,
) -> Vec<LicenseRow> {
    let mut rows = vec![LicenseRow {
        id: "license-status",
        label: license_status_line(status),
        enabled: false,
    }];
    if let Some(attempt) = last_attempt {
        rows.push(LicenseRow {
            id: "license-result",
            label: activation_result_line(attempt),
            enabled: false,
        });
    }
    rows.push(LicenseRow {
        id: "license-activate-clipboard",
        label: "Activate from clipboard".to_string(),
        enabled: true,
    });
    let is_pro = matches!(
        status,
        LicenseStatus::Pro { .. } | LicenseStatus::ProDevOverride
    );
    if should_show_deactivate(is_pro, has_record) {
        rows.push(LicenseRow {
            id: "license-deactivate",
            label: "Deactivate license".to_string(),
            enabled: true,
        });
    }
    rows.push(LicenseRow {
        id: "license-get",
        label: "Get a license…".to_string(),
        enabled: true,
    });
    rows
}

/// Formats a poll timestamp as a local `Updated HH:MM:SS` label.
pub(crate) fn updated_label(updated_at: DateTime<Utc>) -> String {
    format!("Updated {}", updated_at.with_timezone(&Local).format("%H:%M:%S"))
}

/// Builds the full tray menu from the latest usage snapshot. `updated_at` is the
/// poll time shown as an informational row (None for the pre-first-poll menu).
pub fn build_menu<R: Runtime>(
    app: &AppHandle<R>,
    usages: &[AccountUsage],
    updated_at: Option<DateTime<Utc>>,
) -> tauri::Result<Menu<R>> {
    // INVARIANT: license state is sampled EXACTLY ONCE per menu build, and every
    // consumer below is fed that one sample. Activation/deactivation are async,
    // so two reads inside one build can straddle a transition and produce a menu
    // that contradicts itself (D10). `is_pro` is derived from `license_status`
    // by the same rule `license_rows` uses internally, so the Add section and
    // the license section can never disagree.
    let license_status = crate::license::status();
    let is_pro = is_pro_from(&license_status);

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

    // Enablement comes from the authoritative account index, NOT from `usages`:
    // the first menu is built with an empty snapshot (`main.rs:150`), so a
    // snapshot-derived check would render "Add" clickable on a Free install
    // that already holds an account until the first poll lands (D7).
    //
    // `AccountStore::list()` is FAIL-OPEN (`store/index.rs:49-53` maps every
    // read error to an empty Vec), so an unreadable index renders Add as
    // ENABLED even at the cap. That is a cosmetic inconsistency, not a bypass:
    // the store gate still refuses the write and reports the reason (§06.3).
    // This menu is a hint, never the authority.
    let accounts = app.state::<crate::store::AccountStore>().list();

    for spec in super::auth_action_specs_with(is_pro) {
        let enabled = super::is_add_enabled(&spec, is_pro, &accounts);
        add_submenu.append(&MenuItem::with_id(
            app,
            spec.event_id,
            super::add_entry_label(&spec, enabled),
            enabled,
            None::<&str>,
        )?)?;
    }

    menu.append(&add_submenu)?;

    // Add-Account result row: the reason the most recent add attempt failed
    // this run. Mirrors the existing `license-result` row — informational,
    // disabled, and absent until an attempt has actually failed.
    if let Some(reason) = crate::menu_actions::last_add_account_attempt() {
        menu.append(&MenuItem::with_id(
            app,
            "add-account-result",
            super::add_account_result_line(&reason),
            false,
            None::<&str>,
        )?)?;
    }

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

    // License section — status/result (disabled, informational) plus the
    // three clickable actions. These are NOT provider-auth actions (never
    // routed through `auth_action_specs`/`is_dispatch_allowed`); resolved by
    // `menu_actions::handle_menu_event`'s own arms instead. Never renders the
    // key or token: `license_status_line`/`activation_result_line` are built
    // only from `LicenseStatus`/`ActivationErrorClass`, neither of which
    // carries either. Row decisions (ids, labels, enabled flags, and which
    // rows even appear) all live in `license_rows` (item 4 fix) — this loop
    // renders exactly what it returns, with no row logic left inline here.
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    for row in license_rows(
        license_status,
        crate::menu_actions::last_license_attempt().as_ref(),
        crate::license::has_stored_license(),
    ) {
        menu.append(&MenuItem::with_id(app, row.id, row.label, row.enabled, None::<&str>)?)?;
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

pub fn apply_menu<R: Runtime>(app: &AppHandle<R>, usages: &[AccountUsage], updated_at: Option<DateTime<Utc>>) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        eprintln!("tray: icon '{TRAY_ID}' not found");
        return;
    };
    let Ok(menu) = build_menu(app, usages, updated_at) else {
        eprintln!("tray: failed to build menu");
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
