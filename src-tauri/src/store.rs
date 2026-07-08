use std::time::{SystemTime, UNIX_EPOCH};

use keyring::Entry;
use usage_core::account::{Account, Credentials, Provider};

const SERVICE: &str = "usagecheck";
const INDEX_KEY: &str = "index";

/// Serialize an account index to JSON. Pure function — no I/O.
pub fn serialize_index(accounts: &[Account]) -> String {
    serde_json::to_string(accounts).unwrap_or_else(|_| "[]".to_string())
}

/// Parse an account index from JSON. Pure function — no I/O.
/// Returns an empty vec if the input is missing or malformed.
pub fn parse_index(s: &str) -> Vec<Account> {
    serde_json::from_str(s).unwrap_or_default()
}

fn cred_key(id: &str) -> String {
    format!("cred/{id}")
}

/// Generates a reasonably unique id without pulling in a new dependency:
/// nanosecond timestamp since UNIX_EPOCH, formatted as hex.
fn generate_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}

/// Keychain-backed account store. Wraps `keyring::Entry` for the OS
/// credential store (macOS Keychain / Windows Credential Manager).
pub struct AccountStore;

impl AccountStore {
    pub fn new() -> Self {
        AccountStore
    }

    fn entry(key: &str) -> Option<Entry> {
        Entry::new(SERVICE, key).ok()
    }

    /// Reads the account index from the keychain. Returns an empty vec if
    /// the entry is absent or unreadable — never panics.
    pub fn list(&self) -> Vec<Account> {
        Self::entry(INDEX_KEY)
            .and_then(|e| e.get_password().ok())
            .map(|s| parse_index(&s))
            .unwrap_or_default()
    }

    fn save_index(&self, accounts: &[Account]) {
        if let Some(entry) = Self::entry(INDEX_KEY) {
            let _ = entry.set_password(&serialize_index(accounts));
        }
    }

    /// Adds a new account: generates an id, stores its credentials under
    /// `cred/<id>`, and appends it to the persisted index.
    pub fn add(&self, provider: Provider, label: String, creds: Credentials) -> Account {
        let account = Account {
            id: generate_id(),
            provider,
            label,
        };

        if let Some(entry) = Self::entry(&cred_key(&account.id)) {
            if let Ok(json) = serde_json::to_string(&creds) {
                let _ = entry.set_password(&json);
            }
        }

        let mut accounts = self.list();
        accounts.push(account.clone());
        self.save_index(&accounts);

        account
    }

    /// Removes an account: deletes its credential entry and drops it from
    /// the index. Missing entries are handled gracefully (no panic).
    pub fn remove(&self, id: &str) {
        if let Some(entry) = Self::entry(&cred_key(id)) {
            let _ = entry.delete_credential();
        }

        let accounts: Vec<Account> = self.list().into_iter().filter(|a| a.id != id).collect();
        self.save_index(&accounts);
    }

    /// Reads credentials for a given account id. Returns `None` if the
    /// entry is absent or unreadable — never panics.
    pub fn credentials(&self, id: &str) -> Option<Credentials> {
        let entry = Self::entry(&cred_key(id))?;
        let json = entry.get_password().ok()?;
        serde_json::from_str(&json).ok()
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
