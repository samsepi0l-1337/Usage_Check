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

mod usage_model;
mod providers;
use providers::{
    assemble_cli_profile_usage,
    poll_agy,
    poll_claude_oauth,
    poll_codex_cli_profile,
    poll_codex_oauth,
    read_claude_snapshot_outcome,
};
#[cfg(feature = "edition-pro")]
mod providers_pro;
#[cfg(feature = "edition-pro")]
use providers_pro::{poll_cursor, poll_grok, poll_higgsfield};
pub use usage_model::{assemble_account_usage, AccountUsage};

mod last_success;
pub use last_success::evict_last_success;
use last_success::{apply_last_success, last_success_cache};

/// Builds the full per-account usage snapshot.
pub async fn poll_all(store: &AccountStore) -> Vec<AccountUsage> {
    let client = reqwest::Client::new();
    let accounts = store.list();
    let mut out = Vec::with_capacity(accounts.len());
    let now = Utc::now();
    let mut codex_local = local_usage_for_provider(store, &accounts, Provider::Codex, now).await;
    let mut claude_local = local_usage_for_provider(store, &accounts, Provider::Claude, now).await;

    for account in accounts {
        let usage = match account.provider {
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
                        expected_identity, ..
                    } => {
                        let outcome = read_claude_snapshot_outcome(
                            &paths::claude_statusline_snapshot(&account.id),
                            expected_identity,
                        );
                        assemble_cli_profile_usage(&account, outcome, local)
                    }
                    _ => {
                        let outcome = poll_claude_oauth(store, &client, &account).await;
                        assemble_account_usage(&account, outcome, local)
                    }
                }
            }
            #[cfg(feature = "edition-pro")]
            Provider::Cursor => poll_cursor(store, &client, &account).await,
            #[cfg(feature = "edition-pro")]
            Provider::Grok => poll_grok(store, &client, &account).await,
            #[cfg(feature = "edition-pro")]
            Provider::Higgsfield => poll_higgsfield(store, &account).await,
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
