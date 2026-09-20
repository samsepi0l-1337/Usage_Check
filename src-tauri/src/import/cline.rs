use std::path::{Path, PathBuf};

use chrono::{TimeZone, Utc};
use usage_core::account::{Credentials, Provider};

use crate::paths;

use super::{default_label, email_from_jwt, ImportedAccount};

const MISSING_CLINE: &str =
    "Cline account token not found — sign in with `cline auth` so ~/.cline/data/secrets.json has clineApiKey or cline:clineAccountId (BYOK-only cannot show quota)";

fn nonempty(v: &serde_json::Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn first_string(root: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| root.get(*key).and_then(nonempty))
}

fn strip_workos(token: &str) -> &str {
    token
        .strip_prefix("workos:")
        .or_else(|| token.strip_prefix("WorkOS:"))
        .unwrap_or(token)
}

fn jwt_email(token: &str) -> Option<String> {
    email_from_jwt(strip_workos(token))
}

fn expires_from_epoch(n: f64) -> Option<chrono::DateTime<Utc>> {
    if !n.is_finite() || n <= 0.0 {
        return None;
    }
    let seconds = if n.abs() >= 10_000_000_000.0 {
        n / 1000.0
    } else {
        n
    };
    Utc.timestamp_opt(seconds as i64, 0).single()
}

fn imported(
    access: String,
    refresh: Option<String>,
    account_id: Option<String>,
    expires_at: Option<chrono::DateTime<Utc>>,
    email: Option<String>,
) -> ImportedAccount {
    let label = email
        .or_else(|| jwt_email(&access))
        .unwrap_or_else(|| default_label(Provider::Cline));
    ImportedAccount {
        label,
        credentials: Credentials {
            access_token: access,
            refresh_token: refresh,
            account_id,
            expires_at,
        },
    }
}

/// `cline:clineAccountId` is a JSON blob of the WorkOS session (idToken + user id).
fn parse_account_blob(raw: &str) -> Option<ImportedAccount> {
    let root: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
    let access = first_string(
        &root,
        &["idToken", "id_token", "accessToken", "access_token"],
    )?;
    let refresh = first_string(&root, &["refreshToken", "refresh_token"]);
    let user = root.get("userInfo").or_else(|| root.get("user"));
    let account_id = user
        .and_then(|u| first_string(u, &["id", "clineUserId", "cline_user_id"]))
        .or_else(|| first_string(&root, &["accountId", "account_id"]));
    let email = user.and_then(|u| first_string(u, &["email"]));
    let expires = root
        .get("expiresAt")
        .or_else(|| root.get("expires_at"))
        .and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_i64().map(|n| n as f64))
                .or_else(|| v.as_u64().map(|n| n as f64))
        })
        .and_then(expires_from_epoch);
    Some(imported(access, refresh, account_id, expires, email))
}

/// Reads Cline account auth from `secrets.json` (SecretKeys: clineApiKey / cline:clineAccountId).
/// Other provider keys (BYOK) are ignored — they cannot poll Cline quota.
pub fn parse_cline_secrets_json(root: &serde_json::Value) -> Option<ImportedAccount> {
    if let Some(raw) = first_string(root, &["cline:clineAccountId"]) {
        if let Some(imported) = parse_account_blob(&raw) {
            return Some(imported);
        }
    }
    let access = first_string(root, &["clineApiKey", "cline_api_key"])?;
    let account_id = first_string(root, &["clineAccountId", "cline_account_id"]);
    Some(imported(access, None, account_id, None, None))
}

fn auth_object<'a>(
    root: &'a serde_json::Value,
    provider_id: &str,
) -> Option<&'a serde_json::Value> {
    let providers = root.get("providers")?;
    let entry = providers.get(provider_id)?;
    entry
        .get("settings")
        .and_then(|s| s.get("auth"))
        .or_else(|| entry.get("auth"))
}

fn api_key_from_provider(root: &serde_json::Value, provider_id: &str) -> Option<String> {
    let providers = root.get("providers")?;
    let entry = providers.get(provider_id)?;
    let settings = entry.get("settings").unwrap_or(entry);
    first_string(settings, &["apiKey", "api_key"])
}

