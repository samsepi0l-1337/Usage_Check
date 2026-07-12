//! Read-only Cursor local auth from `state.vscdb` (never log token values).

use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use usage_core::account::Credentials;

use crate::import::ImportedAccount;
use crate::paths;

const ACCESS_TOKEN_KEY: &str = "cursorAuth/accessToken";
const REFRESH_TOKEN_KEY: &str = "cursorAuth/refreshToken";
const EMAIL_KEY: &str = "cursorAuth/cachedEmail";
const PLAN_KEY: &str = "cursorAuth/stripeMembershipType";

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

/// Cursor session: read-only from local DB, tokens kept in memory only.
#[derive(Debug, Clone)]
#[cfg(feature = "edition-pro")]
pub struct CursorSession {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub identity: String,  // JWT sub → trimmed-lowercase email fallback
}

/// Error reading Cursor session from local DB.
#[derive(Debug)]
#[cfg(feature = "edition-pro")]
pub enum CursorLocalError {
    NotFound,
    OpenFailed(String),
    TokenMissing,
    IdentityUnderivable,
}

#[cfg(feature = "edition-pro")]
impl std::fmt::Display for CursorLocalError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "Cursor state not found"),
            Self::OpenFailed(e) => write!(f, "Failed to open Cursor state: {}", e),
            Self::TokenMissing => write!(f, "Cursor access token missing"),
            Self::IdentityUnderivable => write!(f, "Could not derive Cursor identity"),
        }
    }
}

#[cfg(feature = "edition-pro")]
impl std::error::Error for CursorLocalError {}

/// Decode JWT payload (middle segment: base64url → JSON).
fn decode_jwt_payload(token: &str) -> Result<serde_json::Value, String> {
    use base64::Engine;
    
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err("Invalid JWT format".to_string());
    }

    let payload_b64 = parts[1];
    // Base64url: - → +, _ → /
    let payload_b64 = payload_b64.replace('-', "+").replace('_', "/");
    // Add padding if needed
    let padding = (4 - payload_b64.len() % 4) % 4;
    let payload_b64 = format!("{}{}", payload_b64, "=".repeat(padding));

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&payload_b64)
        .map_err(|e| format!("Base64 decode failed: {}", e))?;

    serde_json::from_slice(&decoded)
        .map_err(|e| format!("JWT payload parse failed: {}", e))
}

/// Read Cursor session from local DB (read-only, identity from JWT or email).
#[cfg(feature = "edition-pro")]
pub fn read_cursor_session(path: &Path) -> Result<CursorSession, CursorLocalError> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| CursorLocalError::OpenFailed(e.to_string()))?;

    let access_token = read_item(&conn, ACCESS_TOKEN_KEY)
        .ok_or(CursorLocalError::TokenMissing)?;
    let refresh_token = read_item(&conn, REFRESH_TOKEN_KEY);
    let cached_email = read_item(&conn, EMAIL_KEY);
    let plan = read_item(&conn, PLAN_KEY);

    // Derive identity: JWT sub → trimmed-lowercase email fallback
    let identity = if let Ok(payload) = decode_jwt_payload(&access_token) {
        if let Some(sub) = payload.get("sub").and_then(|v| v.as_str()) {
            if !sub.is_empty() {
                sub.to_string()
            } else {
                cached_email
                    .as_ref()
                    .map(|e| e.trim().to_lowercase())
                    .ok_or(CursorLocalError::IdentityUnderivable)?
            }
        } else {
            cached_email
                .as_ref()
                .map(|e| e.trim().to_lowercase())
                .ok_or(CursorLocalError::IdentityUnderivable)?
        }
    } else {
        // Fallback: no valid JWT, try email
        cached_email
            .as_ref()
            .map(|e| e.trim().to_lowercase())
            .ok_or(CursorLocalError::IdentityUnderivable)?
    };

    Ok(CursorSession {
        access_token,
        refresh_token,
        email: cached_email,
        plan,
        identity,
    })
}

/// Loads Cursor JWT + refresh token from the local VS Code DB (read-only).
pub fn load_cursor_local_auth() -> Result<ImportedAccount, String> {
    let path = paths::cursor_state_vscdb()
        .ok_or_else(|| "could not resolve home directory".to_string())?;
    if !path.is_file() {
        return Err(format!(
            "Cursor state.vscdb not found at {} — open Cursor and sign in first",
            path.display()
        ));
    }

    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("could not open Cursor state.vscdb: {e}"))?;

    let access_token = read_item(&conn, ACCESS_TOKEN_KEY).ok_or_else(|| {
        "Cursor access token not found in state.vscdb — sign in via Cursor first".to_string()
    })?;
    let refresh_token = read_item(&conn, REFRESH_TOKEN_KEY);
    let email = read_item(&conn, EMAIL_KEY);
    let plan = read_item(&conn, PLAN_KEY);

    let label = email
        .clone()
        .or_else(|| plan.clone())
        .unwrap_or_else(|| "Cursor".to_string());

    Ok(ImportedAccount {
        label,
        credentials: Credentials {
            access_token,
            refresh_token,
            account_id: plan,
            expires_at: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use rusqlite::{params, Connection};
    use tempfile::NamedTempFile;

    fn create_test_db_with_jwt(
        sub: Option<&str>,
        email: &str,
        plan: &str,
    ) -> NamedTempFile {
        let temp = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp.path()).unwrap();
        conn.execute(
            "CREATE TABLE ItemTable (id INTEGER PRIMARY KEY, key TEXT, value TEXT)",
            [],
        )
        .unwrap();

        let payload = sub
            .map(|sub| serde_json::json!({ "sub": sub }))
            .unwrap_or_else(|| serde_json::json!({}));
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("header.{payload}.signature");

        for (key, value) in [
            (ACCESS_TOKEN_KEY, token.as_str()),
            (EMAIL_KEY, email),
            (PLAN_KEY, plan),
        ] {
            conn.execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                params![key, value],
            )
            .unwrap();
        }

        temp
    }

    #[test]
    #[cfg(feature = "edition-pro")]
    fn cursor_identity_prefers_jwt_sub() {
        let db = create_test_db_with_jwt(Some("user@example.com"), "fallback@test.com", "pro");
        let session = read_cursor_session(db.path()).unwrap();
        assert_eq!(session.identity, "user@example.com");
    }

    #[test]
    #[cfg(feature = "edition-pro")]
    fn cursor_identity_falls_back_to_trimmed_lowercase_email() {
        let db = create_test_db_with_jwt(None, "  User@X.CO  ", "pro");
        let session = read_cursor_session(db.path()).unwrap();
        assert_eq!(session.identity, "user@x.co");
    }

    #[test]
    #[cfg(feature = "edition-pro")]
    fn cursor_plan_is_metadata_not_identity() {
        let db = create_test_db_with_jwt(Some("user@example.com"), "email@test.com", "pro");
        let session = read_cursor_session(db.path()).unwrap();
        assert_eq!(session.plan, Some("pro".to_string()));
        assert_eq!(session.identity, "user@example.com");
        assert_ne!(session.identity, "pro");
    }

    #[test]
    fn cursor_state_path_ends_with_vscdb() {
        if let Some(p) = paths::cursor_state_vscdb() {
            assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("state.vscdb"));
        }
    }
}
