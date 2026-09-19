use std::path::Path;

use usage_core::account::{Credentials, Provider};

use crate::paths;

use super::{default_label, ImportedAccount};

const OPENCODE_GO_LOGIN: &str =
    "OpenCode Go auth not found — run `opencode auth login` and choose OpenCode Go";

fn api_key_from_entry(entry: &serde_json::Value) -> Option<String> {
    entry
        .get("key")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Reads only the `opencode-go` API key. Other entries in `auth.json` (Zen,
/// OpenRouter, …) are ignored.
pub fn parse_opencode_go_auth_json(root: &serde_json::Value) -> Option<String> {
    api_key_from_entry(root.get("opencode-go")?)
}

pub(crate) fn load_opencode_cli_auth_from(path: &Path) -> Result<ImportedAccount, String> {
    let data = std::fs::read_to_string(path).map_err(|_| OPENCODE_GO_LOGIN.to_string())?;
    let root: serde_json::Value = serde_json::from_str(&data)
        .map_err(|_| "OpenCode auth.json is not valid JSON".to_string())?;
    let key = parse_opencode_go_auth_json(&root).ok_or_else(|| OPENCODE_GO_LOGIN.to_string())?;
    Ok(ImportedAccount {
        label: default_label(Provider::OpenCode),
        credentials: Credentials {
            access_token: key,
            refresh_token: None,
            account_id: None,
            expires_at: None,
        },
    })
}

pub fn load_opencode_cli_auth() -> Result<ImportedAccount, String> {
    let path = paths::opencode_auth_file().ok_or_else(|| OPENCODE_GO_LOGIN.to_string())?;
    load_opencode_cli_auth_from(&path)
}
