//! Read-only Trae (intl) local auth from `state.vscdb` (never log token values).
//! Encrypted CN `tc` blobs fail closed — we do not decrypt them.

use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::path::Path;
use usage_core::account::Credentials;

use crate::import::ImportedAccount;
use crate::paths;

const PREFERRED_KEY_HINTS: &[&str] = &[
    "cloud-ide-jwt",
    "cloudidejwt",
    "icubeauthinfo",
    "icube.cloudide",
];

fn read_all_items(conn: &Connection) -> Result<Vec<(String, String)>, rusqlite::Error> {
    let mut stmt = conn.prepare("SELECT key, value FROM ItemTable")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut items = Vec::new();
    for row in rows {
        let (key, value) = row?;
        if !value.is_empty() {
            items.push((key, value));
        }
    }
    Ok(items)
}

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

fn looks_like_jwt(raw: &str) -> bool {
    let t = raw.trim();
    let mut parts = t.split('.');
    parts.next().is_some_and(|h| h.starts_with("eyJ"))
        && parts.next().is_some_and(|p| !p.is_empty())
        && parts.next().is_some()
        && parts.next().is_none()
}

/// CN Trae stores AES blobs that start with `tc` (or non-JSON/non-JWT).
fn looks_encrypted(raw: &str) -> bool {
    let t = raw.trim();
    if t.is_empty() {
        return false;
    }
    if t.starts_with('{') || t.starts_with('"') || t.starts_with('[') {
        return false;
    }
    if looks_like_jwt(t) {
        return false;
    }
    t.to_ascii_lowercase().starts_with("tc")
        || (t.len() > 40
            && t.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_')))
}

fn key_is_preferred(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    PREFERRED_KEY_HINTS.iter().any(|hint| lower.contains(hint))
}

fn jwt_from_value(raw: &str) -> Option<String> {
    let t = raw.trim();
    if looks_like_jwt(t) {
        return Some(t.to_string());
    }
    let root: serde_json::Value = serde_json::from_str(t).ok()?;
    first_named_string(
        &root,
        &[
            "token",
            "access_token",
            "accessToken",
            "jwt",
            "Cloud-IDE-JWT",
            "cloudIdeJwt",
            "cloud_ide_jwt",
        ],
    )
    .filter(|s| looks_like_jwt(s))
}

fn identity_from_auth(raw: &str, jwt: &str) -> String {
    if let Ok(root) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(email) = first_named_string(&root, &["email"]) {
            return email.to_ascii_lowercase();
        }
        if let Some(id) =
            first_named_string(&root, &["userId", "user_id", "uid", "id"]).filter(|s| s != jwt)
        {
            return id;
        }
    }
    crate::import::email_from_jwt(jwt)
        .map(|email| email.to_ascii_lowercase())
        .or_else(|| jwt_subject(jwt))
        .unwrap_or_else(|| key_fingerprint(jwt))
}

fn jwt_subject(jwt: &str) -> Option<String> {
    let payload_b64 = jwt.split('.').nth(1)?;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let root: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    first_named_string(&root, &["email"])
        .map(|s| s.to_ascii_lowercase())
        .or_else(|| {
            root.get("data")
                .and_then(|d| first_named_string(d, &["id", "user_id", "userId"]))
        })
        .or_else(|| first_named_string(&root, &["sub", "id", "user_id", "userId"]))
}

fn key_fingerprint(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("key:{hex}")
}

fn email_from_auth(raw: &str, jwt: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|root| first_named_string(&root, &["email"]))
        .or_else(|| crate::import::email_from_jwt(jwt))
}

/// Trae session: read-only from local DB, JWT kept in memory only.
#[derive(Debug, Clone)]
pub struct TraeSession {
    pub jwt: String,
    pub email: Option<String>,
    pub identity: String,
}

#[derive(Debug)]
pub enum TraeLocalError {
    OpenFailed(String),
    TokenMissing,
    Encrypted,
    IdentityUnderivable,
}

impl std::fmt::Display for TraeLocalError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::OpenFailed(e) => write!(f, "Failed to open Trae state: {e}"),
            Self::TokenMissing => {
                write!(f, "Trae JWT missing — open Trae and sign in first")
            }
            Self::Encrypted => write!(
                f,
                "Trae auth is encrypted (CN tc blob) — UsageCheck does not decrypt it. Sign in to international Trae"
            ),
            Self::IdentityUnderivable => write!(f, "Could not derive Trae identity"),
        }
    }
}

