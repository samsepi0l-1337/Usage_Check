use usage_core::account::{Credentials, Provider};

use crate::oauth::chatgpt_account_id_from_id_token;
use crate::paths;

use super::claude::parse_expires_at;
use super::{default_label, email_from_jwt, ImportedAccount};

/// Pure parser for Codex `auth.json` body. Returns `None` when no usable
/// token is present. Also returns an optional display email from `id_token`.
pub fn parse_codex_auth_json(root: &serde_json::Value) -> Option<(Credentials, Option<String>)> {
    if let Some(tokens) = root.get("tokens") {
        let access = tokens.get("access_token")?.as_str()?.to_string();
        if access.is_empty() {
            return None;
        }
        let account_id = tokens
            .get("account_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                tokens
                    .get("id_token")
                    .and_then(|v| v.as_str())
                    .and_then(chatgpt_account_id_from_id_token)
            });
        let refresh_token = tokens
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let expires_at = tokens
            .get("expires_at")
            .and_then(parse_expires_at)
            .or_else(|| tokens.get("expires").and_then(parse_expires_at));
        let email = tokens
            .get("id_token")
            .and_then(|v| v.as_str())
            .and_then(email_from_jwt);
        return Some((
            Credentials {
                access_token: access,
                refresh_token,
                account_id,
                expires_at,
            },
            email,
        ));
    }

    // Fallback: OPENAI_API_KEY style (API key as bearer — may not work for
    // wham/usage, but matches the Swift reader's behavior).
    let access = root.get("OPENAI_API_KEY")?.as_str()?.to_string();
    if access.is_empty() {
        return None;
    }
    Some((
        Credentials {
            access_token: access,
            refresh_token: None,
            account_id: None,
            expires_at: None,
        },
        None,
    ))
}

/// Loads Codex credentials from `~/.codex/auth.json` (or `$CODEX_HOME`).
pub fn load_codex_cli_auth() -> Result<ImportedAccount, String> {
    let path =
        paths::codex_auth_file().ok_or_else(|| "could not resolve home directory".to_string())?;
    let data = std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "Codex auth not found at {} — run `codex login` first",
            path.display()
        )
    })?;
    let root: serde_json::Value =
        serde_json::from_str(&data).map_err(|_| "Codex auth.json is not valid JSON".to_string())?;
    let (credentials, email) = parse_codex_auth_json(&root)
        .ok_or_else(|| "Codex auth.json has no usable access_token".to_string())?;
    Ok(ImportedAccount {
        label: email.unwrap_or_else(|| default_label(Provider::Codex)),
        credentials,
    })
}
