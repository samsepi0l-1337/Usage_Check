//! UsageCheck — menu-bar / system-tray usage monitor.
//!
//! macOS: menu-bar accessory (no Dock icon, no popup window).
//! Windows: notification-area tray only (no console, no main window).
//! Left-click the tray icon opens a native menu (Docker-style) with live
//! usage rows, Add/Remove account actions, Refresh, and Quit.

// Hide the console window on Windows release builds so the app is tray-only
// (otherwise a blank console looks like a separate on-screen window).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::time::Duration;
use std::{ffi::OsStr, process};

use tauri::{tray::TrayIconBuilder, AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;
use usage_core::{
    account::{AuthSource, Provider},
    AuthMethod,
};

mod agy_local;
mod api;
mod api_accounts;
mod api_alerts;
mod api_auth;
mod api_csv;
mod api_health;
mod api_metrics;
mod api_server;
mod claude_cli;
mod claude_statusline;
mod cli_auth;
mod codex_cli;
#[cfg(feature = "edition-pro")]
mod cursor_local;
mod edition;
mod import;
mod menu_actions;
mod oauth;
mod paths;
mod poller;
mod store;
mod terminal;
mod tray_menu;

use store::AccountStore;
#[allow(unused_imports)]
pub(crate) use menu_actions::{classify_auth_action, AuthAction};

/// Default seconds between background usage-poll ticks that refresh the tray.
const DEFAULT_POLL_SECS: u64 = 60;
/// Lower/upper clamp for a user-configured poll interval.
const MIN_POLL_SECS: u64 = 15;
const MAX_POLL_SECS: u64 = 3600;

/// Resolves the background poll interval from `raw` (the `USAGECHECK_POLL_SECS`
/// env value): a valid integer is clamped to `[MIN_POLL_SECS, MAX_POLL_SECS]`;
/// anything missing/invalid falls back to [`DEFAULT_POLL_SECS`].
fn poll_interval_secs(raw: Option<&str>) -> u64 {
    raw.and_then(|s| s.trim().parse::<u64>().ok())
        .map(|secs| secs.clamp(MIN_POLL_SECS, MAX_POLL_SECS))
        .unwrap_or(DEFAULT_POLL_SECS)
}

/// Background poll interval, honoring `USAGECHECK_POLL_SECS`.
fn poll_interval() -> Duration {
    let raw = std::env::var("USAGECHECK_POLL_SECS").ok();
    Duration::from_secs(poll_interval_secs(raw.as_deref()))
}

/// Builds a simple 22×22 bar-chart tray glyph as raw RGBA (no PNG decoder
/// feature required).
///
/// - macOS: black bars (template image; system tints for light/dark menu bar).
/// - Windows: light bars so the glyph stays visible on the dark taskbar.
fn tray_icon_image() -> tauri::image::Image<'static> {
    const W: u32 = 22;
    const H: u32 = 22;
    let mut rgba = vec![0u8; (W * H * 4) as usize];
    let margin = 3u32;
    let bar_w = 4u32;
    let gap = 2u32;
    let heights = [7u32, 11u32, 9u32];
    #[cfg(target_os = "macos")]
    let (r, g, b) = (0u8, 0u8, 0u8);
    #[cfg(not(target_os = "macos"))]
    let (r, g, b) = (240u8, 240u8, 240u8);
    let mut x0 = margin;
    for bh in heights {
        let y0 = H - margin - bh;
        for y in y0..(H - margin) {
            for x in x0..(x0 + bar_w) {
                let i = ((y * W + x) * 4) as usize;
                rgba[i] = r;
                rgba[i + 1] = g;
                rgba[i + 2] = b;
                rgba[i + 3] = 255;
            }
        }
        x0 += bar_w + gap;
    }
    tauri::image::Image::new_owned(rgba, W, H)
}

fn main() {
    match menu_actions::statusline_bridge_account_id() {
        Ok(Some(account_id)) => match claude_statusline::handle_statusline_bridge(&account_id) {
            Ok(()) => process::exit(0),
            Err(error) => {
                eprintln!("status-line bridge error: {error}");
                process::exit(1);
            }
        },
        Ok(None) => {}
        Err(error) => {
            eprintln!("status-line bridge argument error: {error}");
            process::exit(2);
        }
    }

    tauri::Builder::default()
        .manage(AccountStore::new())
        .manage(api::ApiState::new())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                // Menu-bar accessory: no Dock icon.
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }

            // Destroy any config-declared webview — this app is tray-menu only
            // on both macOS and Windows (no separate usage window).
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.close();
            }

            // Re-anchor existing Claude accounts on the unique accountUuid (best-effort, idempotent).
            if let Err(error) = app
                .state::<AccountStore>()
                .migrate_claude_identity_anchors()
            {
                eprintln!("startup: claude identity migration skipped: {error}");
            }

            let initial = tray_menu::build_menu(app.handle(), &[], None)?;

            // macOS + Windows: left-click opens the native usage menu
            // (same Docker-style UX; no separate popup window).
            let tray = TrayIconBuilder::with_id(tray_menu::tray_id())
                .icon(tray_icon_image())
                .menu(&initial)
                .tooltip(edition::product_name())
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| {
                    // Menu clicks are delivered inside tao's `extern "C"` event
                    // dispatch; a panic unwinding across that boundary aborts the
                    // process. Contain it so one bad click can't kill the app.
                    let id = event.id.as_ref().to_string();
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        menu_actions::handle_menu_event(app, &id);
                    }))
                    .is_err()
                    {
                        eprintln!("tray: menu handler panicked (id={id}); suppressed");
                    }
                });
            #[cfg(target_os = "macos")]
            {
                // Template tinting is macOS-only.
                tray.icon_as_template(true).build(app)?;
            }
            #[cfg(not(target_os = "macos"))]
            {
                tray.build(app)?;
            }

            // Local HTTP API (localhost-only) for other agents / MCP / skills.
            // Disabled via USAGECHECK_API_DISABLE=1; port via USAGECHECK_API_PORT.
            api_server::spawn(app.state::<api::ApiState>().inner().clone());

            // Initial poll + periodic refresh.
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(poll_interval());
                loop {
                    interval.tick().await;
                    menu_actions::refresh_tray(&app_handle).await;
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                // Keep the tray process alive when no windows remain.
                // Explicit Quit still passes Some(exit_code) via `app.exit(0)`.
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
