use crate::{
    AccountStore, AppHandle, AuthMethod, AuthSource, Manager, ManagerExt, OsStr, Provider,
};
use crate::license::ActivationErrorClass;
use tauri::Runtime;

/// Monotonic refresh generation. Every `refresh_tray` claims the next value
/// before polling and re-reads it immediately before publishing; a refresh
/// whose stamp is no longer the newest discards its snapshot instead of
/// publishing it.
static REFRESH_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Polls all accounts and rebuilds the tray menu on the main thread.
///
/// Uses a fresh `AccountStore` handle (file-backed ZST) instead of holding
/// `app.state()` across `.await` — Tauri's managed-state guard must not cross
/// suspension points.
///
/// Generic over `R: Runtime` (rather than hardcoded to the default `Wry`)
/// solely so `handle_menu_event`'s dispatch-gate integration tests
/// (`menu_actions_tests.rs`, B0.4) can drive the REAL event path against
/// `tauri::test::mock_app()`'s headless `AppHandle<MockRuntime>` — production
/// callers (`main.rs`) still resolve `R = Wry` as before; nothing about their
/// behavior changes.
pub(crate) async fn refresh_tray<R: Runtime>(app: &AppHandle<R>) {
    use std::sync::atomic::Ordering;

    let generation = REFRESH_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let store = AccountStore::new();
    let snapshot = crate::poller::poll_all(&store).await;

    // PUBLICATION BOUNDARY. Refreshes overlap (:216-222, :365-369) and a poll
    // takes seconds, so an older refresh can finish after a newer one. Before
    // this stamp, publishing it overwrote newer data — and if the older refresh
    // ran while Pro and the newer while Free, it republished real quota numbers
    // for accounts the runtime is no longer entitled to show. Dropping the stale
    // snapshot is correct on both counts: the newer refresh has already
    // published, or is about to.
    if REFRESH_GENERATION.load(Ordering::SeqCst) != generation {
        return;
    }

    app.state::<crate::api::ApiState>().publish(&snapshot);
    let updated_at = Some(chrono::Utc::now());
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        // Re-read the generation HERE, next to its use. `run_on_main_thread`
        // DEFERS this closure onto the platform event loop, so an arbitrary
        // scheduling gap separates the check above from `apply_menu` below; a
        // newer refresh can win that gap. Re-checking costs one atomic load and
        // removes the larger of the two remaining windows (K12).
        if REFRESH_GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::tray_menu::apply_menu(&app2, &snapshot, updated_at);
        }))
        .is_err()
        {
            eprintln!("tray: apply_menu panicked; suppressed to keep the tray alive");
        }
    });
}

pub(crate) fn import_provider<R: Runtime>(app: &AppHandle<R>, provider: Provider) {
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
                record_add_outcome(
                    "import",
                    store.add(provider, imported.label, imported.credentials),
                );
                refresh_tray(&app2).await;
            }
            Err(e) => eprintln!("import: {e}"),
        }
    });
}

pub(crate) fn cli_coordinator_setup<R: Runtime>(app: &AppHandle<R>, provider: Provider) {
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
                let saved = store.add_reference(
                    account.provider,
                    account.label.clone(),
                    account.auth_source,
                );
                let saved_account = saved.as_ref().ok().cloned();
                record_add_outcome("cli setup", saved);
                if let Some(saved) = saved_account {
                    if saved.provider == Provider::Claude {
                        if let AuthSource::CliProfile { profile_root, .. } = &saved.auth_source {
                            let settings_path = profile_root.join("settings.json");
                            if let Err(error) =
                                crate::claude_statusline::install_statusline_bridge(
                                    &settings_path,
                                    &saved.id,
                                )
                            {
                                eprintln!("cli setup: bridge install failed: {error}");
                            }
                        }
                    }
                }
                refresh_tray(&app2).await;
            }
            Err(error) => eprintln!("cli setup: {error:?}"),
        }
    });
}

pub(crate) fn import_grok_clipboard<R: Runtime>(app: &AppHandle<R>) {
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        match crate::import::import_grok_from_clipboard().await {
            Ok(imported) => {
                let store = app2.state::<AccountStore>();
                record_add_outcome(
                    "import grok",
                    store.add(Provider::Grok, imported.label, imported.credentials),
                );
                refresh_tray(&app2).await;
            }
            Err(e) => eprintln!("import grok: {e}"),
        }
    });
}

/// Reason string of the most recent FAILED "Add Account" attempt in THIS
/// process — never persisted. `None` when the last attempt succeeded or none
/// has been made this run. Read by `tray_menu::build_menu`.
///
/// SECURITY: holds only `usage_core::edition::free_limit_reason` output or an
/// `AccountStore::add*` error string — provider display names, fixed reason
/// literals, and filesystem paths. No arm formats a credential value.
static LAST_ADD_ACCOUNT_ATTEMPT: std::sync::Mutex<Option<String>> =
    std::sync::Mutex::new(None);

