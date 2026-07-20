use usage_core::account::Credentials;

use super::ImportedAccount;

#[cfg(feature = "edition-pro")]
/// xAI Management API credentials from `XAI_MGMT_KEY` + `XAI_TEAM_ID`.
pub fn load_grok_env_auth() -> Result<ImportedAccount, String> {
    let key = std::env::var("XAI_MGMT_KEY")
        .or_else(|_| std::env::var("XAI_MANAGEMENT_KEY"))
        .map_err(|_| {
            "set XAI_MGMT_KEY (or XAI_MANAGEMENT_KEY) with your xAI Management Key".to_string()
        })?;
    if key.trim().is_empty() {
        return Err("XAI_MGMT_KEY is empty".into());
    }
    let team_id = std::env::var("XAI_TEAM_ID")
        .map_err(|_| "set XAI_TEAM_ID with your xAI team ID".to_string())?;
    if team_id.trim().is_empty() {
        return Err("XAI_TEAM_ID is empty".into());
    }
    grok_imported_account(&key, &team_id)
}

#[cfg(feature = "edition-pro")]
pub(crate) fn grok_imported_account(key: &str, team_id: &str) -> Result<ImportedAccount, String> {
    use usage_core::fetch::grok::is_valid_team_id;

    let team_id = team_id.trim();
    if !is_valid_team_id(team_id) {
        return Err(format!(
            "'{team_id}' is not a valid xAI team id (must be a single token with no spaces) — \
             management-key validation failed and the pasted/`XAI_TEAM_ID` fallback isn't a team id. \
             Paste your xAI Management Key (and team id on its own line), or set XAI_TEAM_ID."
        ));
    }
    Ok(ImportedAccount {
        label: format!("Grok · team {team_id}"),
        credentials: Credentials {
            access_token: key.trim().to_string(),
            refresh_token: None,
            account_id: Some(team_id.to_string()),
            expires_at: None,
        },
    })
}

#[cfg(feature = "edition-pro")]
fn read_clipboard_text() -> Result<String, String> {
    arboard::Clipboard::new()
        .map_err(|e| format!("clipboard unavailable: {e}"))?
        .get_text()
        .map_err(|_| {
            "clipboard has no text — copy your xAI Management Key, then try again".to_string()
        })
}

#[cfg(feature = "edition-pro")]
/// Validates a Management Key via the official xAI endpoint and resolves team ID.
pub async fn validate_grok_management_key(key: &str) -> Result<String, String> {
    use usage_core::fetch::grok::team_id_from_validation;

    let client = reqwest::Client::new();
    let resp = client
        .get("https://management-api.x.ai/auth/management-keys/validation")
        .header("Accept", "application/json")
        .header("User-Agent", "UsageCheck")
        .bearer_auth(key.trim())
        .send()
        .await
        .map_err(|e| format!("management key validation request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "management key validation failed (HTTP {})",
            resp.status()
        ));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|_| "management key validation response is not valid JSON".to_string())?;
    team_id_from_validation(&body)
        .ok_or_else(|| "validation succeeded but response has no team/scope id".to_string())
}

#[cfg(feature = "edition-pro")]
/// Imports Grok from the system clipboard: validates the Management Key, or
/// falls back to a pasted team ID / `XAI_TEAM_ID` when validation cannot
/// resolve scope.
pub async fn import_grok_from_clipboard() -> Result<ImportedAccount, String> {
    use usage_core::fetch::grok::parse_grok_paste;

    let text = read_clipboard_text()?;
    let (key, pasted_team) = parse_grok_paste(&text);
    if key.is_empty() {
        return Err(
            "clipboard is empty — copy your xAI Management Key, then choose Import Grok (clipboard)"
                .into(),
        );
    }

    match validate_grok_management_key(&key).await {
        Ok(team_id) => grok_imported_account(&key, &team_id),
        Err(validation_err) => {
            let team_id = pasted_team
                .or_else(|| {
                    std::env::var("XAI_TEAM_ID")
                        .ok()
                        .filter(|s| !s.trim().is_empty())
                })
                .ok_or_else(|| {
                    format!(
                        "{validation_err} — paste key and team ID on separate lines, \
                         set XAI_TEAM_ID, or use Import Grok (env vars)"
                    )
                })?;
            grok_imported_account(&key, &team_id)
        }
    }
}
