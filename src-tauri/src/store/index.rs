use std::fs;
use std::sync::{Mutex, OnceLock};

use usage_core::account::Account;

use super::{reject_symlink, write_private_file, AccountStore};

/// Process-global lock serializing every index read-modify-write. Each
/// `AccountStore` is stateless and recreated per call, and mutators run from
/// concurrently-spawned async tasks, so without this two interleaved mutations
/// would both read the same base list and the last writer would silently drop
/// the other's change (lost update / account resurrection).
pub(super) fn index_mutation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Serialize an account index to JSON. Pure function — no I/O.
pub fn serialize_index(accounts: &[Account]) -> String {
    serde_json::to_string_pretty(accounts).unwrap_or_else(|_| "[]".to_string())
}

/// Parse an account index from JSON. Pure function — no I/O.
/// Returns an empty vec if the input is missing or malformed.
pub fn parse_index(s: &str) -> Vec<Account> {
    let Ok(values) = serde_json::from_str::<Vec<serde_json::Value>>(s) else {
        return Vec::new();
    };
    values
        .into_iter()
        .filter_map(|value| serde_json::from_value::<Account>(value).ok())
        .collect()
}

/// Serialize an account index while preserving entries this build cannot
/// deserialize, such as accounts from another edition or a future provider.
pub fn serialize_index_preserving(accounts: &[Account], unknown: &[serde_json::Value]) -> String {
    let mut arr: Vec<serde_json::Value> = accounts
        .iter()
        .map(|a| serde_json::to_value(a).unwrap_or(serde_json::Value::Null))
        .filter(|v| !v.is_null())
        .collect();
    arr.extend(unknown.iter().cloned());
    serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".to_string())
}

impl AccountStore {
    /// Reads the account index. Returns an empty vec if absent/unreadable.
    pub fn list(&self) -> Vec<Account> {
        fs::read_to_string(self.index_path())
            .ok()
            .map(|s| super::parse_index(&s))
            .unwrap_or_default()
    }

    pub fn account(&self, id: &str) -> Option<Account> {
        self.list().into_iter().find(|account| account.id == id)
    }

    pub(super) fn save_index(&self, accounts: &[Account]) -> Result<(), String> {
        self.ensure_root()?;
        let path = self.index_path();
        reject_symlink(&path, "account index")?;
        write_private_file(&path, &super::serialize_index(accounts))
    }

    pub(super) fn save_index_preserving(
        &self,
        accounts: &[Account],
        unknown: &[serde_json::Value],
    ) -> Result<(), String> {
        self.ensure_root()?;
        let path = self.index_path();
        reject_symlink(&path, "account index")?;
        write_private_file(
            &path,
            &super::serialize_index_preserving(accounts, unknown),
        )
    }

    /// Reads the index, partitioning entries into deserializable `Account`s and raw JSON values that this
    /// build cannot deserialize (e.g. an other-edition/future provider). Preserves the unknown entries so a
    /// read-modify-write cycle never destroys them. Genuine corruption (not a JSON array) still errors.
    pub(super) fn read_index_partitioned(
        &self,
    ) -> Result<(Vec<Account>, Vec<serde_json::Value>), String> {
        let path = self.index_path();
        match fs::read_to_string(&path) {
            Ok(contents) => {
                let values: Vec<serde_json::Value> = serde_json::from_str(&contents)
                    .map_err(|e| format!("account index is corrupt ({}): {e}", path.display()))?;
                let mut known = Vec::new();
                let mut unknown = Vec::new();
                for v in values {
                    match serde_json::from_value::<Account>(v.clone()) {
                        Ok(acc) => known.push(acc),
                        Err(_) => unknown.push(v),
                    }
                }
                Ok((known, unknown))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Vec::new(), Vec::new())),
            Err(e) => Err(format!("read {}: {e}", path.display())),
        }
    }
}
