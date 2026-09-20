use std::path::Path;

use usage_core::account::{Credentials, Provider};

use crate::paths;

use super::{default_label, ImportedAccount};

const MISSING_ZAI: &str =
    "Z.AI API key not found — add a `zai` key via `opencode auth login`, or put one in ~/.zcode/v2/config.json";

fn nonempty(v: &serde_json::Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn key_from_object(obj: &serde_json::Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| obj.get(*name).and_then(nonempty))
}

fn zai_entry_key(root: &serde_json::Value) -> Option<String> {
    // Only the zai coding-plan slots — never OpenRouter / OpenCode Go / etc.
    for name in ["zai", "zai-coding-plan"] {
        if let Some(entry) = root.get(name) {
            if let Some(key) = key_from_object(entry, &["key", "api_key", "apiKey"]) {
                return Some(key);
            }
        }
    }
    None
}

/// OpenCode `auth.json` `zai` (or `zai-coding-plan`) entry only.
pub fn parse_opencode_zai_auth(root: &serde_json::Value) -> Option<String> {
    zai_entry_key(root)
}

/// Hermes `auth.json` zai entry.
pub fn parse_hermes_zai_auth(root: &serde_json::Value) -> Option<String> {
    zai_entry_key(root)
}

/// zcode `~/.zcode/v2/config.json` API key field.
pub fn parse_zcode_config_key(root: &serde_json::Value) -> Option<String> {
    if let Some(key) = key_from_object(
        root,
        &[
            "apiKey",
            "api_key",
            "key",
            "zaiApiKey",
            "zai_api_key",
            "ZAI_API_KEY",
        ],
    ) {
        return Some(key);
    }
    root.get("zai")
        .or_else(|| root.get("zcode"))
        .or_else(|| root.get("auth"))
        .and_then(|nested| key_from_object(nested, &["key", "api_key", "apiKey"]))
}

fn load_json(path: &Path) -> Option<serde_json::Value> {
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

pub(crate) fn load_zai_cli_auth_from(
    opencode_auth: Option<&Path>,
    zcode_config: Option<&Path>,
    hermes_auth: Option<&Path>,
) -> Result<ImportedAccount, String> {
    if let Some(path) = opencode_auth {
        if let Some(root) = load_json(path) {
            if let Some(key) = parse_opencode_zai_auth(&root) {
                return Ok(ImportedAccount {
                    label: default_label(Provider::Zai),
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
    if let Some(path) = zcode_config {
        if let Some(root) = load_json(path) {
            if let Some(key) = parse_zcode_config_key(&root) {
                return Ok(ImportedAccount {
                    label: default_label(Provider::Zai),
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
    if let Some(path) = hermes_auth {
        if let Some(root) = load_json(path) {
            if let Some(key) = parse_hermes_zai_auth(&root) {
                return Ok(ImportedAccount {
                    label: default_label(Provider::Zai),
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
    Err(MISSING_ZAI.into())
}

pub fn load_zai_cli_auth() -> Result<ImportedAccount, String> {
    load_zai_cli_auth_from(
        paths::opencode_auth_file().as_deref(),
        paths::zcode_config_json().as_deref(),
        paths::hermes_auth_file().as_deref(),
    )
}
