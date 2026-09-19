use std::path::Path;

use usage_core::account::{Credentials, Provider};

use crate::paths;

use super::{default_label, ImportedAccount};

const MISSING: &str = "GitHub Copilot token not found — sign in to Copilot in VS Code or `gh auth login`, then import from ~/.config/github-copilot";

fn nonempty_str(v: &serde_json::Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn first_named_string(value: &serde_json::Value, names: &[&str]) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            for name in names {
                if let Some(found) = map.get(*name).and_then(nonempty_str) {
                    return Some(found);
                }
            }
            map.values()
                .find_map(|child| first_named_string(child, names))
        }
        serde_json::Value::Array(items) => items
            .iter()
            .find_map(|child| first_named_string(child, names)),
        _ => None,
    }
}

/// Walks Copilot / gh JSON for the first non-empty `oauth_token`. Never logs it.
pub fn parse_copilot_oauth_token(root: &serde_json::Value) -> Option<String> {
    first_named_string(root, &["oauth_token"])
}

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

/// Tiny github.com-section scanner for `gh` `hosts.yml`. Not a YAML crate.
pub fn parse_gh_hosts_yml(text: &str) -> Option<String> {
    let mut in_github = false;
    for raw in text.lines() {
        let indent = raw.len() - raw.trim_start().len();
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if indent == 0 && line.ends_with(':') {
            let host = line.trim_end_matches(':').trim();
            in_github = host.eq_ignore_ascii_case("github.com");
            continue;
        }
        if !in_github {
            continue;
        }
        for key in ["oauth_token", "token"] {
            let Some(rest) = line.strip_prefix(key) else {
                continue;
            };
            let rest = rest.trim_start();
            let Some(value) = rest.strip_prefix(':').map(str::trim) else {
                continue;
            };
            let token = unquote(value);
            if !token.is_empty() {
                return Some(token);
            }
        }
    }
    None
}

fn imported_from_token(token: String, label: Option<String>) -> ImportedAccount {
    ImportedAccount {
        label: label.unwrap_or_else(|| default_label(Provider::Copilot)),
        credentials: Credentials {
            access_token: token,
            refresh_token: None,
            account_id: None,
            expires_at: None,
        },
    }
}

fn load_copilot_json(path: &Path) -> Option<ImportedAccount> {
    let data = std::fs::read_to_string(path).ok()?;
    let root: serde_json::Value = serde_json::from_str(&data).ok()?;
    let token = parse_copilot_oauth_token(&root)?;
    let label = first_named_string(&root, &["user", "login"]);
    Some(imported_from_token(token, label))
}

pub(crate) fn load_copilot_cli_auth_from_files(
    json_files: &[std::path::PathBuf],
    hosts_yml: &[std::path::PathBuf],
) -> Result<ImportedAccount, String> {
    for path in json_files {
        if let Some(imported) = load_copilot_json(path) {
            return Ok(imported);
        }
    }
    for path in hosts_yml {
        if let Some(token) = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| parse_gh_hosts_yml(&text))
        {
            return Ok(imported_from_token(token, None));
        }
    }
    Err(MISSING.into())
}

/// First usable local Copilot / gh token. Does not read `GITHUB_TOKEN`.
pub fn load_copilot_cli_auth() -> Result<ImportedAccount, String> {
    load_copilot_cli_auth_from_files(
        &paths::github_copilot_token_files(),
        &paths::gh_hosts_yml_files(),
    )
}
