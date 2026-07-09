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

use tauri::{
    tray::TrayIconBuilder,
    AppHandle, Manager,
};
use usage_core::account::Provider;

mod import;
mod oauth;
mod paths;
mod poller;
mod store;
mod tray_menu;

use store::AccountStore;

/// Interval between background usage-poll ticks that refresh the tray menu.
const POLL_INTERVAL: Duration = Duration::from_secs(60);

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

/// Polls all accounts and rebuilds the tray menu on the main thread.
///
/// Uses a fresh `AccountStore` handle (file-backed ZST) instead of holding
/// `app.state()` across `.await` — Tauri's managed-state guard must not cross
/// suspension points.
async fn refresh_tray(app: &AppHandle) {
    let store = AccountStore::new();
    let snapshot = poller::poll_all(&store).await;
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        tray_menu::apply_menu(&app2, &snapshot);
    });
}

fn import_provider(app: &AppHandle, provider: Provider) {
    let store = app.state::<AccountStore>();
    match import::import_from_cli(provider) {
        Ok(imported) => match store.add(provider, imported.label, imported.credentials) {
            Ok(_) => {
                let app2 = app.clone();
                tauri::async_runtime::spawn(async move {
                    refresh_tray(&app2).await;
                });
            }
            Err(e) => eprintln!("import: failed to save account: {e}"),
        },
        Err(e) => eprintln!("import: {e}"),
    }
}

fn oauth_provider(app: &AppHandle, provider: Provider) {
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        match oauth::begin_login(provider).await {
            Ok(creds) => {
                let store = app2.state::<AccountStore>();
                let label = match provider {
                    Provider::Codex => "Codex".to_string(),
                    Provider::Claude => "Claude".to_string(),
                    Provider::Agy => "Gemini".to_string(),
                };
                if let Err(e) = store.add(provider, label, creds) {
                    eprintln!("oauth: failed to save account: {e}");
                    return;
                }
                refresh_tray(&app2).await;
            }
            Err(e) => eprintln!("oauth: {e}"),
        }
    });
}

fn handle_menu_event(app: &AppHandle, id: &str) {
    match id {
        "quit" => {
            app.exit(0);
        }
        "refresh" => {
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                refresh_tray(&app2).await;
            });
        }
        "add-codex-cli" => import_provider(app, Provider::Codex),
        "add-claude-cli" => import_provider(app, Provider::Claude),
        "add-agy" => import_provider(app, Provider::Agy),
        "add-codex-oauth" => oauth_provider(app, Provider::Codex),
        "add-claude-oauth" => oauth_provider(app, Provider::Claude),
        other if other.starts_with("remove-") => {
            let account_id = &other["remove-".len()..];
            app.state::<AccountStore>().remove(account_id);
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                refresh_tray(&app2).await;
            });
        }
        _ => {}
    }
}

fn main() {
    tauri::Builder::default()
        .manage(AccountStore::new())
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

            // Collapse duplicate CLI/OAuth imports of the same account.
            AccountStore::new().dedupe();

            let initial = tray_menu::build_menu(app.handle(), &[])?;

            // macOS + Windows: left-click opens the native usage menu
            // (same Docker-style UX; no separate popup window).
            let tray = TrayIconBuilder::with_id(tray_menu::tray_id())
                .icon(tray_icon_image())
                .menu(&initial)
                .tooltip("UsageCheck")
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| {
                    handle_menu_event(app, event.id.as_ref());
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

            // Initial poll + periodic refresh.
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                refresh_tray(&app_handle).await;
                let mut interval = tokio::time::interval(POLL_INTERVAL);
                loop {
                    interval.tick().await;
                    refresh_tray(&app_handle).await;
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
