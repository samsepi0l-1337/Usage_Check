//! Per-account usage poller: builds an `AccountUsage` snapshot for every
//! account in the `AccountStore`.
//!
//! Codex/Claude: live HTTP quota using the stored access token; on failure
//! falls back to local-log aggregation.
//! Agy: Antigravity Model Quota — prefer the running app's local
//! `RetrieveUserQuotaSummary`, else Cloud Code OAuth remote fetch. No local
//! SQLite token totals (those are not the UI quota %).
//!
//! SECURITY: never log/print an access token or other credential value.

use chrono::Utc;
use usage_core::account::{AuthSource, Provider};
use usage_core::models::{LocalProvenance, LocalUsage};

use crate::paths;
use crate::store::AccountStore;

mod http;

mod local_scan;
use local_scan::local_usage_for_provider;

mod providers;
mod usage_model;
use providers::{
    assemble_cli_profile_usage, poll_agy, poll_claude_cli_profile, poll_claude_oauth,
    poll_codex_cli_profile, poll_codex_oauth,
};
mod providers_pro;
use providers_pro::{poll_cursor, poll_grok, poll_higgsfield};
pub use usage_model::{account_usage_pro_required, assemble_account_usage, AccountUsage};

mod last_success;
pub use last_success::evict_last_success;
use last_success::{apply_last_success, last_success_cache};

/// Builds the full per-account usage snapshot.
///
/// Entitlement is read twice on purpose. The first read parameterizes the poll;
/// the second, AFTER every awaited fetch has resolved, re-applies the Free gate
/// to THIS snapshot, so a license deactivated mid-poll cannot be reported with
/// real quota numbers for surplus accounts.
///
/// This closes the WITHIN-ONE-POLL window only. The separate window — an older
/// refresh finishing after a newer one and publishing stale data — is closed at
/// the publication boundary by `menu_actions::refresh_tray`'s generation stamp
/// (plan §06.4, D9). The two are complementary, not redundant: this one runs
/// when there is no second refresh at all.
///
/// Downgrade-only by design. A Free→Pro change is NOT re-polled, so an upgrade
/// landing mid-poll leaves surplus accounts reading `pro_required` until the
/// next refresh. That direction under-reports rather than over-reports.
pub async fn poll_all(store: &AccountStore) -> Vec<AccountUsage> {
    let mut snapshot = poll_all_with(store, crate::license::is_pro()).await;
    if !crate::license::is_pro() {
        apply_free_gate(&mut snapshot);
    }
    snapshot
}

/// Re-applies the Free-state per-provider cap to an already-assembled snapshot.
/// Idempotent, and performs no I/O: the ranking is recomputed from the
/// snapshot's own accounts, which `poll_all_with` emits in index order.
fn apply_free_gate(snapshot: &mut [AccountUsage]) {
    let accounts: Vec<usage_core::account::Account> =
        snapshot.iter().map(|usage| usage.account.clone()).collect();
    let surplus = usage_core::edition::free_surplus_account_ids(&accounts);
    for usage in snapshot.iter_mut() {
        if surplus.contains(&usage.account.id) {
            *usage = account_usage_pro_required(&usage.account);
        }
    }
}

/// Core of [`poll_all`], with the license flag injected explicitly (mirrors
/// `tray_menu::actions::auth_action_specs_with`) so the paid-provider gate is
/// unit-testable without reading global license state.
async fn poll_all_with(store: &AccountStore, is_pro: bool) -> Vec<AccountUsage> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let accounts = store.list();
    // Free-state per-provider cap: every free-provider account beyond the first
    // is reported with the same `pro_required` shape as an unlicensed paid
    // account — kept in the index, still listed in the tray, no quota numbers,
    // and no provider fetch. Derived from index order
    // (`usage_core::edition::free_surplus_account_ids`), the same rule the
    // add-time gate uses, so the two halves cannot disagree.
    let free_surplus = if is_pro {
        std::collections::BTreeSet::new()
    } else {
        usage_core::edition::free_surplus_account_ids(&accounts)
    };
    let mut out = Vec::with_capacity(accounts.len());
    let now = Utc::now();
    let mut codex_local = local_usage_for_provider(store, &accounts, Provider::Codex, now).await;
    let mut claude_local = local_usage_for_provider(store, &accounts, Provider::Claude, now).await;

    for account in accounts {
        let usage = if free_surplus.contains(&account.id) {
            account_usage_pro_required(&account)
        } else {
            match account.provider {
                Provider::Agy => poll_agy(store, &client, &account).await,
                Provider::Codex => {
                    let local = codex_local
                        .remove(&account.id)
                        .unwrap_or_else(|| LocalUsage::none(LocalProvenance::NoLocalProfile));
                    match &account.auth_source {
                        AuthSource::CliProfile {
                            profile_root,
                            expected_identity,
                            ..
                        } => {
                            let outcome =
                                poll_codex_cli_profile(profile_root, expected_identity).await;
                            assemble_cli_profile_usage(&account, outcome, local)
                        }
                        _ => {
                            let outcome = poll_codex_oauth(store, &client, &account).await;
                            assemble_account_usage(&account, outcome, local)
                        }
                    }
                }
                Provider::Claude => {
                    let local = claude_local
                        .remove(&account.id)
                        .unwrap_or_else(|| LocalUsage::none(LocalProvenance::NoLocalProfile));
                    match &account.auth_source {
                        AuthSource::CliProfile {
                            profile_root,
                            expected_identity,
                            ..
                        } => {
                            let outcome = poll_claude_cli_profile(
                                store,
                                &account.id,
                                &client,
                                profile_root,
                                expected_identity,
                                &paths::claude_statusline_snapshot(&account.id),
                            )
                            .await;
                            assemble_cli_profile_usage(&account, outcome, local)
                        }
                        _ => {
                            let outcome = poll_claude_oauth(store, &client, &account).await;
                            assemble_account_usage(&account, outcome, local)
                        }
                    }
                }
                Provider::Cursor | Provider::Grok | Provider::Higgsfield if !is_pro => {
                    account_usage_pro_required(&account)
                }
                Provider::Cursor => poll_cursor(store, &client, &account).await,
                Provider::Grok => poll_grok(store, &client, &account).await,
                Provider::Higgsfield => poll_higgsfield(store, &account).await,
            }
        };
        let usage = {
            let mut cache = last_success_cache()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            apply_last_success(&mut cache, &account.id, usage)
        };
        out.push(usage);
    }

    out
}

#[cfg(test)]
#[path = "pro_gate_tests.rs"]
mod pro_gate_tests;

#[cfg(test)]
#[path = "free_gate_tests.rs"]
mod free_gate_tests;