impl std::error::Error for TraeLocalError {}

/// Read Trae session from local DB (read-only). Encrypted blobs fail closed.
pub fn read_trae_session(path: &Path) -> Result<TraeSession, TraeLocalError> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| TraeLocalError::OpenFailed(e.to_string()))?;
    let items = read_all_items(&conn).map_err(|e| TraeLocalError::OpenFailed(e.to_string()))?;

    let mut saw_encrypted = false;
    let mut chosen: Option<(String, String)> = None;
    for (key, value) in &items {
        let preferred = key_is_preferred(key);
        if looks_encrypted(value) {
            if preferred {
                saw_encrypted = true;
            }
            continue;
        }
        if let Some(jwt) = jwt_from_value(value) {
            if preferred {
                chosen = Some((value.clone(), jwt));
                break;
            }
            if chosen.is_none() {
                chosen = Some((value.clone(), jwt));
            }
        }
    }

    let Some((raw, jwt)) = chosen else {
        return Err(if saw_encrypted {
            TraeLocalError::Encrypted
        } else {
            TraeLocalError::TokenMissing
        });
    };

    let identity = identity_from_auth(&raw, &jwt);
    if identity.is_empty() {
        return Err(TraeLocalError::IdentityUnderivable);
    }
    Ok(TraeSession {
        email: email_from_auth(&raw, &jwt),
        jwt,
        identity,
    })
}

/// Loads Trae Cloud-IDE-JWT from the local VS Code DB (read-only).
pub fn load_trae_local_auth() -> Result<ImportedAccount, String> {
    let path =
        paths::trae_state_vscdb().ok_or_else(|| "could not resolve home directory".to_string())?;
    if !path.is_file() {
        return Err(format!(
            "Trae state.vscdb not found at {} — open Trae and sign in first",
            path.display()
        ));
    }
    let session = read_trae_session(&path).map_err(|e| e.to_string())?;
    let label = session.email.clone().unwrap_or_else(|| "Trae".to_string());
    Ok(ImportedAccount {
        label,
        credentials: Credentials {
            access_token: session.jwt,
            refresh_token: None,
            account_id: Some(session.identity),
            expires_at: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};
    use tempfile::NamedTempFile;

    fn create_test_db(rows: &[(&str, &str)]) -> NamedTempFile {
        let temp = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp.path()).unwrap();
        conn.execute(
            "CREATE TABLE ItemTable (id INTEGER PRIMARY KEY, key TEXT, value TEXT)",
            [],
        )
        .unwrap();
        for (key, value) in rows {
            conn.execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                params![key, value],
            )
            .unwrap();
        }
        temp
    }

    fn sample_jwt(sub: &str) -> String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"email":"{sub}"}}"#).as_bytes());
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn reads_icube_auth_info_token() {
        let jwt = sample_jwt("dev@trae.ai");
        let db = create_test_db(&[(
            "iCubeAuthInfo://icube.cloudide",
            &format!(r#"{{"token":"{jwt}","userId":"u-1","account":{{"email":"dev@trae.ai"}}}}"#),
        )]);
        let session = read_trae_session(db.path()).unwrap();
        assert_eq!(session.jwt, jwt);
        assert_eq!(session.identity, "dev@trae.ai");
    }

    #[test]
    fn accepts_cloud_ide_jwt_key() {
        let jwt = sample_jwt("a@b.co");
        let db = create_test_db(&[("Cloud-IDE-JWT", &jwt)]);
        let session = read_trae_session(db.path()).unwrap();
        assert_eq!(session.jwt, jwt);
        assert_eq!(session.identity, "a@b.co");
    }

    #[test]
    fn encrypted_tc_blob_fails_closed() {
        let db = create_test_db(&[(
            "iCubeAuthInfo://icube.cloudide",
            "tcABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+/====",
        )]);
        let err = read_trae_session(db.path()).unwrap_err();
        assert!(matches!(err, TraeLocalError::Encrypted));
    }

    #[test]
    fn missing_token_is_token_missing() {
        let db = create_test_db(&[("unrelated", r#"{"foo":"bar"}"#)]);
        let err = read_trae_session(db.path()).unwrap_err();
        assert!(matches!(err, TraeLocalError::TokenMissing));
    }
}
