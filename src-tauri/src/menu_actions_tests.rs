//! B0.4 (Stage-A residual fix): the paid-provider dispatch gate was
//! previously tested only through the PURE `is_dispatch_allowed`/
//! `spec_for_event` predicates (`tray_menu/tests.rs`) — proof the predicate
//! is correct, but not proof `handle_menu_event` actually CALLS it. These
//! tests go through the real event path instead: a headless
//! `tauri::test::mock_app()` `AppHandle`, the SAME `handle_menu_event`
//! `main.rs` wires to every tray click. Deleting the
//! `is_dispatch_allowed(...)` check at the dispatch site (or the `if/else`
//! around it) makes these tests fail, because [`DispatchOutcome::Refused`]
//! is only ever returned from that branch.

use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::SigningKey;
use std::ffi::OsString;
use std::path::Path;

// `USAGECHECK_APP_DATA_DIR` / `USAGECHECK_LICENSE_PUBKEY` are shared,
// process-global env vars ALSO mutated by `license/http_tests.rs` and
// `license/status_tests.rs` — reuse the ONE crate-wide lock there instead of
// a file-local one, or different files (or different vars within this file)
// mutating the same var with different locks could race under `cargo test`
// (which runs every `#[cfg(test)]` module in one process, by default on
// multiple threads).
use crate::license::LICENSE_ENV_LOCK;

struct AppDataDirGuard(Option<OsString>);

impl AppDataDirGuard {
    fn set(path: &Path) -> Self {
        let previous = std::env::var_os("USAGECHECK_APP_DATA_DIR");
        std::env::set_var("USAGECHECK_APP_DATA_DIR", path);
        Self(previous)
    }
}

impl Drop for AppDataDirGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(previous) => std::env::set_var("USAGECHECK_APP_DATA_DIR", previous),
            None => std::env::remove_var("USAGECHECK_APP_DATA_DIR"),
        }
    }
}

struct PubkeyEnvGuard(Option<OsString>);

impl PubkeyEnvGuard {
    fn set(signing_key: &SigningKey) -> Self {
        let previous = std::env::var_os("USAGECHECK_LICENSE_PUBKEY");
        std::env::set_var(
            "USAGECHECK_LICENSE_PUBKEY",
            STANDARD.encode(signing_key.verifying_key().to_bytes()),
        );
        Self(previous)
    }
}

impl Drop for PubkeyEnvGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(previous) => std::env::set_var("USAGECHECK_LICENSE_PUBKEY", previous),
            None => std::env::remove_var("USAGECHECK_LICENSE_PUBKEY"),
        }
    }
}

/// A headless mock Tauri app with `AccountStore` managed against an isolated
/// tempdir. Deliberately does NOT register the real `tauri_plugin_autostart`
/// plugin — none of the three paid-provider dispatch paths under test reach
/// `toggle-autostart`, and registering it for real would risk mutating an
/// actual macOS LaunchAgent, which a test must never do.
fn build_mock_app(store_root: std::path::PathBuf) -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .manage(AccountStore::new_at(store_root))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build mock tauri app")
}

const PAID_PROVIDER_EVENTS: &[&str] = &["add-cursor-local", "add-grok-clipboard", "add-higgsfield-cli"];

#[test]
fn dispatch_gate_refuses_all_three_paid_providers_when_unlicensed() {
    let _env_lock = LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let _app_data_env = AppDataDirGuard::set(tmp.path());

    // No license.json exists at this fresh, isolated app-data dir, so
    // `crate::license::is_pro()` is deterministically false — independent of
    // whatever the machine actually running this test has cached for real.
    assert!(!crate::license::is_pro(), "sanity: fresh app-data dir must be unlicensed");

    let app = build_mock_app(tmp.path().join("store"));
    for event_id in PAID_PROVIDER_EVENTS.iter().copied() {
        let outcome = handle_menu_event(app.handle(), event_id);
        assert_eq!(
            outcome,
            DispatchOutcome::Refused,
            "{event_id} must be refused through the REAL dispatch path when unlicensed"
        );
    }
}