/// Publishes (or clears) the add-account reason shown in the tray. The single
/// writer, so the refusal path and the store-rejection path cannot drift apart.
fn set_add_account_reason(reason: Option<String>) {
    *LAST_ADD_ACCOUNT_ATTEMPT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = reason;
}

/// Records the outcome of one add attempt and logs failures. Returns `true` on
/// success. Every `store.add*` caller in this file funnels through here.
fn record_add_outcome(context: &str, result: Result<usage_core::account::Account, String>) -> bool {
    match result {
        Ok(_) => {
            set_add_account_reason(None);
            true
        }
        Err(error) => {
            eprintln!("{context}: failed to save account: {error}");
            set_add_account_reason(Some(error));
            false
        }
    }
}

pub(crate) fn last_add_account_attempt() -> Option<String> {
    LAST_ADD_ACCOUNT_ATTEMPT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Outcome of the most recent tray "Activate from clipboard" attempt in THIS
/// process — never persisted to disk. `None` before any attempt has been
/// made this run. Read by the tray's license "result" row
/// ([`last_license_attempt`]) and updated only by
/// [`activate_license_from_clipboard`].
///
/// SECURITY: holds only the coarse [`ActivationErrorClass`] (H4), never the
/// key, the token, or `ActivationError`'s `Display` (which for
/// `Server{..}`/`InvalidToken` can embed server-provided text) — so this
/// state can never leak a secret into the tray even by construction.
static LAST_LICENSE_ATTEMPT: std::sync::Mutex<Option<Result<(), ActivationErrorClass>>> =
    std::sync::Mutex::new(None);

fn set_last_license_attempt(result: Result<(), ActivationErrorClass>) {
    *LAST_LICENSE_ATTEMPT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(result);
}

/// Read by `tray_menu::build_menu` to render (or omit) the license "result"
/// row.
pub(crate) fn last_license_attempt() -> Option<Result<(), ActivationErrorClass>> {
    *LAST_LICENSE_ATTEMPT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Reads the system clipboard for a license key — same pattern as
/// `import::grok::read_clipboard_text` (`arboard`). SECURITY: the clipboard
/// text (the license key) is never logged.
fn read_license_clipboard_text() -> Result<String, String> {
    arboard::Clipboard::new()
        .map_err(|e| format!("clipboard unavailable: {e}"))?
        .get_text()
        .map_err(|_| "clipboard has no text".to_string())
}

/// Pure gate: `None` when `text` trims to nothing. Extracted so the "refuse
/// an empty/whitespace clipboard without calling `license::activate`" rule
/// is unit-testable without touching the real system clipboard.
pub(crate) fn license_key_from_clipboard_text(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Tray "Activate from clipboard": reads the clipboard exactly like
/// `import_grok_clipboard`, trims it, and — only when non-empty — calls
/// [`crate::license::activate`]. On success or failure, refreshes the tray
/// so the status/result rows and (on success) the newly-unlocked paid
/// providers appear. SECURITY: never logs the clipboard contents, the key,
/// or the token — only the coarse `ActivationErrorClass` on failure (H4).
pub(crate) fn activate_license_from_clipboard<R: Runtime>(app: &AppHandle<R>) {
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let clipboard_text = match read_license_clipboard_text() {
            Ok(text) => text,
            Err(e) => {
                eprintln!("license: clipboard read failed: {e}");
                return;
            }
        };
        let Some(key) = license_key_from_clipboard_text(&clipboard_text) else {
            eprintln!("license: clipboard is empty; not attempting activation");
            return;
        };
        match crate::license::activate(key).await {
            Ok(_) => set_last_license_attempt(Ok(())),
            Err(error) => {
                eprintln!("license: activation failed; class={}", error.classify());
                set_last_license_attempt(Err(error.classify()));
            }
        }
        refresh_tray(&app2).await;
    });
}

/// Tray "Deactivate license": removes the persisted record, then refreshes
/// the tray so the status row and the now-locked paid providers update.
pub(crate) fn deactivate_license<R: Runtime>(app: &AppHandle<R>) {
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = crate::license::deactivate().await {
            eprintln!("license: deactivate failed: {error}");
        }
        refresh_tray(&app2).await;
    });
}

/// Tray "Get a license…": opens the license page in the system browser,
/// same `open` crate already used for `open-api`/OAuth callbacks.
pub(crate) fn open_license_page() {
    if let Err(error) = open::that("https://autoworkit.com/") {
        eprintln!("license: failed to open license page: {error}");
    }
}

/// The three tray-clickable license actions. NOT provider-auth actions —
/// never routed through `auth_action_specs`/`is_dispatch_allowed`
/// (`tray_menu::actions`); resolved here with their own ids instead. Pure
/// routing decision extracted (mirrors [`classify_auth_action`]) so it is
/// unit-testable without a live `AppHandle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LicenseAction {
    ActivateFromClipboard,
    Deactivate,
    GetLicense,
}

