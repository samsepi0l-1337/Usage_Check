use std::path::Path;

use usage_core::account::{Credentials, Provider};

use crate::paths;

use super::{default_label, ImportedAccount};

const MISSING_DSH: &str =
    "install DeepSeek Harness (`npx @deepseek-ai/dsh`) or put DEEPSEEK_API_KEY in ~/.dsh/.env";

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2)
        || (trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2)
    {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Tiny parser for the two DeepSeek Harness shapes (`refs.DEEPSEEK_API_KEY`
/// YAML and a flat `DEEPSEEK_API_KEY:` / `.env` assignment). Not a YAML crate.
pub fn parse_deepseek_api_key(text: &str) -> Option<String> {
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim();
        let Some(rest) = line.strip_prefix("DEEPSEEK_API_KEY") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(value) = rest
            .strip_prefix(':')
            .or_else(|| rest.strip_prefix('='))
            .map(str::trim)
        else {
            continue;
        };
        let key = unquote(value);
        if !key.is_empty() {
            return Some(key);
        }
    }
    None
}

pub(crate) fn load_deepseek_cli_auth_from(dsh_home: &Path) -> Result<ImportedAccount, String> {
    let yaml = dsh_home.join(".credentials.yaml");
    let env = dsh_home.join(".env");
    let key = std::fs::read_to_string(&yaml)
        .ok()
        .and_then(|text| parse_deepseek_api_key(&text))
        .or_else(|| {
            std::fs::read_to_string(&env)
                .ok()
                .and_then(|text| parse_deepseek_api_key(&text))
        })
        .ok_or_else(|| MISSING_DSH.to_string())?;
    Ok(ImportedAccount {
        label: default_label(Provider::DeepSeek),
        credentials: Credentials {
            access_token: key,
            refresh_token: None,
            account_id: None,
            expires_at: None,
        },
    })
}

pub fn load_deepseek_cli_auth() -> Result<ImportedAccount, String> {
    let home = paths::dsh_home().ok_or_else(|| MISSING_DSH.to_string())?;
    load_deepseek_cli_auth_from(&home)
}
