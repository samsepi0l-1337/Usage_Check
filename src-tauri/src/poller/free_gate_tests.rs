use super::*;

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::Path;

use usage_core::account::Credentials;
use usage_core::models::QuotaUsage;

use crate::store::SecretSource;

struct EnvVarGuard {
    name: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(name: &'static str, value: &OsStr) -> Self {
        let previous = std::env::var_os(name);
        std::env::set_var(name, value);
        Self { name, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(previous) => std::env::set_var(self.name, previous),
            None => std::env::remove_var(self.name),
        }
    }
}

/// Redirects both local-scan roots to empty tempdirs. The shared import lock
/// protects both variables: it already owns `CLAUDE_CONFIG_DIR`, and no other
/// test mutates the parent process's `CODEX_HOME`.
struct LocalScanEnv {
    _codex_env: EnvVarGuard,
    _claude_env: EnvVarGuard,
    _codex_root: tempfile::TempDir,
    _claude_root: tempfile::TempDir,
}

impl LocalScanEnv {
    fn new() -> Self {
        let codex_root = tempfile::tempdir().expect("create empty Codex home");
        let claude_root = tempfile::tempdir().expect("create empty Claude config root");
        let codex_env = EnvVarGuard::set("CODEX_HOME", codex_root.path().as_os_str());
        let claude_env = EnvVarGuard::set("CLAUDE_CONFIG_DIR", claude_root.path().as_os_str());
        Self {
            _codex_env: codex_env,
            _claude_env: claude_env,
            _codex_root: codex_root,
            _claude_root: claude_root,
        }
    }
}

fn add_free_accounts(store: &AccountStore, provider: Provider, count: usize) -> Vec<String> {
    (0..count)
        .map(|index| {
            store
                .add_secret_with(
                    provider,
                    format!("{provider:?}-{index}@example.test"),
                    SecretSource::BrowserOAuth,
                    Credentials {
                        // Empty tokens keep Codex and Claude on their
                        // deterministic, non-network `needs_login` paths.
                        access_token: String::new(),
                        refresh_token: None,
                        account_id: Some(format!("{provider:?}-{index}")),
                        expires_at: None,
                    },
                    || true,
                )
                .expect("register free-provider fixture account")
                .id
        })
        .collect()
}

/// Registers `count` browser-OAuth accounts for `provider`, bypassing the Free
/// cap via `|| true` so the fixture can build the "user was Pro, added several,
/// then downgraded" state without deleting anything. Empty access tokens: a
/// non-empty one routes into the real network fetch (`providers.rs:583-635`).
fn store_with_free_accounts(
    provider: Provider,
    count: usize,
) -> (tempfile::TempDir, AccountStore, Vec<String>) {
    let tmp = tempfile::tempdir().expect("create account store tempdir");
    let store = AccountStore::new_at(tmp.path().join("store"));
    let ids = add_free_accounts(&store, provider, count);
    (tmp, store, ids)
}

/// Asserts `usage` is EXACTLY the gated placeholder — every field, not just the
/// status — so a partially-populated result from a fetch that did happen fails.
fn assert_is_gated_placeholder(usage: &AccountUsage) {
    let expected = account_usage_pro_required(&usage.account);
    assert_eq!(usage.status, "pro_required");
    assert_eq!(usage.display_name, expected.display_name);
    assert!(usage.plan.is_none());
    assert!(usage.five_hour.is_none());
    assert!(usage.week.is_none());
    assert!(usage.pool_breakdown.is_empty());
    assert!(usage.breakdown.is_empty());
    assert!(usage.detail_suffix.is_none());
    assert_eq!(usage.totals, expected.totals);
}

fn result_ids(results: &[AccountUsage]) -> Vec<String> {
    results
        .iter()
        .map(|usage| usage.account.id.clone())
        .collect()
}

