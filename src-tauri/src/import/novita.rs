use std::path::Path;

use usage_core::account::{Credentials, Provider};

use crate::paths;

use super::{default_label, ImportedAccount};

const MISSING_NOVITA: &str =
    "Novita CLI credentials not found at ~/.novita/config.json — run `novita auth login`";

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

fn team_key(root: &serde_json::Value) -> Option<String> {
    for team_key in ["team", "selectedTeam", "selected_team", "activeTeam"] {
        if let Some(team) = root.get(team_key) {
            if let Some(key) = key_from_object(
                team,
                &["apiKey", "api_key", "teamApiKey", "team_api_key", "key"],
            ) {
                return Some(key);
            }
        }
    }
    None
}

/// Team API key preferred (billing uses Bearer API key); session token last.
pub fn parse_novita_config(root: &serde_json::Value) -> Option<(String, Option<String>)> {
    let key = team_key(root).or_else(|| {
        key_from_object(
            root,
            &[
                "teamApiKey",
                "team_api_key",
                "apiKey",
                "api_key",
                "key",
                "token",
                "accessToken",
                "access_token",
            ],
        )
    })?;
    let email = key_from_object(root, &["email", "userEmail", "user_email"]).or_else(|| {
        root.get("user")
            .and_then(|user| key_from_object(user, &["email"]))
    });
    let team_name = ["team", "selectedTeam", "selected_team", "activeTeam"]
        .into_iter()
        .find_map(|k| {
            root.get(k)
                .and_then(|team| key_from_object(team, &["name", "teamName", "displayName"]))
        });
    let label = email.or(team_name);
    Some((key, label))
}

pub(crate) fn load_novita_cli_auth_from(path: &Path) -> Result<ImportedAccount, String> {
    let data = std::fs::read_to_string(path).map_err(|_| MISSING_NOVITA.to_string())?;
    let root: serde_json::Value = serde_json::from_str(&data)
        .map_err(|_| "Novita config.json is not valid JSON".to_string())?;
    let (key, label) = parse_novita_config(&root).ok_or_else(|| MISSING_NOVITA.to_string())?;
    Ok(ImportedAccount {
        label: label.unwrap_or_else(|| default_label(Provider::Novita)),
        credentials: Credentials {
            access_token: key,
            refresh_token: None,
            account_id: None,
            expires_at: None,
        },
    })
}

pub fn load_novita_cli_auth() -> Result<ImportedAccount, String> {
    let path = paths::novita_config_json().ok_or_else(|| MISSING_NOVITA.to_string())?;
    load_novita_cli_auth_from(&path)
}
