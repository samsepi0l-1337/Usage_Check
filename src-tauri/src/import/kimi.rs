use std::path::Path;

use chrono::Utc;
use usage_core::account::Credentials;

use super::claude::parse_expires_at;
use crate::paths;

use super::{default_label, email_from_jwt, ImportedAccount};

fn nonempty_str(root: &serde_json::Value, key: &str) -> Option<String> {
    root.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Parses a Kimi Code credentials JSON object. Tokens are never logged.
pub fn parse_kimi_credentials_json(root: &serde_json::Value) -> Option<Credentials> {
    let access = nonempty_str(root, "access_token")?;
    let refresh_token = nonempty_str(root, "refresh_token");
    let expires_at = root.get("expires_at").and_then(parse_expires_at);
    if expires_at.is_some_and(|exp| exp <= Utc::now()) && refresh_token.is_none() {
        return None;
    }
    Some(Credentials {
        access_token: access,
        refresh_token,
        account_id: None,
        expires_at,
    })
}

fn load_kimi_file(path: &Path) -> Option<ImportedAccount> {
    let data = std::fs::read_to_string(path).ok()?;
    let root: serde_json::Value = serde_json::from_str(&data).ok()?;
    let credentials = parse_kimi_credentials_json(&root)?;
    let label = email_from_jwt(&credentials.access_token)
        .unwrap_or_else(|| default_label(usage_core::account::Provider::Kimi));
    Some(ImportedAccount { credentials, label })
}

pub(crate) fn load_kimi_cli_auth_from_files(
    files: &[std::path::PathBuf],
) -> Result<ImportedAccount, String> {
    for path in files {
        if let Some(imported) = load_kimi_file(path) {
            return Ok(imported);
        }
    }
    Err(
        "Kimi Code credentials not found — sign in with kimi-code, then import from ~/.kimi-code/credentials"
            .into(),
    )
}

/// First usable Kimi OAuth file from the documented credential locations.
pub fn load_kimi_cli_auth() -> Result<ImportedAccount, String> {
    load_kimi_cli_auth_from_files(&paths::kimi_credential_files())
}