/// Documented CLI login file `~/.cline/data/settings/providers.json`.
pub fn parse_cline_providers_json(root: &serde_json::Value) -> Option<ImportedAccount> {
    for provider_id in ["cline", "cline-pass"] {
        if let Some(auth) = auth_object(root, provider_id) {
            if let Some(access) = first_string(
                auth,
                &["accessToken", "access_token", "idToken", "id_token"],
            ) {
                let refresh = first_string(auth, &["refreshToken", "refresh_token"]);
                let account_id = first_string(auth, &["accountId", "account_id"]);
                let email = first_string(auth, &["email"]);
                let expires = auth
                    .get("expiresAt")
                    .or_else(|| auth.get("expires_at"))
                    .and_then(|v| {
                        v.as_f64()
                            .or_else(|| v.as_i64().map(|n| n as f64))
                            .or_else(|| v.as_u64().map(|n| n as f64))
                    })
                    .and_then(expires_from_epoch);
                return Some(imported(access, refresh, account_id, expires, email));
            }
        }
        if let Some(key) = api_key_from_provider(root, provider_id) {
            return Some(imported(key, None, None, None, None));
        }
    }
    None
}

fn load_json(path: &Path) -> Option<serde_json::Value> {
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

pub(crate) fn load_cline_cli_auth_from(
    secrets: &[PathBuf],
    providers: &[PathBuf],
) -> Result<ImportedAccount, String> {
    for path in secrets {
        if let Some(root) = load_json(path) {
            if let Some(imported) = parse_cline_secrets_json(&root) {
                return Ok(imported);
            }
        }
    }
    for path in providers {
        if let Some(root) = load_json(path) {
            if let Some(imported) = parse_cline_providers_json(&root) {
                return Ok(imported);
            }
        }
    }
    Err(MISSING_CLINE.into())
}

pub fn load_cline_cli_auth() -> Result<ImportedAccount, String> {
    load_cline_cli_auth_from(
        &paths::cline_secrets_files(),
        &paths::cline_providers_files(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn parses_cline_api_key_and_account_id() {
        let imported = parse_cline_secrets_json(&json!({
            "clineApiKey": "ck-test",
            "clineAccountId": "user_abc"
        }))
        .unwrap();
        assert_eq!(imported.credentials.access_token, "ck-test");
        assert_eq!(imported.credentials.account_id.as_deref(), Some("user_abc"));
    }

    #[test]
    fn parses_session_blob_before_api_key() {
        let blob = serde_json::json!({
            "idToken": "jwt-access",
            "refreshToken": "rt",
            "expiresAt": 1899578924,
            "userInfo": { "id": "user_1", "email": "you@cline.bot" }
        })
        .to_string();
        let imported = parse_cline_secrets_json(&json!({
            "clineApiKey": "ck-ignored",
            "cline:clineAccountId": blob
        }))
        .unwrap();
        assert_eq!(imported.credentials.access_token, "jwt-access");
        assert_eq!(imported.credentials.refresh_token.as_deref(), Some("rt"));
        assert_eq!(imported.credentials.account_id.as_deref(), Some("user_1"));
        assert_eq!(imported.label, "you@cline.bot");
        assert!(imported.credentials.expires_at.is_some());
    }

    #[test]
    fn byok_only_secrets_are_not_cline() {
        assert!(parse_cline_secrets_json(&json!({
            "openRouterApiKey": "sk-or-x",
            "anthropicApiKey": "sk-ant"
        }))
        .is_none());
    }

    #[test]
    fn parses_providers_json_oauth() {
        let imported = parse_cline_providers_json(&json!({
            "providers": {
                "cline": {
                    "settings": {
                        "auth": {
                            "accessToken": "workos:jwt",
                            "refreshToken": "rt",
                            "accountId": "user_9"
                        }
                    }
                }
            }
        }))
        .unwrap();
        assert_eq!(imported.credentials.access_token, "workos:jwt");
        assert_eq!(imported.credentials.account_id.as_deref(), Some("user_9"));
    }

    #[test]
    fn load_prefers_secrets_over_providers() {
        let dir = TempDir::new().unwrap();
        let secrets = dir.path().join("secrets.json");
        std::fs::write(&secrets, r#"{"clineApiKey":"from-secrets"}"#).unwrap();
        let providers = dir.path().join("providers.json");
        std::fs::write(
            &providers,
            r#"{"providers":{"cline":{"settings":{"apiKey":"from-providers"}}}}"#,
        )
        .unwrap();
        let imported = load_cline_cli_auth_from(&[secrets], &[providers]).unwrap();
        assert_eq!(imported.credentials.access_token, "from-secrets");
    }

    #[test]
    fn missing_files_need_setup() {
        let dir = TempDir::new().unwrap();
        let err = load_cline_cli_auth_from(
            &[dir.path().join("secrets.json")],
            &[dir.path().join("providers.json")],
        )
        .unwrap_err();
        assert!(err.contains("Cline account"), "{err}");
        assert!(err.contains("BYOK-only"), "{err}");
    }
}
