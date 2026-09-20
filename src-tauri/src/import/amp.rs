use std::path::Path;

use usage_core::account::{Credentials, Provider};

use crate::paths;

use super::{default_label, ImportedAccount};

const MISSING_AMP: &str =
    "Amp API key not found — install Amp and run `amp login` (writes ~/.local/share/amp/secrets.json)";

fn nonempty(v: &serde_json::Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Reads `apiKey@https://ampcode.com/` (or any `apiKey@…` fallback).
pub fn parse_amp_secrets(root: &serde_json::Value) -> Option<String> {
    let obj = root.as_object()?;
    if let Some(key) = obj.get("apiKey@https://ampcode.com/").and_then(nonempty) {
        return Some(key);
    }
    for (name, value) in obj {
        if name.starts_with("apiKey@") {
            if let Some(key) = nonempty(value) {
                return Some(key);
            }
        }
    }
    None
}

pub(crate) fn load_amp_cli_auth_from(path: &Path) -> Result<ImportedAccount, String> {
    let data = std::fs::read_to_string(path).map_err(|_| MISSING_AMP.to_string())?;
    let root: serde_json::Value = serde_json::from_str(&data)
        .map_err(|_| "Amp secrets.json is not valid JSON".to_string())?;
    let key = parse_amp_secrets(&root).ok_or_else(|| MISSING_AMP.to_string())?;
    Ok(ImportedAccount {
        label: default_label(Provider::Amp),
        credentials: Credentials {
            access_token: key,
            refresh_token: None,
            account_id: None,
            expires_at: None,
        },
    })
}

pub fn load_amp_cli_auth() -> Result<ImportedAccount, String> {
    let path = paths::amp_secrets_file().ok_or_else(|| MISSING_AMP.to_string())?;
    load_amp_cli_auth_from(&path)
}
