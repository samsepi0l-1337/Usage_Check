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
use usage_core::{
    account::{AuthSource, Provider},
    AuthMethod,
};

mod agy_local;
mod api;
mod claude_cli;
mod claude_statusline;
mod cli_auth;
mod codex_cli;
#[cfg(feature = "edition-pro")]
mod cursor_local;
mod edition;
mod import;
mod oauth;
mod paths;
mod poller;
mod store;
mod terminal;
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
    // Publish to the local HTTP API so agents see the same data as the tray.
    // Synchronous (no `.await`), so the managed-state guard never crosses a
    // suspension point.
    app.state::<api::ApiState>().publish(&snapshot);
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

fn cli_coordinator_setup(app: &AppHandle, provider: Provider) {
    use crate::cli_auth::{CliAuthCoordinator, ProviderAdapter, RetrySchedule};
    use crate::terminal::TerminalLauncher;

    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let adapter: Box<dyn ProviderAdapter> = match provider {
            Provider::Codex => Box::new(crate::codex_cli::CodexCliAdapter),
            Provider::Claude => Box::new(crate::claude_cli::ClaudeCliAdapter),
            _ => return,
        };

        #[cfg(target_os = "macos")]
        let launcher: Box<dyn TerminalLauncher> = Box::new(crate::terminal::MacosTerminalLauncher);
        #[cfg(target_os = "windows")]
        let launcher: Box<dyn TerminalLauncher> =
            Box::new(crate::terminal::WindowsTerminalLauncher);
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let launcher: Box<dyn TerminalLauncher> = {
            eprintln!("cli setup: unsupported platform");
            return;
        };

        let coordinator = CliAuthCoordinator::new(adapter, launcher, RetrySchedule::production());
        match coordinator.execute().await {
            Ok(account) => {
                let auth_source = match account.auth_source {
                    AuthSource::CliProfile {
                        profile_root,
                        ownership,
                        ..
                    } => AuthSource::CliProfile {
                        profile_root,
                        ownership,
                        expected_identity: account.label.clone(),
                    },
                    other => other,
                };
                let store = app2.state::<AccountStore>();
                match store.add_reference(account.provider, account.label.clone(), auth_source) {
                    Ok(saved) => {
                        if saved.provider == Provider::Claude {
                            if let AuthSource::CliProfile { profile_root, .. } = &saved.auth_source
                            {
                                let settings_path = profile_root.join("settings.json");
                                if let Err(error) = claude_statusline::install_statusline_bridge(
                                    &settings_path,
                                    &saved.id,
                                ) {
                                    eprintln!("cli setup: bridge install failed: {error}");
                                }
                            }
                        }
                        refresh_tray(&app2).await;
                    }
                    Err(error) => eprintln!("cli setup: failed to save account: {error}"),
                }
            }
            Err(error) => eprintln!("cli setup: {error:?}"),
        }
    });
}

#[cfg(feature = "edition-pro")]
fn import_grok_clipboard(app: &AppHandle) {
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        match import::import_grok_from_clipboard().await {
            Ok(imported) => {
                let store = app2.state::<AccountStore>();
                match store.add(Provider::Grok, imported.label, imported.credentials) {
                    Ok(_) => refresh_tray(&app2).await,
                    Err(e) => eprintln!("import: failed to save Grok account: {e}"),
                }
            }
            Err(e) => eprintln!("import grok: {e}"),
        }
    });
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
                    // Resolve Google email so a terminal account switch shows
                    // the new identity immediately (not a stale "agy" label).
                    Provider::Agy => oauth::agy_email_from_access_token(&creds.access_token)
                        .await
                        .unwrap_or_else(|| "agy".to_string()),
                    #[cfg(feature = "edition-pro")]
                    Provider::Cursor | Provider::Grok | Provider::Higgsfield => {
                        provider.display_name().to_string()
                    }
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

fn dispatch_auth_action(app: &AppHandle, provider: Provider, method: AuthMethod) {
    match method {
        AuthMethod::BrowserOAuth => oauth_provider(app, provider),
        AuthMethod::Cli => match provider {
            Provider::Codex | Provider::Claude => cli_coordinator_setup(app, provider),
            _ => import_provider(app, provider),
        },
        AuthMethod::LocalDatabase | AuthMethod::ManagementKeyEnvironment => {
            import_provider(app, provider)
        }
        AuthMethod::ManagementKeyClipboard => {
            #[cfg(feature = "edition-pro")]
            import_grok_clipboard(app);
            #[cfg(not(feature = "edition-pro"))]
            eprintln!("clipboard authentication is unavailable in the Free edition");
        }
    }
}

fn handle_menu_event(app: &AppHandle, id: &str) {
    match id {
        "quit" => app.exit(0),
        "refresh" => {
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                refresh_tray(&app2).await;
            });
        }
        other if other.starts_with("remove-") => {
            let account_id = other["remove-".len()..].to_string();
            match app.state::<AccountStore>().remove(&account_id) {
                Ok(Some(removed)) => {
                    poller::evict_last_success(&removed.id);
                    if removed.provider == Provider::Claude {
                        if let AuthSource::CliProfile { profile_root, .. } = &removed.auth_source {
                            let settings_path = profile_root.join("settings.json");
                            if let Err(error) =
                                claude_statusline::remove_statusline_bridge(&settings_path)
                            {
                                eprintln!("remove: bridge teardown failed: {error}");
                            }
                        }
                    }
                }
                Ok(None) => {}
                Err(error) => eprintln!("remove: {error}"),
            }
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                refresh_tray(&app2).await;
            });
        }
        event_id => {
            if let Some(spec) = tray_menu::spec_for_event(event_id) {
                dispatch_auth_action(app, spec.provider, spec.method);
            }
        }
    }
}

fn statusline_bridge_account_id() -> Result<Option<String>, String> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new("--claude-statusline-bridge")) {
        return Ok(None);
    }
    let account_id = args
        .next()
        .ok_or_else(|| "--claude-statusline-bridge requires an account id".to_string())?
        .into_string()
        .map_err(|_| "Claude account id must be valid UTF-8".to_string())?;
    if args.next().is_some() {
        return Err("--claude-statusline-bridge accepts exactly one account id".to_string());
    }
    claude_statusline::validate_account_id(&account_id)?;
    Ok(Some(account_id))
}

fn main() {
    match statusline_bridge_account_id() {
        Ok(Some(account_id)) => {
            let settings_path = paths::claude_settings_json(&account_id);
            match claude_statusline::handle_statusline_bridge(&account_id, &settings_path) {
                Ok(()) => process::exit(0),
                Err(error) => {
                    eprintln!("status-line bridge error: {error}");
                    process::exit(1);
                }
            }
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("status-line bridge argument error: {error}");
            process::exit(2);
        }
    }

    tauri::Builder::default()
        .manage(AccountStore::new())
        .manage(api::ApiState::new())
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
                .tooltip(edition::product_name())
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

            // Local HTTP API (localhost-only) for other agents / MCP / skills.
            // Disabled via USAGECHECK_API_DISABLE=1; port via USAGECHECK_API_PORT.
            api::spawn(app.state::<api::ApiState>().inner().clone());

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
