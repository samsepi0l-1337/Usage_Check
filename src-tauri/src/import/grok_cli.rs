//! Grok CLI OIDC session (`~/.grok/auth.json`), read-only.
//!
//! SECURITY: tokens are never logged; the file is only ever read, never
//! written or refreshed (the CLI owns it and rotates its own refresh token).

use std::path::Path;

use chrono::{DateTime, Utc};
use usage_core::account::Credentials;

use super::ImportedAccount;

/// One parsed `auth.json` session. `Debug` redacts tokens.
#[derive(Clone, PartialEq)]
pub struct GrokCliEntry {
    pub credentials: Credentials,
    pub identity: String,
    pub email: Option<String>,
}

impl std::fmt::Debug for GrokCliEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrokCliEntry")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

/// Why a stored CLI session could not be used. Carries no token material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrokCliReadError {
    Missing,
    Malformed,
    IdentityNotFound,
    Expired,
}

pub fn parse_grok_cli_auth_json(_root: &serde_json::Value, _now: DateTime<Utc>) -> Vec<GrokCliEntry> {
    Vec::new()
}

pub(crate) fn load_grok_cli_auth_from(_path: &Path) -> Result<Vec<ImportedAccount>, String> {
    Err(String::new())
}

pub fn load_grok_cli_auth() -> Result<Vec<ImportedAccount>, String> {
    Err(String::new())
}

pub(crate) fn read_grok_cli_credentials(
    _path: &Path,
    _identity: &str,
    _now: DateTime<Utc>,
) -> Result<Credentials, GrokCliReadError> {
    Err(GrokCliReadError::Missing)
}

/// Management Key (env) wins when configured; otherwise the first CLI session.
pub(crate) fn load_grok_auth_from(
    _env: Result<ImportedAccount, String>,
    _cli_path: &Path,
) -> Result<ImportedAccount, String> {
    Err(String::new())
}

#[cfg(test)]
#[path = "grok_cli_tests.rs"]
mod tests;
