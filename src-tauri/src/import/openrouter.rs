use std::path::Path;

use usage_core::account::{Credentials, Provider};

use crate::paths;

use super::{default_label, ImportedAccount};

const MISSING_OPENROUTER: &str =
    "OpenRouter API key not found — add one to ~/.ori/config.json or run `opencode auth login` for OpenRouter";

fn nonempty_key(v: &serde_json::Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn looks_like_openrouter_key(key: &str) -> bool {
    key.starts_with("sk-or-") || key.starts_with("sk-or")
}

/// Pulls an API key from Ori `config.json` if one is present.
pub fn parse_ori_api_key(root: &serde_json::Value) -> Option<String> {
    if let Some(key) = root
        .get("env")
        .and_then(|env| env.get("OPENROUTER_API_KEY"))
        .and_then(nonempty_key)
    {
        return Some(key);
    }
    for key_name in [
        "api_key",
        "apiKey",
        "openrouter_api_key",
        "OPENROUTER_API_KEY",
    ] {
        if let Some(key) = root.get(key_name).and_then(nonempty_key) {
            return Some(key);
        }
    }
    if let Some(nested) = root.get("openrouter") {
        for key_name in ["key", "api_key", "apiKey"] {
            if let Some(key) = nested.get(key_name).and_then(nonempty_key) {
                return Some(key);
            }
        }
    }
    root.get("key")
        .and_then(nonempty_key)
        .filter(|key| looks_like_openrouter_key(key))
}

/// Reads only the `openrouter` entry from OpenCode `auth.json`.
pub fn parse_openrouter_opencode_auth(root: &serde_json::Value) -> Option<String> {
    root.get("openrouter")
        .and_then(|entry| entry.get("key"))
        .and_then(nonempty_key)
}

fn load_json(path: &Path) -> Option<serde_json::Value> {
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

pub(crate) fn load_openrouter_cli_auth_from(
    ori_config: Option<&Path>,
    opencode_auth: Option<&Path>,
) -> Result<ImportedAccount, String> {
    if let Some(path) = ori_config {
        if let Some(root) = load_json(path) {
            if let Some(key) = parse_ori_api_key(&root) {
                return Ok(ImportedAccount {
                    label: default_label(Provider::OpenRouter),
                    credentials: Credentials {
                        access_token: key,
                        refresh_token: None,
                        account_id: None,
                        expires_at: None,
                    },
                });
            }
        }
    }
    if let Some(path) = opencode_auth {
        if let Some(root) = load_json(path) {
            if let Some(key) = parse_openrouter_opencode_auth(&root) {
                return Ok(ImportedAccount {
                    label: default_label(Provider::OpenRouter),
                    credentials: Credentials {
                        access_token: key,
                        refresh_token: None,
                        account_id: None,
                        expires_at: None,
                    },
                });
            }
        }
    }
    Err(MISSING_OPENROUTER.into())
}

pub fn load_openrouter_cli_auth() -> Result<ImportedAccount, String> {
    load_openrouter_cli_auth_from(
        paths::ori_config_file().as_deref(),
        paths::opencode_auth_file().as_deref(),
    )
}