pub(crate) fn license_action_for_event(event_id: &str) -> Option<LicenseAction> {
    match event_id {
        "license-activate-clipboard" => Some(LicenseAction::ActivateFromClipboard),
        "license-deactivate" => Some(LicenseAction::Deactivate),
        "license-get" => Some(LicenseAction::GetLicense),
        _ => None,
    }
}

pub(crate) fn oauth_provider<R: Runtime>(app: &AppHandle<R>, provider: Provider) {
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
                    Provider::Cursor | Provider::Grok | Provider::Higgsfield => {
                        provider.display_name().to_string()
                    }
                };
                record_add_outcome("oauth", store.add(provider, label, creds));
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

pub(crate) fn dispatch_auth_action<R: Runtime>(
    app: &AppHandle<R>,
    provider: Provider,
    method: AuthMethod,
) {
    match classify_auth_action(provider, method) {
        AuthAction::Oauth => oauth_provider(app, provider),
        AuthAction::CliCoordinator => cli_coordinator_setup(app, provider),
        AuthAction::Import => import_provider(app, provider),
        AuthAction::GrokClipboard => import_grok_clipboard(app),
    }
}

/// The outcome of one [`handle_menu_event`] call. Exists so the B0.4
/// dispatch-gate integration tests (`menu_actions_tests.rs`) can assert on
/// the REAL license-gate check inside `handle_menu_event` itself — not just
/// the pure `is_dispatch_allowed`/`spec_for_event` predicates it calls — by
/// distinguishing "an auth-action event id was dispatched" from "it was
/// refused because the license isn't active" from "not an auth-action event
/// id at all". Production callers (`main.rs`) ignore the return value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DispatchOutcome {
    /// `id` did not resolve to an auth-action event (quit/about/refresh/
    /// remove-*/toggle-autostart/open-api, or an unknown id).
    Other,
    /// `id` resolved to a registered auth action and dispatch was attempted.
    Dispatched,
    /// `id` resolved to a registered auth action and was refused because it is
    /// either a Pro-gated provider without an active license, or a free provider
    /// already at the Free per-provider account cap. The reason is published via
    /// `set_add_account_reason` and the menu is rebuilt, so the refusal is visible
    /// rather than silent.
    Refused,
}

pub(crate) fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, id: &str) -> DispatchOutcome {
    // License actions are resolved first and are NOT provider-auth actions —
    // they never go through `spec_for_event`/`is_dispatch_allowed` below.
    if let Some(action) = license_action_for_event(id) {
        match action {
            LicenseAction::ActivateFromClipboard => activate_license_from_clipboard(app),
            LicenseAction::Deactivate => deactivate_license(app),
            LicenseAction::GetLicense => open_license_page(),
        }
        return DispatchOutcome::Other;
    }
    match id {
        "quit" => {
            app.exit(0);
            DispatchOutcome::Other
        }
        "about" => DispatchOutcome::Other,
        "open-api" => {
            if let Some(url) = crate::api::public_url() {
                if let Err(error) = open::that(&url) {
                    eprintln!("open-api: failed to open {url}: {error}");
                }
            }
            DispatchOutcome::Other
        }
        "refresh" => {
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                refresh_tray(&app2).await;
            });
            DispatchOutcome::Other
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
            DispatchOutcome::Other
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
            DispatchOutcome::Other
        }
        event_id => {
            let Some(spec) = crate::tray_menu::spec_for_event(event_id) else {
                return DispatchOutcome::Other;
            };
            // Re-check entitlement HERE, at the point of side effect.
            // `spec_for_event` resolves through the full, ungated registry, so a
            // stale/crafted event id — or a menu rendered before the license
            // lapsed or before the cap was reached — must not still trigger an
            // import. `is_add_enabled` is the same predicate the menu used,
            // against the same `AccountStore::list()`.
            let is_pro = crate::license::is_pro();
            let accounts = app.state::<AccountStore>().list();
            if crate::tray_menu::is_add_enabled(&spec, is_pro, &accounts) {
                dispatch_auth_action(app, spec.provider, spec.method);
                DispatchOutcome::Dispatched
            } else {
                // A refusal the user cannot see is not a refusal they can act
                // on: publish the same sentence the store rejection would have
                // produced, and rebuild the tray so the row that should have
                // been disabled becomes disabled.
                let reason = if usage_core::edition::requires_pro(spec.provider) {
                    format!("{} requires a Pro license.", spec.provider.display_name())
                } else {
                    usage_core::edition::free_limit_reason(spec.provider)
                };
                eprintln!("dispatch: {event_id} refused: {reason}");
                set_add_account_reason(Some(reason));
                let app2 = app.clone();
                tauri::async_runtime::spawn(async move {
                    refresh_tray(&app2).await;
                });
                DispatchOutcome::Refused
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

#[cfg(test)]
#[path = "menu_actions_tests.rs"]
mod tests;
