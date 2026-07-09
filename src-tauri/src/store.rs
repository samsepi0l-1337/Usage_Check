//! File-backed account store under the OS app-data directory.
//!
//! Keychain/`keyring` silently failed for many users (no Keychain ACL prompt
//! for a headless tray app), so the account index and credentials live in:
//!   macOS:  ~/Library/Application Support/UsageCheck/
//!   Windows: %APPDATA%/UsageCheck/
//!   Linux:  ~/.local/share/UsageCheck/
//!
//! SECURITY: credentials are stored with restrictive file permissions (0600
//! on Unix). Never log/print access_token or refresh_token values.

use std::fs;
use std::path::PathBuf;

use usage_core::account::{Account, Credentials, Provider};

const APP_DIR: &str = "UsageCheck";
const INDEX_FILE: &str = "accounts.json";
const CREDS_DIR: &str = "credentials";

/// Serialize an account index to JSON. Pure function — no I/O.
pub fn serialize_index(accounts: &[Account]) -> String {
    serde_json::to_string_pretty(accounts).unwrap_or_else(|_| "[]".to_string())
}

/// Parse an account index from JSON. Pure function — no I/O.
/// Returns an empty vec if the input is missing or malformed.
pub fn parse_index(s: &str) -> Vec<Account> {
    serde_json::from_str(s).unwrap_or_default()
}

fn app_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        return crate::paths::home_dir().map(|h| {
            h.join("Library")
                .join("Application Support")
                .join(APP_DIR)
        });
    }
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(crate::paths::home_dir)
            .map(|h| h.join(APP_DIR));
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        return crate::paths::home_dir().map(|h| h.join(".local").join("share").join(APP_DIR));
    }
}

fn index_path() -> Option<PathBuf> {
    app_data_dir().map(|d| d.join(INDEX_FILE))
}

fn cred_path(id: &str) -> Option<PathBuf> {
    app_data_dir().map(|d| d.join(CREDS_DIR).join(format!("{id}.json")))
}

fn ensure_dirs() -> Option<PathBuf> {
    let dir = app_data_dir()?;
    fs::create_dir_all(dir.join(CREDS_DIR)).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
        let _ = fs::set_permissions(dir.join(CREDS_DIR), fs::Permissions::from_mode(0o700));
    }
    Some(dir)
}

fn write_secret_file(path: &PathBuf, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create dir: {e}"))?;
    }
    fs::write(path, contents).map_err(|e| format!("write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// File-backed account store.
pub struct AccountStore;

impl AccountStore {
    pub fn new() -> Self {
        let _ = ensure_dirs();
        AccountStore
    }

    /// Reads the account index. Returns an empty vec if absent/unreadable.
    pub fn list(&self) -> Vec<Account> {
        let Some(path) = index_path() else {
            return Vec::new();
        };
        fs::read_to_string(path)
            .ok()
            .map(|s| parse_index(&s))
            .unwrap_or_default()
    }

    fn save_index(&self, accounts: &[Account]) -> Result<(), String> {
        ensure_dirs().ok_or_else(|| "could not resolve app data directory".to_string())?;
        let path = index_path().ok_or_else(|| "could not resolve index path".to_string())?;
        write_secret_file(&path, &serialize_index(accounts))
    }

    /// Adds a new account and persists credentials. Returns an error if the
    /// app-data directory cannot be written (so the UI can surface it).
    pub fn add(
        &self,
        provider: Provider,
        label: String,
        creds: Credentials,
    ) -> Result<Account, String> {
        ensure_dirs().ok_or_else(|| "could not resolve app data directory".to_string())?;

        let account = Account {
            id: uuid::Uuid::new_v4().to_string(),
            provider,
            label,
        };

        let path = cred_path(&account.id).ok_or_else(|| "could not resolve cred path".to_string())?;
        let json = serde_json::to_string_pretty(&creds)
            .map_err(|e| format!("serialize credentials: {e}"))?;
        write_secret_file(&path, &json)?;

        let mut accounts = self.list();
        accounts.push(account.clone());
        self.save_index(&accounts)?;

        Ok(account)
    }

    /// Removes an account and its credential file.
    pub fn remove(&self, id: &str) {
        if let Some(path) = cred_path(id) {
            let _ = fs::remove_file(path);
        }
        let accounts: Vec<Account> = self.list().into_iter().filter(|a| a.id != id).collect();
        let _ = self.save_index(&accounts);
    }

    /// Reads credentials for a given account id.
    pub fn credentials(&self, id: &str) -> Option<Credentials> {
        let path = cred_path(id)?;
        let json = fs::read_to_string(path).ok()?;
        serde_json::from_str(&json).ok()
    }

    /// Overwrites stored credentials after a token refresh.
    pub fn update_credentials(&self, id: &str, creds: &Credentials) {
        let Some(path) = cred_path(id) else {
            return;
        };
        if let Ok(json) = serde_json::to_string_pretty(creds) {
            let _ = write_secret_file(&path, &json);
        }
    }
}

impl Default for AccountStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use usage_core::account::{Account, Provider};

    #[test]
    fn index_roundtrips() {
        let accts = vec![Account {
            id: "1".into(),
            provider: Provider::Codex,
            label: "work".into(),
        }];
        let s = serialize_index(&accts);
        assert_eq!(parse_index(&s), accts);
    }

    #[test]
    fn parse_index_empty_on_garbage() {
        assert_eq!(parse_index("not json"), Vec::<Account>::new());
    }

    #[test]
    fn parse_index_empty_on_empty_string() {
        assert_eq!(parse_index(""), Vec::<Account>::new());
    }
}
