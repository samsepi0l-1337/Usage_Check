use std::path::Path;

use usage_core::account::{Credentials, Provider};

use crate::paths;

use super::{default_label, ImportedAccount};

const MISSING_FIREWORKS: &str =
    "Fireworks CLI auth not found at ~/.fireworks/auth.ini — run `firectl signin` or `firectl set-api-key`";

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

fn normalize_key(raw: &str) -> String {
    raw.trim()
        .trim_matches(|c: char| c == '"' || c == '\'')
        .replace('-', "_")
        .to_ascii_lowercase()
}

/// Tiny INI reader for firectl `auth.ini` (`account_id` + API key). Prefers
/// `[default]`, else the first section that has both fields, else top-level.
pub fn parse_fireworks_auth_ini(text: &str) -> Option<(String, String)> {
    let mut current = String::new();
    let mut sections: Vec<(String, Option<String>, Option<String>)> =
        vec![(String::new(), None, None)];

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(inner) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            current = inner.trim().to_string();
            if !sections.iter().any(|(name, _, _)| name == &current) {
                sections.push((current.clone(), None, None));
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=').or_else(|| line.split_once(':')) else {
            continue;
        };
        let value = unquote(value);
        if value.is_empty() {
            continue;
        }
        let Some(slot) = sections.iter_mut().find(|(name, _, _)| name == &current) else {
            continue;
        };
        match normalize_key(key).as_str() {
            "account_id" | "accountid" => slot.1 = Some(value),
            "api_key" | "apikey" | "key" => slot.2 = Some(value),
            _ => {}
        }
    }

    let complete = |account: &Option<String>, key: &Option<String>| -> Option<(String, String)> {
        match (account.clone(), key.clone()) {
            (Some(account), Some(key)) if !account.is_empty() && !key.is_empty() => {
                Some((account, key))
            }
            _ => None,
        }
    };

    if let Some((_, account, key)) = sections.iter().find(|(name, _, _)| name == "default") {
        if let Some(pair) = complete(account, key) {
            return Some(pair);
        }
    }
    sections
        .iter()
        .find_map(|(_, account, key)| complete(account, key))
}

pub(crate) fn load_fireworks_cli_auth_from(path: &Path) -> Result<ImportedAccount, String> {
    let text = std::fs::read_to_string(path).map_err(|_| MISSING_FIREWORKS.to_string())?;
    let (account_id, api_key) =
        parse_fireworks_auth_ini(&text).ok_or_else(|| MISSING_FIREWORKS.to_string())?;
    Ok(ImportedAccount {
        label: default_label(Provider::Fireworks),
        credentials: Credentials {
            access_token: api_key,
            refresh_token: None,
            account_id: Some(account_id),
            expires_at: None,
        },
    })
}

pub fn load_fireworks_cli_auth() -> Result<ImportedAccount, String> {
    let path = paths::fireworks_auth_ini().ok_or_else(|| MISSING_FIREWORKS.to_string())?;
    load_fireworks_cli_auth_from(&path)
}
