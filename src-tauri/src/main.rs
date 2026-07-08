use std::time::Duration;

use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};
use usage_core::account::{Account, Provider};

mod oauth;
mod poller;
mod store;

use poller::AccountUsage;
use store::AccountStore;

/// Interval between background usage-poll ticks that feed the `usage-updated`
/// event to the frontend.
const POLL_INTERVAL: Duration = Duration::from_secs(60);

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
            remove_account,
            get_usage
        ])
        .setup(|app| {
            TrayIconBuilder::new()
                .icon_as_template(true)
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
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
