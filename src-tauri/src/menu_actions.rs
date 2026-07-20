use crate::{
    AccountStore, AppHandle, AuthMethod, AuthSource, Manager, ManagerExt, OsStr, Provider,
};

/// Polls all accounts and rebuilds the tray menu on the main thread.
///
/// Uses a fresh `AccountStore` handle (file-backed ZST) instead of holding
/// `app.state()` across `.await` — Tauri's managed-state guard must not cross
/// suspension points.
pub(crate) async fn refresh_tray(app: &AppHandle) {
    let store = AccountStore::new();
    let snapshot = crate::poller::poll_all(&store).await;
    // Publish to the local HTTP API so agents see the same data as the tray.
    // Synchronous (no `.await`), so the managed-state guard never crosses a
    // suspension point.
    app.state::<crate::api::ApiState>().publish(&snapshot);
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        // Runs inside tao's `extern "C"` `send_event`; a panic unwinding across
        // that FFI frame triggers `panic_cannot_unwind` → process abort. Contain it
        // so a malformed snapshot can never take the whole app down.
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::tray_menu::apply_menu(&app2, &snapshot);
        }))
        .is_err()
        {
            eprintln!("tray: apply_menu panicked; suppressed to keep the tray alive");
        }
    });
}

pub(crate) fn import_provider(app: &AppHandle, provider: Provider) {
    // Previously this ran the blocking CLI/DB read synchronously inside the tray
    // menu-event callback (tao `send_event`), so any panic in `import_from_cli`
    // unwound across the FFI boundary and aborted the app. Spawn it (like every
    // sibling action) so panics are contained by the runtime and the menu stays
    // responsive during the import.
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        match crate::import::import_from_cli(provider) {
            Ok(imported) => {
                let store = app2.state::<AccountStore>();
                match store.add(provider, imported.label, imported.credentials) {
                    Ok(_) => refresh_tray(&app2).await,
                    Err(e) => eprintln!("import: failed to save account: {e}"),
                }
            }
            Err(e) => eprintln!("import: {e}"),
        }
    });
}

pub(crate) fn cli_coordinator_setup(app: &AppHandle, provider: Provider) {
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
                let store = app2.state::<AccountStore>();
                match store.add_reference(
                    account.provider,
                    account.label.clone(),
                    account.auth_source,
                ) {
                    Ok(saved) => {
                        if saved.provider == Provider::Claude {
                            if let AuthSource::CliProfile { profile_root, .. } = &saved.auth_source
                            {
                                let settings_path = profile_root.join("settings.json");
                                if let Err(error) = crate::claude_statusline::install_statusline_bridge(
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
pub(crate) fn import_grok_clipboard(app: &AppHandle) {
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        match crate::import::import_grok_from_clipboard().await {
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

pub(crate) fn oauth_provider(app: &AppHandle, provider: Provider) {
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        match crate::oauth::begin_login(provider).await {
            Ok(creds) => {
                let store = app2.state::<AccountStore>();
                let label = match provider {
                    Provider::Codex => "Codex".to_string(),
                    Provider::Claude => "Claude".to_string(),
                    // Resolve Google email so a terminal account switch shows
                    // the new identity immediately (not a stale "agy" label).
                    Provider::Agy => crate::oauth::agy_email_from_access_token(&creds.access_token)
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

/// The side-effecting action the tray should take for a given (provider, method) pair.
/// Pure routing decision extracted from `dispatch_auth_action` so it is unit-testable
/// without a live `AppHandle`. Edition-INDEPENDENT: the same total mapping over all
/// `AuthMethod` values in both editions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthAction {
    Oauth,
    CliCoordinator,
    Import,
    GrokClipboard,
}

pub(crate) fn classify_auth_action(provider: Provider, method: AuthMethod) -> AuthAction {
    match method {
        AuthMethod::BrowserOAuth => AuthAction::Oauth,
        AuthMethod::Cli => match provider {
            Provider::Codex | Provider::Claude => AuthAction::CliCoordinator,
            _ => AuthAction::Import,
        },
        AuthMethod::LocalDatabase | AuthMethod::ManagementKeyEnvironment => AuthAction::Import,
        AuthMethod::ManagementKeyClipboard => AuthAction::GrokClipboard,
    }
}

pub(crate) fn dispatch_auth_action(app: &AppHandle, provider: Provider, method: AuthMethod) {
    match classify_auth_action(provider, method) {
        AuthAction::Oauth => oauth_provider(app, provider),
        AuthAction::CliCoordinator => cli_coordinator_setup(app, provider),
        AuthAction::Import => import_provider(app, provider),
        AuthAction::GrokClipboard => {
            #[cfg(feature = "edition-pro")]
            import_grok_clipboard(app);
            #[cfg(not(feature = "edition-pro"))]
            eprintln!("clipboard authentication is unavailable in the Free edition");
        }
    }
}

pub(crate) fn handle_menu_event(app: &AppHandle, id: &str) {
    match id {
        "quit" => app.exit(0),
        "about" => {}
        "open-api" => {
            if let Some(url) = crate::api::public_url() {
                if let Err(error) = open::that(&url) {
                    eprintln!("open-api: failed to open {url}: {error}");
                }
            }
        }
        "refresh" => {
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                refresh_tray(&app2).await;
            });
        }
        other if other.starts_with("remove-") => {
            let account_id = other["remove-".len()..].to_string();
            let store = app.state::<AccountStore>();
            let indexed_account = store
                .list()
                .into_iter()
                .find(|account| account.id == account_id);
            match store.remove(&account_id) {
                Ok(Some(removed)) => {
                    crate::poller::evict_last_success(&removed.id);
                    if removed.provider == Provider::Claude {
                        if let AuthSource::CliProfile { profile_root, .. } = &removed.auth_source {
                            let settings_path = profile_root.join("settings.json");
                            if let Err(error) = crate::claude_statusline::remove_statusline_bridge(
                                &settings_path,
                                &removed.id,
                            ) {
                                eprintln!("remove: bridge teardown failed: {error}");
                            }
                        }
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    eprintln!("remove: {error}");
                    crate::poller::evict_last_success(&account_id);
                    if let Some(removed) = indexed_account {
                        if removed.provider == Provider::Claude {
                            if let AuthSource::CliProfile { profile_root, .. } =
                                &removed.auth_source
                            {
                                let settings_path = profile_root.join("settings.json");
                                if let Err(error) = crate::claude_statusline::remove_statusline_bridge(
                                    &settings_path,
                                    &account_id,
                                ) {
                                    eprintln!("remove: bridge teardown failed: {error}");
                                }
                            }
                        }
                    }
                }
            }
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                refresh_tray(&app2).await;
            });
        }
        "toggle-autostart" => {
            let manager = app.autolaunch();
            let result = match manager.is_enabled() {
                Ok(true) => manager.disable(),
                Ok(false) => manager.enable(),
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                eprintln!("autostart: toggle failed: {error}");
            }
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                refresh_tray(&app2).await;
            });
        }
        event_id => {
            if let Some(spec) = crate::tray_menu::spec_for_event(event_id) {
                dispatch_auth_action(app, spec.provider, spec.method);
            }
        }
    }
}

pub(crate) fn statusline_bridge_account_id() -> Result<Option<String>, String> {
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
    crate::claude_statusline::validate_account_id(&account_id)?;
    Ok(Some(account_id))
}
