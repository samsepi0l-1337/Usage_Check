use std::path::Path;

use usage_core::account::{Credentials, Provider};

use crate::paths;

use super::{default_label, email_from_jwt, ImportedAccount};

const MISSING_FACTORY: &str =
    "Factory auth.json not found — run `droid` login first (writes ~/.factory/auth.json)";
const ENCRYPTED_V2: &str =
    "Factory auth.v2 store is encrypted — UsageCheck does not decrypt it. Use plaintext ~/.factory/auth.json";

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

/// Reads plaintext Factory `auth.json`. Encrypted v2 is handled by the loader.
pub fn parse_factory_auth_json(root: &serde_json::Value) -> Option<ImportedAccount> {
    let access = first_string(root, &["access_token", "accessToken", "token"])?;
    let refresh = first_string(root, &["refresh_token", "refreshToken"]);
    let org = first_string(root, &["org_id", "orgId", "organization_id"]);
    let label = first_string(root, &["email"])
        .or_else(|| email_from_jwt(&access))
        .unwrap_or_else(|| default_label(Provider::Factory));
    Some(ImportedAccount {
        label,
        credentials: Credentials {
            access_token: access,
            refresh_token: refresh,
            account_id: org,
            expires_at: None,
        },
    })
}

pub(crate) fn load_factory_cli_auth_from(
    auth_json: &Path,
    v2_file: Option<&Path>,
) -> Result<ImportedAccount, String> {
    if auth_json.is_file() {
        let data = std::fs::read_to_string(auth_json).map_err(|_| MISSING_FACTORY.to_string())?;
        let root: serde_json::Value = serde_json::from_str(&data)
            .map_err(|_| "Factory auth.json is not valid JSON".to_string())?;
        return parse_factory_auth_json(&root).ok_or_else(|| MISSING_FACTORY.to_string());
    }
    if v2_file.is_some_and(|p| p.is_file()) {
        return Err(ENCRYPTED_V2.to_string());
    }
    Err(MISSING_FACTORY.to_string())
}

pub fn load_factory_cli_auth() -> Result<ImportedAccount, String> {
    let auth_json = paths::factory_auth_json().ok_or_else(|| MISSING_FACTORY.to_string())?;
    let v2 = paths::factory_auth_v2_file();
    load_factory_cli_auth_from(&auth_json, v2.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn parses_plaintext_auth_json() {
        let imported = parse_factory_auth_json(&json!({
            "access_token": "at-factory",
            "refresh_token": "rt-factory",
            "org_id": "org_1"
        }))
        .unwrap();
        assert_eq!(imported.credentials.access_token, "at-factory");
        assert_eq!(
            imported.credentials.refresh_token.as_deref(),
            Some("rt-factory")
        );
        assert_eq!(imported.credentials.account_id.as_deref(), Some("org_1"));
    }

    #[test]
    fn load_prefers_plaintext_over_v2() {
        let dir = TempDir::new().unwrap();
        let auth = dir.path().join("auth.json");
        std::fs::write(&auth, r#"{"access_token":"plain"}"#).unwrap();
        let v2 = dir.path().join("auth.v2.file");
        std::fs::write(&v2, "encrypted-blob").unwrap();
        let imported = load_factory_cli_auth_from(&auth, Some(&v2)).unwrap();
        assert_eq!(imported.credentials.access_token, "plain");
    }

    #[test]
    fn encrypted_v2_without_plaintext_fails_closed() {
        let dir = TempDir::new().unwrap();
        let auth = dir.path().join("auth.json");
        let v2 = dir.path().join("auth.v2.file");
        std::fs::write(&v2, "encrypted-blob").unwrap();
        let err = load_factory_cli_auth_from(&auth, Some(&v2)).unwrap_err();
        assert!(err.contains("encrypted"), "{err}");
        assert!(err.contains("does not decrypt"), "{err}");
    }

    #[test]
    fn missing_files_need_setup() {
        let dir = TempDir::new().unwrap();
        let err = load_factory_cli_auth_from(&dir.path().join("auth.json"), None).unwrap_err();
        assert!(err.contains("auth.json"), "{err}");
    }
}
