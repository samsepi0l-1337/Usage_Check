use std::path::Path;

use chrono::{DateTime, Utc};
use usage_core::account::{Credentials, Provider};

use crate::paths;

use super::{default_label, email_from_jwt, ImportedAccount};

const MISSING_KIRO: &str =
    "Kiro auth token not found — sign in to Kiro first (writes ~/.aws/sso/cache/kiro-auth-token.json)";

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

/// Region from `region` / `awsRegion` or `profileArn` (`arn:aws:codewhisperer:<region>:…`).
pub fn kiro_region_from_token(root: &serde_json::Value) -> Option<String> {
    if let Some(region) = first_string(root, &["region", "awsRegion", "aws_region"]) {
        return Some(region);
    }
    region_from_profile_arn(&first_string(root, &["profileArn", "profile_arn", "arn"])?)
}

pub fn region_from_profile_arn(arn: &str) -> Option<String> {
    // arn:aws:codewhisperer:us-east-1:123:profile/…
    let region = arn.split(':').nth(3)?.trim();
    (!region.is_empty()).then(|| region.to_string())
}

fn parse_expiry(root: &serde_json::Value) -> Option<DateTime<Utc>> {
    let raw = first_string(root, &["expiresAt", "expires_at", "expiry"])?;
    DateTime::parse_from_rfc3339(&raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Reads Kiro desktop `kiro-auth-token.json` (never logs token values).
pub fn parse_kiro_auth_token(root: &serde_json::Value) -> Option<ImportedAccount> {
    let access = first_string(root, &["accessToken", "access_token", "token"])?;
    let refresh = first_string(root, &["refreshToken", "refresh_token"]);
    let profile_arn = first_string(root, &["profileArn", "profile_arn", "arn"]);
    let label = first_string(root, &["email", "provider", "authMethod"])
        .or_else(|| email_from_jwt(&access))
        .or_else(|| profile_arn.clone())
        .unwrap_or_else(|| default_label(Provider::Kiro));
    Some(ImportedAccount {
        label,
        credentials: Credentials {
            access_token: access,
            refresh_token: refresh,
            account_id: profile_arn,
            expires_at: parse_expiry(root),
        },
    })
}

pub(crate) fn load_kiro_cli_auth_from(path: &Path) -> Result<ImportedAccount, String> {
    let data = std::fs::read_to_string(path).map_err(|_| MISSING_KIRO.to_string())?;
    let root: serde_json::Value = serde_json::from_str(&data)
        .map_err(|_| "Kiro auth token file is not valid JSON".to_string())?;
    parse_kiro_auth_token(&root).ok_or_else(|| MISSING_KIRO.to_string())
}

pub fn load_kiro_cli_auth() -> Result<ImportedAccount, String> {
    let path = paths::kiro_auth_token_file().ok_or_else(|| MISSING_KIRO.to_string())?;
    if !path.is_file() {
        return Err(MISSING_KIRO.to_string());
    }
    load_kiro_cli_auth_from(&path)
}

/// Optional IDE usageState cache (only when the token file is missing).
pub fn read_kiro_usage_state(path: &Path) -> Option<serde_json::Value> {
    use rusqlite::{Connection, OpenFlags};

    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let mut stmt = conn
        .prepare("SELECT value FROM ItemTable WHERE key = ?1 LIMIT 1")
        .ok()?;
    let raw: String = stmt.query_row(["kiro.kiroAgent"], |row| row.get(0)).ok()?;
    let root: serde_json::Value = serde_json::from_str(&raw).ok()?;
    if let Some(state) = root.pointer("/kiro.resourceNotifications.usageState") {
        return Some(state.clone());
    }
    root.get("usageState")
        .cloned()
        .or_else(|| root.get("usage_state").cloned())
        .or(Some(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_desktop_token_file() {
        let imported = parse_kiro_auth_token(&json!({
            "accessToken": "at-1",
            "refreshToken": "rt-1",
            "expiresAt": "2026-04-06T19:29:16.090Z",
            "authMethod": "social",
            "provider": "Google",
            "profileArn": "arn:aws:codewhisperer:us-east-1:699475941385:profile/abc"
        }))
        .unwrap();
        assert_eq!(imported.credentials.access_token, "at-1");
        assert_eq!(imported.credentials.refresh_token.as_deref(), Some("rt-1"));
        assert_eq!(
            imported.credentials.account_id.as_deref(),
            Some("arn:aws:codewhisperer:us-east-1:699475941385:profile/abc")
        );
        assert!(imported.credentials.expires_at.is_some());
    }

    #[test]
    fn region_from_arn_and_field() {
        assert_eq!(
            region_from_profile_arn("arn:aws:codewhisperer:eu-central-1:1:profile/x").as_deref(),
            Some("eu-central-1")
        );
        assert_eq!(
            kiro_region_from_token(&json!({ "region": "us-west-2" })).as_deref(),
            Some("us-west-2")
        );
        assert_eq!(
            kiro_region_from_token(&json!({
                "profileArn": "arn:aws:codewhisperer:ap-southeast-1:1:profile/x"
            }))
            .as_deref(),
            Some("ap-southeast-1")
        );
    }

    #[test]
    fn missing_access_token_is_none() {
        assert!(parse_kiro_auth_token(&json!({ "refreshToken": "rt" })).is_none());
    }
}
