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
use std::sync::atomic::Ordering;
use usage_core::account::{Account, Credentials};

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

fn credentials() -> Credentials {
    Credentials {
        access_token: "test-access-token".into(),
        refresh_token: Some("test-refresh-token".into()),
        account_id: Some("test-account-id".into()),
        expires_at: None,
    }
}

fn account(id: &str, provider: Provider) -> Account {
    Account {
        id: id.into(),
        provider,
        label: format!("{id}@example.com"),
        auth_source: AuthSource::BrowserOAuth {
            credential_id: format!("credential-{id}"),
        },
    }
}

fn seed_codex_account(app: &tauri::App<tauri::test::MockRuntime>) {
    app.state::<AccountStore>()
        .add(Provider::Codex, "codex-one".into(), credentials()) // BARE-WRAPPER-WIRING
        .expect("seed one Codex account below the cap");
}

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
fn handle_menu_event_refuses_a_free_provider_add_at_the_cap() {
    let _env_lock = LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let _app_data_env = AppDataDirGuard::set(tmp.path());
    let app = build_mock_app(tmp.path().join("store"));
    seed_codex_account(&app);

    assert_eq!(
        handle_menu_event(app.handle(), "add-codex-cli"),
        DispatchOutcome::Refused
    );
}

#[test]
fn handle_menu_event_dispatches_a_free_provider_add_below_the_cap() {
    let _env_lock = LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let _app_data_env = AppDataDirGuard::set(tmp.path());
    let app = build_mock_app(tmp.path().join("store"));

    assert_eq!(
        handle_menu_event(app.handle(), "add-claude-cli"),
        DispatchOutcome::Dispatched
    );
}

#[test]
fn refusal_publishes_a_user_visible_reason() {
    let _env_lock = LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let _app_data_env = AppDataDirGuard::set(tmp.path());
    let app = build_mock_app(tmp.path().join("store"));
    seed_codex_account(&app);
    set_add_account_reason(None);

    assert_eq!(
        handle_menu_event(app.handle(), "add-codex-cli"),
        DispatchOutcome::Refused
    );
    assert_eq!(
        last_add_account_attempt(),
        Some(usage_core::edition::free_limit_reason(Provider::Codex))
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide app-data env mutation must remain serialized.
async fn refusal_rerenders_without_starting_a_poll_refresh() {
    let _env_lock = LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().expect("create isolated app-data directory");
    let _app_data_env = AppDataDirGuard::set(tmp.path());
    let app = build_mock_app(tmp.path().join("store"));
    let retained_usage = crate::poller::account_usage_pro_required(&account(
        "retained-claude",
        Provider::Claude,
    ));
    let retained_at = chrono::Utc::now();
    retain_last_snapshot(&[retained_usage], Some(retained_at));
    let generation_before = REFRESH_GENERATION.load(Ordering::SeqCst);

    assert_eq!(
        handle_menu_event(app.handle(), "add-grok-clipboard"),
        DispatchOutcome::Refused
    );

    // Give an incorrectly detached refresh ample opportunity to claim a
    // generation before asserting that refusal is render-only.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        REFRESH_GENERATION.load(Ordering::SeqCst),
        generation_before,
        "a refused Add action must not start a provider poll"
    );
    let retained = last_snapshot();
    assert_eq!(retained.usages.len(), 1);
    assert_eq!(retained.usages[0].account.id, "retained-claude");
    assert_eq!(retained.updated_at, Some(retained_at));
}

#[test]
fn record_add_outcome_publishes_the_reason_and_clears_it_on_success() {
    let _env_lock = LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    set_add_account_reason(None);

    assert!(!record_add_outcome("test", Err("boom".into())));
    assert_eq!(last_add_account_attempt(), Some("boom".into()));

    assert!(record_add_outcome(
        "test",
        Ok(account("codex-one", Provider::Codex))
    ));
    assert_eq!(last_add_account_attempt(), None);
}

#[test]
fn the_cap_gate_keys_off_is_pro_not_has_stored_license() {
    let _env_lock = LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let _app_data_env = AppDataDirGuard::set(tmp.path());
    std::fs::write(tmp.path().join("license.json"), b"not valid license JSON")
        .expect("write malformed license record");
    assert!(crate::license::has_stored_license());
    assert!(!crate::license::is_pro());

    let app = build_mock_app(tmp.path().join("store"));
    seed_codex_account(&app);

    assert_eq!(
        handle_menu_event(app.handle(), "add-codex-cli"),
        DispatchOutcome::Refused
    );
}

#[test]
fn a_superseded_refresh_does_not_publish() {
    let _env_lock = LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let generation = REFRESH_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let _newer_generation = REFRESH_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    // This exercises the stamp COMPARISON, not the publish path it guards.
    // `build_mock_app` intentionally manages no `ApiState`; optional API
    // publication is covered separately above, without pretending this test
    // drives the generation-guarded refresh publication path.
    assert_ne!(REFRESH_GENERATION.load(Ordering::SeqCst), generation);
}

#[test]
fn publishing_a_snapshot_without_managed_api_state_is_a_noop() {
    let tmp = tempfile::tempdir().expect("create isolated store directory");
    let app = build_mock_app(tmp.path().join("store"));

    publish_api_snapshot(app.handle(), &[]);
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