#[test]
fn dispatch_gate_permits_all_three_paid_providers_when_licensed() {
    let _env_lock = LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let _app_data_env = AppDataDirGuard::set(tmp.path());

    let signing_key = SigningKey::from_bytes(&[77u8; 32]);
    let _pubkey_env = PubkeyEnvGuard::set(&signing_key);
    crate::license::testing_persist_pro_license(tmp.path(), &signing_key);
    assert!(
        crate::license::is_pro(),
        "sanity: the persisted record must actually verify as Pro before testing dispatch"
    );

    let app = build_mock_app(tmp.path().join("store"));
    for event_id in PAID_PROVIDER_EVENTS.iter().copied() {
        let outcome = handle_menu_event(app.handle(), event_id);
        assert_eq!(
            outcome,
            DispatchOutcome::Dispatched,
            "{event_id} must be dispatched through the REAL path once Pro is active"
        );
    }
}

#[test]
fn dispatch_gate_always_permits_a_free_provider_regardless_of_license() {
    let _env_lock = LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let _app_data_env = AppDataDirGuard::set(tmp.path());
    assert!(!crate::license::is_pro());

    let app = build_mock_app(tmp.path().join("store"));
    let outcome = handle_menu_event(app.handle(), "add-codex-oauth");
    assert_eq!(outcome, DispatchOutcome::Dispatched);
}

#[test]
fn unrecognized_event_id_is_not_an_auth_dispatch() {
    let _env_lock = LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let _app_data_env = AppDataDirGuard::set(tmp.path());

    let app = build_mock_app(tmp.path().join("store"));
    assert_eq!(
        handle_menu_event(app.handle(), "not-a-real-event-id"),
        DispatchOutcome::Other
    );
    assert_eq!(handle_menu_event(app.handle(), "about"), DispatchOutcome::Other);
}

// --- Stage C: license tray section -----------------------------------

#[test]
fn license_action_for_event_resolves_the_three_clickable_ids() {
    assert_eq!(
        license_action_for_event("license-activate-clipboard"),
        Some(LicenseAction::ActivateFromClipboard)
    );
    assert_eq!(
        license_action_for_event("license-deactivate"),
        Some(LicenseAction::Deactivate)
    );
    assert_eq!(
        license_action_for_event("license-get"),
        Some(LicenseAction::GetLicense)
    );
    assert_eq!(license_action_for_event("not-a-license-event"), None);
    assert_eq!(license_action_for_event("add-grok-clipboard"), None);
}

#[test]
fn license_events_are_not_provider_auth_actions() {
    // These must NOT resolve through the auth-action registry — they are
    // handled by their own arm in `handle_menu_event`, never routed through
    // `auth_action_specs`/`is_dispatch_allowed`.
    for id in [
        "license-activate-clipboard",
        "license-deactivate",
        "license-get",
    ] {
        assert!(
            crate::tray_menu::spec_for_event(id).is_none(),
            "{id} must not resolve through the auth-action registry"
        );
    }
}

#[test]
fn license_key_from_clipboard_text_rejects_empty_and_whitespace() {
    assert_eq!(license_key_from_clipboard_text(""), None);
    assert_eq!(license_key_from_clipboard_text("   "), None);
    assert_eq!(license_key_from_clipboard_text("\n\t  \n"), None);
}

#[test]
fn license_key_from_clipboard_text_trims_a_real_key() {
    assert_eq!(
        license_key_from_clipboard_text("  ABC-123-KEY  \n"),
        Some("ABC-123-KEY")
    );
}

#[test]
fn last_license_attempt_reflects_the_most_recent_recorded_attempt() {
    // `LAST_LICENSE_ATTEMPT` is process-global state shared across every
    // test in this binary (not one of the three env vars `LICENSE_ENV_LOCK`
    // documents, but the same kind of shared mutable state) — take the same
    // lock anyway so this test's two writes can never interleave with
    // another test's. Order relative to OTHER tests isn't asserted (`cargo
    // test` doesn't guarantee run order), only that a write is reflected by
    // the very next read.
    let _env_lock = LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    set_last_license_attempt(Ok(()));
    assert_eq!(last_license_attempt(), Some(Ok(())));
    set_last_license_attempt(Err(ActivationErrorClass::Network));
    assert_eq!(
        last_license_attempt(),
        Some(Err(ActivationErrorClass::Network))
    );
}
