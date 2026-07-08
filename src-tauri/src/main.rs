use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};

mod oauth;
mod poller;
mod store;

fn main() {
    tauri::Builder::default()
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

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