fn result_id_set(results: &[AccountUsage]) -> BTreeSet<String> {
    result_ids(results).into_iter().collect()
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn free_downgrade_keeps_the_first_account_and_gates_the_rest() {
    let _env_lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _scan_env = LocalScanEnv::new();
    let (_tmp, store, ids) = store_with_free_accounts(Provider::Codex, 3);

    let results = poll_all_with(&store, false).await;

    assert_eq!(results.len(), 3);
    assert_eq!(result_ids(&results), ids);
    assert_eq!(results[0].status, "needs_login");
    assert_is_gated_placeholder(&results[1]);
    assert_is_gated_placeholder(&results[2]);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn free_downgrade_deletes_nothing() {
    let _env_lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _scan_env = LocalScanEnv::new();
    let (_tmp, store, ids) = store_with_free_accounts(Provider::Codex, 3);

    let results = poll_all_with(&store, false).await;

    assert_eq!(results.len(), 3);
    assert_eq!(store.list().len(), 3);
    assert_eq!(
        store
            .list()
            .into_iter()
            .map(|account| account.id)
            .collect::<Vec<_>>(),
        ids
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn pro_upgrade_restores_every_account_with_no_reimport() {
    let _env_lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _scan_env = LocalScanEnv::new();
    let (_tmp, store, ids) = store_with_free_accounts(Provider::Codex, 3);

    let results = poll_all_with(&store, true).await;

    assert_eq!(results.len(), 3);
    assert_eq!(
        result_id_set(&results),
        ids.into_iter().collect::<BTreeSet<_>>()
    );
    assert!(results.iter().all(|usage| usage.status == "needs_login"));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn exactly_one_account_is_never_gated() {
    let _env_lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _scan_env = LocalScanEnv::new();
    let tmp = tempfile::tempdir().expect("create account store tempdir");
    let store = AccountStore::new_at(tmp.path().join("store"));
    let mut ids = add_free_accounts(&store, Provider::Codex, 1);
    ids.extend(add_free_accounts(&store, Provider::Claude, 1));

    let results = poll_all_with(&store, false).await;

    assert_eq!(results.len(), 2);
    assert_eq!(result_ids(&results), ids);
    assert!(results.iter().all(|usage| usage.status == "needs_login"));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn the_cap_is_per_provider_not_global() {
    let _env_lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _scan_env = LocalScanEnv::new();
    let tmp = tempfile::tempdir().expect("create account store tempdir");
    let store = AccountStore::new_at(tmp.path().join("store"));
    let codex_ids = add_free_accounts(&store, Provider::Codex, 2);
    let claude_ids = add_free_accounts(&store, Provider::Claude, 2);

    let results = poll_all_with(&store, false).await;

    assert_eq!(results.len(), 4);
    assert_eq!(
        result_ids(&results),
        [codex_ids.clone(), claude_ids.clone()].concat()
    );
    let gated_ids: BTreeSet<_> = results
        .iter()
        .filter(|usage| usage.status == "pro_required")
        .map(|usage| usage.account.id.clone())
        .collect();
    assert_eq!(
        gated_ids,
        BTreeSet::from([codex_ids[1].clone(), claude_ids[1].clone()])
    );
    assert_eq!(results[0].status, "needs_login");
    assert_eq!(results[2].status, "needs_login");
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn paid_and_free_gates_are_independent() {
    let _env_lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _scan_env = LocalScanEnv::new();
    let (_tmp, store, codex_ids) = store_with_free_accounts(Provider::Codex, 2);
    let higgsfield = store
        .add_reference_with(
            Provider::Higgsfield,
            "higgsfield@example.test".to_string(),
            AuthSource::HiggsfieldCli {
                expected_identity: "higgsfield@example.test".to_string(),
            },
            || true,
        )
        .expect("register paid-provider fixture account");

    let results = poll_all_with(&store, false).await;

    assert_eq!(results.len(), 3);
    assert_eq!(
        result_ids(&results),
        vec![
            codex_ids[0].clone(),
            codex_ids[1].clone(),
            higgsfield.id.clone()
        ]
    );
    assert_eq!(results[0].status, "needs_login");
    assert_is_gated_placeholder(&results[1]);
    assert_is_gated_placeholder(&results[2]);
}

/// Pins `apply_last_success`'s status filter: a cached success must not replace
/// `pro_required`. The ungated fixture is `needs_login`, so this does not prove
/// whether the Free gate runs before or after `apply_last_success`.
#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn gated_account_does_not_inherit_a_cached_success() {
    let _env_lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _scan_env = LocalScanEnv::new();
    let (_tmp, store, ids) = store_with_free_accounts(Provider::Codex, 2);
    let second = store
        .list()
        .into_iter()
        .find(|account| account.id == ids[1])
        .expect("second fixture account exists");
    let mut ok_usage = account_usage_pro_required(&second);
    ok_usage.status = "ok".to_string();
    ok_usage.five_hour = Some(QuotaUsage {
        percent: 42.0,
        resets_at: None,
        window_seconds: None,
    });
    let stamped = {
        let mut cache = last_success_cache()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        apply_last_success(&mut cache, &second.id, ok_usage)
    };
    assert_eq!(stamped.status, "ok", "seed must actually cache as ok");

    let results = poll_all_with(&store, false).await;

    assert_eq!(results.len(), 2);
    assert_eq!(result_ids(&results), ids);
    assert_is_gated_placeholder(&results[1]);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn apply_free_gate_reclassifies_a_pro_snapshot_after_a_downgrade() {
    let _env_lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _scan_env = LocalScanEnv::new();
    let (_tmp, store, ids) = store_with_free_accounts(Provider::Codex, 2);
    let mut snapshot = poll_all_with(&store, true).await;
    let first_before = serde_json::to_value(&snapshot[0]).expect("serialize first account");

    apply_free_gate(&mut snapshot);

    assert_eq!(snapshot.len(), 2);
    assert_eq!(result_ids(&snapshot), ids);
    assert_eq!(
        serde_json::to_value(&snapshot[0]).expect("serialize first account after gate"),
        first_before
    );
    assert_is_gated_placeholder(&snapshot[1]);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn apply_free_gate_is_idempotent() {
    let _env_lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _scan_env = LocalScanEnv::new();
    let (_tmp, store, _ids) = store_with_free_accounts(Provider::Claude, 2);
    let mut snapshot = poll_all_with(&store, true).await;

    apply_free_gate(&mut snapshot);
    let once = serde_json::to_value(&snapshot).expect("serialize once-gated snapshot");
    apply_free_gate(&mut snapshot);

    assert_eq!(snapshot.len(), 2);
    assert_eq!(
        serde_json::to_value(&snapshot).expect("serialize twice-gated snapshot"),
        once
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Fixed lock order: license, then scan env.
async fn poll_all_applies_the_free_gate_from_real_license_state() {
    let _license_lock = crate::license::LICENSE_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _scan_lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let license_root = tempfile::tempdir().expect("create empty app-data dir");
    let _license_env = EnvVarGuard::set(
        "USAGECHECK_APP_DATA_DIR",
        Path::new(license_root.path()).as_os_str(),
    );
    let _scan_env = LocalScanEnv::new();
    assert!(!crate::license::is_pro());
    let (_tmp, store, ids) = store_with_free_accounts(Provider::Codex, 2);

    let results = poll_all(&store).await;

    assert_eq!(results.len(), 2);
    assert_eq!(result_ids(&results), ids);
    assert_eq!(results[0].status, "needs_login");
    assert_is_gated_placeholder(&results[1]);
}
