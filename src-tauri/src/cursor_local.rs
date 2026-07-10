//! Read-only Cursor local auth from `state.vscdb` (never log token values).

use rusqlite::{Connection, OpenFlags};
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

    #[test]
    fn cursor_state_path_ends_with_vscdb() {
        if let Some(p) = paths::cursor_state_vscdb() {
            assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("state.vscdb"));
        }
    }
}
