use std::time::Duration;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};
use usage_core::account::{Account, Provider};

mod import;
mod oauth;
mod paths;
mod poller;
mod store;

use poller::AccountUsage;
use store::AccountStore;

/// Interval between background usage-poll ticks that feed the `usage-updated`
/// event to the frontend.
const POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Builds a simple 22×22 bar-chart tray glyph as raw RGBA (no PNG decoder
/// feature required). Black-on-transparent so macOS template rendering works.
fn tray_icon_image() -> tauri::image::Image<'static> {
    const W: u32 = 22;
    const H: u32 = 22;
    let mut rgba = vec![0u8; (W * H * 4) as usize];
    let margin = 3u32;
    let bar_w = 4u32;
    let gap = 2u32;
    let heights = [7u32, 11u32, 9u32];
    let mut x0 = margin;
    for bh in heights {
        let y0 = H - margin - bh;
        for y in y0..(H - margin) {
            for x in x0..(x0 + bar_w) {
                let i = ((y * W + x) * 4) as usize;
                rgba[i] = 0;
                rgba[i + 1] = 0;
                rgba[i + 2] = 0;
                rgba[i + 3] = 255;
            }
        }
        x0 += bar_w + gap;
    }
    tauri::image::Image::new_owned(rgba, W, H)
}

#[tauri::command]
fn list_accounts(state: tauri::State<'_, AccountStore>) -> Vec<Account> {
    state.list()
}

/// Runs the OAuth login flow for `provider` and, on success, persists the new
/// account (with a default "<provider> account" label) in the store. On
/// failure (e.g. agy has no reproducible OAuth flow — see `oauth::config`),
/// the error string is propagated so the UI can show the fallback message
/// instead of throwing.
#[tauri::command]
async fn add_account(
    provider: String,
    state: tauri::State<'_, AccountStore>,
) -> Result<Account, String> {
    let provider = Provider::from_str(&provider)
        .ok_or_else(|| format!("unknown provider: {provider}"))?;

    let creds = oauth::begin_login(provider).await?;

    let label = format!("{} account", provider.as_str());
    let account = state.add(provider, label, creds);
    Ok(account)
}

/// Imports an account from the local CLI config (Codex/Claude auth files) or
/// registers a local-log-only agy account. This is the fallback path when
/// browser OAuth is unavailable or the user already logged in via the CLI.
#[tauri::command]
fn import_account(
    provider: String,
    state: tauri::State<'_, AccountStore>,
) -> Result<Account, String> {
    let provider = Provider::from_str(&provider)
        .ok_or_else(|| format!("unknown provider: {provider}"))?;

    let creds = import::import_from_cli(provider)?;
    let label = match provider {
        Provider::Agy => "agy (local logs)".to_string(),
        Provider::Codex => "codex (CLI import)".to_string(),
        Provider::Claude => "claude (CLI import)".to_string(),
    };
    Ok(state.add(provider, label, creds))
}

#[tauri::command]
fn remove_account(id: String, state: tauri::State<'_, AccountStore>) {
    state.remove(&id);
}

#[tauri::command]
async fn get_usage(state: tauri::State<'_, AccountStore>) -> Result<Vec<AccountUsage>, String> {
    Ok(poller::poll_all(&state).await)
}

fn main() {
    tauri::Builder::default()
        .manage(AccountStore::new())
        .invoke_handler(tauri::generate_handler![
            list_accounts,
            add_account,
            import_account,
            remove_account,
            get_usage
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Tray apps hide instead of destroying the popup window.
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                // Menu-bar accessory: no Dock icon.
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }

            let quit = MenuItem::with_id(app, "quit", "Quit UsageCheck", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&quit])?;

            TrayIconBuilder::new()
                .icon(tray_icon_image())
                .icon_as_template(true)
                .menu(&menu)
                .tooltip("UsageCheck")
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    if event.id.as_ref() == "quit" {
                        app.exit(0);
                    }
                })
                .on_tray_icon_event(|tray, event| match event {
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } => {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            if window.is_visible().unwrap_or(false) {
                                if let Err(e) = window.hide() {
                                    eprintln!("tray: hide failed: {e}");
                                }
                            } else {
                                if let Err(e) = window.show() {
                                    eprintln!("tray: show failed: {e}");
                                }
                                if let Err(e) = window.set_focus() {
                                    eprintln!("tray: set_focus failed: {e}");
                                }
                            }
                        }
                    }
                    _ => {}
                })
                .build(app)?;

            // Background poll loop: every 60s, build a fresh usage snapshot
            // and broadcast it to any listening webview.
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(POLL_INTERVAL);
                loop {
                    interval.tick().await;
                    let store = app_handle.state::<AccountStore>();
                    let snapshot = poller::poll_all(&store).await;
                    if let Err(e) = app_handle.emit("usage-updated", snapshot) {
                        eprintln!("poll loop: emit failed: {e}");
                    }
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                // Keep the tray process alive when the (hidden) window would
                // otherwise trigger a default exit. Explicit Quit still passes
                // Some(exit_code) via `app.exit(0)`.
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}
