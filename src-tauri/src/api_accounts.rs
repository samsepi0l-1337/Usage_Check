//! `GET /v1/accounts`: a lightweight inventory of configured accounts with a
//! SAFE auth-method label (never a credential value), for tooling that wants to
//! enumerate accounts and how each authenticates without the full usage payload.
//!
//! Kept in a sibling module so `api.rs` stays within its file-size budget.

use serde::Serialize;
use usage_core::account::{AuthSource, Provider};

use crate::api::UsageResponse;

/// Stable, non-secret label for how an account authenticates. Deliberately
/// avoids any credential value or a name that collides with the DTO's
/// leak-guard denylist (e.g. `management`, `credential_id`).
pub(crate) fn auth_kind(source: &AuthSource) -> &'static str {
    // AuthSource variants exist in both editions (only the Provider enum is
    // edition-gated), so every arm is unconditional.
    match source {
        AuthSource::CliProfile { .. } => "cli_profile",
        AuthSource::BrowserOAuth { .. } => "browser_oauth",
        AuthSource::CursorDatabase { .. } => "cursor_database",
        AuthSource::XaiManagement { .. } => "xai_key",
        AuthSource::HiggsfieldCli { .. } => "higgsfield_cli",
    }
}

#[derive(Serialize)]
struct AccountInfo<'a> {
    id: &'a str,
    provider: Provider,
    display_name: &'a str,
    status: &'a str,
    auth_kind: &'a str,
}

#[derive(Serialize)]
pub struct AccountsResponse<'a> {
    count: usize,
    accounts: Vec<AccountInfo<'a>>,
}

/// Projects the published snapshot into the trimmed inventory shape.
pub fn accounts_response(resp: &UsageResponse) -> AccountsResponse<'_> {
    let accounts: Vec<AccountInfo> = resp
        .accounts
        .iter()
        .map(|a| AccountInfo {
            id: &a.id,
            provider: a.provider,
            display_name: &a.display_name,
            status: &a.status,
            auth_kind: a.auth_kind,
        })
        .collect();
    AccountsResponse {
        count: accounts.len(),
        accounts,
    }
}

#[cfg(test)]
#[path = "api_accounts_tests.rs"]
mod tests;
