//! Read-only Windsurf local auth from `state.vscdb` (never log token values).

use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::path::Path;
use usage_core::account::Credentials;

use crate::import::ImportedAccount;
use crate::paths;

const AUTH_STATUS_KEY: &str = "windsurfAuthStatus";

fn read_item(conn: &Connection, key: &str) -> Option<String> {
    let mut stmt = conn
        .prepare("SELECT value FROM ItemTable WHERE key = ?1 LIMIT 1")
        .ok()?;
    let mut rows = stmt.query([key]).ok()?;
    let row = rows.next().ok()??;
    let value: String = row.get(0).ok()?;
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
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

fn parse_auth_status(raw: &str) -> Option<serde_json::Value> {
    serde_json::from_str(raw).ok()
}

fn key_fingerprint(api_key: &str) -> String {
    let digest = Sha256::digest(api_key.as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("key:{hex}")
}

fn root_str(root: &serde_json::Value, keys: &[&str]) -> Option<String> {
    let map = root.as_object()?;
    for key in keys {
        if let Some(found) = map.get(*key).and_then(nonempty_str) {
            return Some(found);
        }
    }
    None
}

fn identity_from_status(root: &serde_json::Value, api_key: &str) -> String {
    first_named_string(root, &["email"])
        .map(|email| email.to_ascii_lowercase())
        .or_else(|| root_str(root, &["userId", "user_id"]))
        .or_else(|| root_str(root, &["name"]).map(|name| name.to_ascii_lowercase()))
        .unwrap_or_else(|| key_fingerprint(api_key))
}

/// Windsurf session: read-only from local DB, key kept in memory only.
#[derive(Debug, Clone)]
pub struct WindsurfSession {
    pub api_key: String,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub identity: String,
}

#[derive(Debug)]
pub enum WindsurfLocalError {
    OpenFailed(String),
    TokenMissing,
    IdentityUnderivable,
}

impl std::fmt::Display for WindsurfLocalError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::OpenFailed(e) => write!(f, "Failed to open Windsurf state: {e}"),
            Self::TokenMissing => write!(f, "Windsurf API key missing"),
            Self::IdentityUnderivable => write!(f, "Could not derive Windsurf identity"),
        }
    }
}

impl std::error::Error for WindsurfLocalError {}

/// Read Windsurf session from local DB (read-only).
pub fn read_windsurf_session(path: &Path) -> Result<WindsurfSession, WindsurfLocalError> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| WindsurfLocalError::OpenFailed(e.to_string()))?;
    let raw = read_item(&conn, AUTH_STATUS_KEY).ok_or(WindsurfLocalError::TokenMissing)?;
    let root = parse_auth_status(&raw).ok_or(WindsurfLocalError::TokenMissing)?;
    let api_key = first_named_string(&root, &["apiKey", "api_key"])
        .ok_or(WindsurfLocalError::TokenMissing)?;
    if api_key.is_empty() {
        return Err(WindsurfLocalError::TokenMissing);
    }
    let email = first_named_string(&root, &["email"]);
    let plan = first_named_string(&root, &["planName", "plan_name", "plan"]);
    let identity = identity_from_status(&root, &api_key);
    if identity.is_empty() {
        return Err(WindsurfLocalError::IdentityUnderivable);
    }
    Ok(WindsurfSession {
        api_key,
        email,
        plan,
        identity,
    })
}

/// Loads Windsurf `apiKey` from the local VS Code DB (read-only).
pub fn load_windsurf_local_auth() -> Result<ImportedAccount, String> {
    let path = paths::windsurf_state_vscdb()
        .ok_or_else(|| "could not resolve home directory".to_string())?;
    if !path.is_file() {
        return Err(format!(
            "Windsurf state.vscdb not found at {} — open Windsurf and sign in first",
            path.display()
        ));
    }
    let session = read_windsurf_session(&path).map_err(|e| e.to_string())?;
    let label = session
        .email
        .clone()
        .or_else(|| session.plan.clone())
        .unwrap_or_else(|| "Windsurf".to_string());
    Ok(ImportedAccount {
        label,
        credentials: Credentials {
            access_token: session.api_key,
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

    fn create_test_db(status: &serde_json::Value) -> NamedTempFile {
        let temp = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp.path()).unwrap();
        conn.execute(
            "CREATE TABLE ItemTable (id INTEGER PRIMARY KEY, key TEXT, value TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
            params![AUTH_STATUS_KEY, serde_json::to_string(status).unwrap()],
        )
        .unwrap();
        temp
    }

    #[test]
    fn reads_api_key_and_email_identity() {
        let db = create_test_db(&serde_json::json!({
            "email": "  User@X.CO  ",
            "apiKey": "ws-key",
            "planName": "Pro"
        }));
        let session = read_windsurf_session(db.path()).unwrap();
        assert_eq!(session.api_key, "ws-key");
        assert_eq!(session.identity, "user@x.co");
        assert_eq!(session.plan.as_deref(), Some("Pro"));
    }

    #[test]
    fn accepts_nested_api_key() {
        let db = create_test_db(&serde_json::json!({
            "userId": "user-1",
            "status": { "api_key": "nested-key" }
        }));
        let session = read_windsurf_session(db.path()).unwrap();
        assert_eq!(session.api_key, "nested-key");
        assert_eq!(session.identity, "user-1");
    }

    #[test]
    fn nested_generic_id_is_not_identity() {
        let db = create_test_db(&serde_json::json!({
            "apiKey": "ws-key",
            "session": { "id": "rotating-session" }
        }));
        let session = read_windsurf_session(db.path()).unwrap();
        assert!(session.identity.starts_with("key:"));
        assert_ne!(session.identity, "rotating-session");
    }

    #[test]
    fn fingerprints_key_when_no_identity_fields() {
        let db = create_test_db(&serde_json::json!({ "apiKey": "only-key" }));
        let session = read_windsurf_session(db.path()).unwrap();
        assert!(session.identity.starts_with("key:"));
        assert_ne!(session.identity, "only-key");
    }

    #[test]
    fn missing_key_is_token_missing() {
        let db = create_test_db(&serde_json::json!({ "email": "a@b.co" }));
        let err = read_windsurf_session(db.path()).unwrap_err();
        assert!(matches!(err, WindsurfLocalError::TokenMissing));
    }
}
