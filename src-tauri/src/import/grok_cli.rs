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

/// Upper bound for `auth.json`; anything larger is not a CLI session file.
const MAX_AUTH_FILE_BYTES: u64 = 1024 * 1024;

fn nonempty_str(obj: &serde_json::Value, key: &str) -> Option<String> {
    obj.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Parses every `auth.json` entry (top-level map keyed by issuer slot such as
/// `https://auth.x.ai::<client-id>`). Entries whose access token is already
/// expired are kept only when a `refresh_token` is present, so the CLI can
/// renew them; expired-only entries are skipped. Never panics on odd shapes.
pub fn parse_grok_cli_auth_json(root: &serde_json::Value, now: DateTime<Utc>) -> Vec<GrokCliEntry> {
    let Some(map) = root.as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (slot, value) in map {
        if !value.is_object() {
            continue;
        }
        let Some(access) =
            nonempty_str(value, "key").or_else(|| nonempty_str(value, "access_token"))
        else {
            continue;
        };
        let refresh_token = nonempty_str(value, "refresh_token");
        let expires_at = value
            .get("expires_at")
            .and_then(super::claude::parse_expires_at)
            .or_else(|| crate::oauth::jwt_exp(&access));
        if expires_at.is_some_and(|exp| exp <= now) && refresh_token.is_none() {
            continue;
        }
        // Fail closed: `AccountStore::add` treats credentials without a JWT,
        // refresh token or expiry as a Management Key and would persist the
        // token as an app-owned secret. Never let a CLI entry take that path.
        if !usage_core::fetch::grok::credentials_are_oidc(
            &access,
            refresh_token.as_deref(),
            expires_at.is_some(),
        ) {
            continue;
        }
        let email = nonempty_str(value, "email");
        let identity = nonempty_str(value, "user_id")
            .or_else(|| email.clone())
            .unwrap_or_else(|| slot.clone());
        out.push(GrokCliEntry {
            credentials: Credentials {
                access_token: access,
                refresh_token,
                account_id: Some(identity.clone()),
                expires_at,
            },
            identity,
            email,
        });
    }
    out
}

fn read_auth_root(path: &Path) -> Result<serde_json::Value, GrokCliReadError> {
    let file = std::fs::File::open(path).map_err(|_| GrokCliReadError::Missing)?;
    let mut data = String::new();
    std::io::Read::read_to_string(
        &mut std::io::Read::take(file, MAX_AUTH_FILE_BYTES + 1),
        &mut data,
    )
    .map_err(|_| GrokCliReadError::Malformed)?;
    if data.len() as u64 > MAX_AUTH_FILE_BYTES {
        return Err(GrokCliReadError::Malformed);
    }
    serde_json::from_str(&data).map_err(|_| GrokCliReadError::Malformed)
}

fn grok_cli_label(entry: &GrokCliEntry) -> String {
    entry
        .email
        .clone()
        .unwrap_or_else(|| format!("Grok · {}", entry.identity))
}

pub(crate) fn load_grok_cli_auth_from(path: &Path) -> Result<Vec<ImportedAccount>, String> {
    let root = read_auth_root(path).map_err(|e| match e {
        GrokCliReadError::Missing => format!(
            "Grok CLI auth not found at {} — run `grok login` first",
            path.display()
        ),
        _ => "Grok auth.json is not valid JSON".to_string(),
    })?;
    let entries = parse_grok_cli_auth_json(&root, Utc::now());
    if entries.is_empty() {
        return Err(
            "Grok CLI auth.json has no usable accounts — run `grok login` to sign in".into(),
        );
    }
    Ok(entries
        .into_iter()
        .map(|entry| ImportedAccount {
            label: grok_cli_label(&entry),
            credentials: entry.credentials,
        })
        .collect())
}

/// Loads every Grok CLI session from `~/.grok/auth.json` (or `$GROK_HOME`).
pub fn load_grok_cli_auth() -> Result<Vec<ImportedAccount>, String> {
    let path = crate::paths::grok_auth_file()
        .ok_or_else(|| "could not resolve home directory".to_string())?;
    load_grok_cli_auth_from(&path)
}

/// Current credentials of the session whose identity matches `identity`.
/// Read-only: a token that is expired (even with a refresh token) yields
/// [`GrokCliReadError::Expired`] and is never refreshed here — the CLI renews
/// its own file.
pub(crate) fn read_grok_cli_credentials(
    path: &Path,
    identity: &str,
    now: DateTime<Utc>,
) -> Result<Credentials, GrokCliReadError> {
    let root = read_auth_root(path)?;
    let entry = parse_grok_cli_auth_json(&root, now)
        .into_iter()
        .find(|e| e.identity == identity)
        .ok_or(GrokCliReadError::IdentityNotFound)?;
    if entry.credentials.expires_at.is_some_and(|exp| exp <= now) {
        return Err(GrokCliReadError::Expired);
    }
    Ok(entry.credentials)
}

/// Management Key (env) wins when configured; otherwise the first CLI session.
pub(crate) fn load_grok_auth_from(
    env: Result<ImportedAccount, String>,
    cli_path: &Path,
) -> Result<ImportedAccount, String> {
    match env {
        Ok(account) => Ok(account),
        Err(env_err) => match load_grok_cli_auth_from(cli_path) {
            Ok(mut accounts) => Ok(accounts.remove(0)),
            Err(cli_err) => Err(format!("{env_err}; {cli_err}")),
        },
    }
}

/// Production entry for the generic import: env Management Key first, then CLI.
pub(crate) fn load_grok_auth() -> Result<ImportedAccount, String> {
    let env = super::grok::load_grok_env_auth();
    match crate::paths::grok_auth_file() {
        Some(path) => load_grok_auth_from(env, &path),
        None => env,
    }
}

#[cfg(test)]
#[path = "grok_cli_tests.rs"]
mod tests;
