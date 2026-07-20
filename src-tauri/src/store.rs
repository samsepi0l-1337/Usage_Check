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
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use usage_core::account::{Account, AuthSource, Credentials, Provider};

/// Process-global lock serializing every index read-modify-write. Each
/// `AccountStore` is stateless and recreated per call, and mutators run from
/// concurrently-spawned async tasks, so without this two interleaved mutations
/// would both read the same base list and the last writer would silently drop
/// the other's change (lost update / account resurrection).
fn index_mutation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

const LEGACY_INDEX_FILE: &str = "accounts.json";
const INDEX_FILE: &str = "accounts-v2.json";
const SCHEMA_MARKER: &str = "schema-v2";
const CREDS_DIR: &str = "credentials";

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

fn set_private_dir_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
}

fn write_private_file(path: &Path, contents: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("no parent directory for {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|e| format!("create dir: {e}"))?;
    set_private_dir_permissions(parent);

    // Atomic write: stage into a private temp sibling on the SAME directory
    // (hence same filesystem), then rename over the target. A crash mid-write
    // can then never truncate/half-write the real file — the rename either
    // fully lands the new bytes or leaves the previous file intact.
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let tmp = parent.join(format!(".{file_name}.tmp-{}", uuid::Uuid::new_v4()));

    fs::write(&tmp, contents).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("write {}: {e}", tmp.display())
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("commit {}: {e}", path.display())
    })?;
    Ok(())
}

fn reject_symlink(path: &Path, description: &str) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
            "{description} must not be a symlink: {}",
            path.display()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("inspect {}: {error}", path.display())),
    }
}

/// App-owned authentication sources that require a credential record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SecretSource {
    BrowserOAuth,
    #[cfg(feature = "edition-pro")]
    XaiManagement {
        team_id: String,
    },
}

/// File-backed schema-v2 account store rooted in UsageCheck's app-data folder.
#[derive(Clone, Debug)]
pub struct AccountStore {
    root: PathBuf,
}

impl AccountStore {
    pub fn new() -> Self {
        let store = Self {
            root: crate::paths::usagecheck_app_data_dir().unwrap_or_default(),
        };
        let _ = store.initialize_v2();
        store
    }

    #[cfg(test)]
    pub fn new_at(root: PathBuf) -> Self {
        Self { root }
    }

    fn ensure_root(&self) -> Result<(), String> {
        if self.root.as_os_str().is_empty() {
            return Err("could not resolve app data directory".to_string());
        }
        match fs::symlink_metadata(&self.root) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "store root must not be a symlink: {}",
                    self.root.display()
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(format!(
                    "store root is not a directory: {}",
                    self.root.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(&self.root)
                    .map_err(|e| format!("create {}: {e}", self.root.display()))?;
            }
            Err(error) => return Err(format!("inspect {}: {error}", self.root.display())),
        }
        reject_symlink(&self.root, "store root")?;
        set_private_dir_permissions(&self.root);
        Ok(())
    }

    fn index_path(&self) -> PathBuf {
        self.root.join(INDEX_FILE)
    }

    fn credential_path(&self, credential_id: &str) -> Result<PathBuf, String> {
        uuid::Uuid::parse_str(credential_id).map_err(|_| "invalid credential id".to_string())?;
        Ok(self
            .root
            .join(CREDS_DIR)
            .join(format!("{credential_id}.json")))
    }

    fn credential_path_for_write(&self, credential_id: &str) -> Result<PathBuf, String> {
        let directory = self.root.join(CREDS_DIR);
        reject_symlink(&directory, "credential directory")?;
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if !metadata.is_dir() => {
                return Err(format!(
                    "credential directory is not a directory: {}",
                    directory.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(&directory)
                    .map_err(|e| format!("create {}: {e}", directory.display()))?;
                set_private_dir_permissions(&directory);
            }
            Err(error) => return Err(format!("inspect {}: {error}", directory.display())),
        }
        reject_symlink(&directory, "credential directory")?;
        let path = self.credential_path(credential_id)?;
        reject_symlink(&path, "credential file")?;
        Ok(path)
    }

    fn credential_path_for_read(&self, credential_id: &str) -> Result<PathBuf, String> {
        let directory = self.root.join(CREDS_DIR);
        reject_symlink(&directory, "credential directory")?;
        let metadata = fs::symlink_metadata(&directory)
            .map_err(|e| format!("inspect {}: {e}", directory.display()))?;
        if !metadata.is_dir() {
            return Err(format!(
                "credential directory is not a directory: {}",
                directory.display()
            ));
        }
        let path = self.credential_path(credential_id)?;
        reject_symlink(&path, "credential file")?;
        Ok(path)
    }

    fn remove_legacy_index(&self) -> Result<(), String> {
        let path = self.root.join(LEGACY_INDEX_FILE);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("inspect {}: {error}", path.display())),
        };
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            return Err(format!("legacy index is not a file: {}", path.display()));
        }
        fs::remove_file(&path).map_err(|e| format!("remove {}: {e}", path.display()))
    }

    fn remove_legacy_credentials(&self) -> Result<(), String> {
        let path = self.root.join(CREDS_DIR);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("inspect {}: {error}", path.display())),
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            fs::remove_file(&path).map_err(|e| format!("remove {}: {e}", path.display()))
        } else {
            fs::remove_dir_all(&path).map_err(|e| format!("remove {}: {e}", path.display()))
        }
    }

    /// Initializes schema v2. A missing marker resets only the two fixed,
    /// app-owned legacy paths beneath this store's injected root.
    pub fn initialize_v2(&self) -> Result<(), String> {
        self.ensure_root()?;
        let marker = self.root.join(SCHEMA_MARKER);
        let marker_exists = match fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "schema marker must not be a symlink: {}",
                    marker.display()
                ));
            }
            Ok(metadata) if !metadata.is_file() => {
                return Err(format!("schema marker is not a file: {}", marker.display()));
            }
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(format!("inspect {}: {error}", marker.display())),
        };
        reject_symlink(&self.index_path(), "account index")?;
        if marker_exists {
            if !self.index_path().exists() {
                self.save_index(&[])?;
            }
            return Ok(());
        }

        self.remove_legacy_index()?;
        self.remove_legacy_credentials()?;
        self.save_index(&[])?;
        reject_symlink(&marker, "schema marker")?;
        write_private_file(&marker, "2\n")
    }

    /// Reads the account index. Returns an empty vec if absent/unreadable.
    pub fn list(&self) -> Vec<Account> {
        fs::read_to_string(self.index_path())
            .ok()
            .map(|s| parse_index(&s))
            .unwrap_or_default()
    }

    pub fn account(&self, id: &str) -> Option<Account> {
        self.list().into_iter().find(|account| account.id == id)
    }

    fn save_index(&self, accounts: &[Account]) -> Result<(), String> {
        self.ensure_root()?;
        let path = self.index_path();
        reject_symlink(&path, "account index")?;
        write_private_file(&path, &serialize_index(accounts))
    }

    fn save_index_preserving(
        &self,
        accounts: &[Account],
        unknown: &[serde_json::Value],
    ) -> Result<(), String> {
        self.ensure_root()?;
        let path = self.index_path();
        reject_symlink(&path, "account index")?;
        write_private_file(&path, &serialize_index_preserving(accounts, unknown))
    }

    /// Reads the index, partitioning entries into deserializable `Account`s and raw JSON values that this
    /// build cannot deserialize (e.g. an other-edition/future provider). Preserves the unknown entries so a
    /// read-modify-write cycle never destroys them. Genuine corruption (not a JSON array) still errors.
    fn read_index_partitioned(&self) -> Result<(Vec<Account>, Vec<serde_json::Value>), String> {
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

    fn validate_reference_source(provider: Provider, source: &AuthSource) -> Result<(), String> {
        let valid = match (provider, source) {
            (Provider::Codex | Provider::Claude, AuthSource::CliProfile { .. }) => true,
            #[cfg(feature = "edition-pro")]
            (Provider::Cursor, AuthSource::CursorDatabase { .. }) => true,
            #[cfg(feature = "edition-pro")]
            (Provider::Higgsfield, AuthSource::HiggsfieldCli { .. }) => true,
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err("authentication source is not a reference for this provider".to_string())
        }
    }

    fn validate_secret_source(provider: Provider, source: &SecretSource) -> Result<(), String> {
        let valid = match source {
            SecretSource::BrowserOAuth => {
                matches!(provider, Provider::Codex | Provider::Claude | Provider::Agy)
            }
            #[cfg(feature = "edition-pro")]
            SecretSource::XaiManagement { .. } => provider == Provider::Grok,
        };
        if valid {
            Ok(())
        } else {
            Err("authentication source is not an app-owned secret for this provider".to_string())
        }
    }

    fn duplicate_source(accounts: &[Account], source: &AuthSource) -> Option<&'static str> {
        accounts
            .iter()
            .find_map(|account| match (&account.auth_source, source) {
                (
                    AuthSource::CliProfile {
                        profile_root: existing,
                        ..
                    },
                    AuthSource::CliProfile {
                        profile_root: candidate,
                        ..
                    },
                ) if existing == candidate => Some("profile root already registered"),
                (
                    AuthSource::CursorDatabase {
                        expected_identity: existing,
                        ..
                    },
                    AuthSource::CursorDatabase {
                        expected_identity: candidate,
                        ..
                    },
                ) if existing == candidate => Some("Cursor identity already registered"),
                (
                    AuthSource::XaiManagement {
                        team_id: existing, ..
                    },
                    AuthSource::XaiManagement {
                        team_id: candidate, ..
                    },
                ) if existing == candidate => Some("xAI team already registered"),
                (
                    AuthSource::HiggsfieldCli {
                        expected_identity: existing,
                    },
                    AuthSource::HiggsfieldCli {
                        expected_identity: candidate,
                    },
                ) if existing == candidate => Some("Higgsfield CLI account already registered"),
                _ => None,
            })
    }

    /// Stable identity keys for a BrowserOAuth account: collect both the provider
    /// account id and email-shaped label when present. An empty set means the
    /// identity is unidentifiable and dedup is skipped rather than guessed.
    /// `AuthSource::BrowserOAuth` carries only a random `credential_id`, so
    /// re-import/re-login of the SAME identity mints a fresh id — dedup must key
    /// off the credentials/label, not the source.
    fn oauth_identity_keys(label: &str, creds: &Credentials) -> Vec<String> {
        let mut keys = Vec::new();
        if let Some(id) = creds
            .account_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            keys.push(format!("id:{}", id.to_ascii_lowercase()));
        }
        let label = label.trim();
        if label.contains('@') {
            keys.push(format!("email:{}", label.to_ascii_lowercase()));
        }
        keys
    }

    /// Reject re-adding a BrowserOAuth account whose identity already exists for
    /// the same provider (else re-login/re-import silently creates a duplicate
    /// polled and shown twice). Compares against each existing OAuth account's
    /// stored credentials; unidentifiable pairs fall through (no false reject).
    fn oauth_duplicate(
        &self,
        accounts: &[Account],
        provider: Provider,
        label: &str,
        creds: &Credentials,
    ) -> Option<String> {
        let new_keys = Self::oauth_identity_keys(label, creds);
        if new_keys.is_empty() {
            return None;
        }
        for existing in accounts {
            if existing.provider != provider {
                continue;
            }
            let AuthSource::BrowserOAuth { credential_id } = &existing.auth_source else {
                continue;
            };
            let Some(existing_creds) = self.credentials(credential_id) else {
                continue;
            };
            let existing_keys = Self::oauth_identity_keys(&existing.label, &existing_creds);
            if new_keys.iter().any(|key| existing_keys.contains(key)) {
                return Some("account already registered".to_string());
            }
        }
        None
    }

    pub fn add_reference(
        &self,
        provider: Provider,
        label: String,
        auth_source: AuthSource,
    ) -> Result<Account, String> {
        self.initialize_v2()?;
        Self::validate_reference_source(provider, &auth_source)?;
        let _guard = index_mutation_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (mut accounts, unknown) = self.read_index_partitioned()?;
        if let Some(reason) = Self::duplicate_source(&accounts, &auth_source) {
            return Err(reason.to_string());
        }
        let account = Account {
            id: uuid::Uuid::new_v4().to_string(),
            provider,
            label,
            auth_source,
        };
        accounts.push(account.clone());
        self.save_index_preserving(&accounts, &unknown)?;
        Ok(account)
    }

    /// Best-effort startup migration: re-anchor existing Claude CliProfile accounts from the shared
    /// organizationUuid (or email) to the UNIQUE accountUuid, and refresh the label to the account email,
    /// by reading each account's `<profile_root>/.claude.json`. Non-destructive: an account whose
    /// `.claude.json` is unreadable or has no accountUuid is left exactly as-is. Preserves unknown index
    /// entries and holds the index mutation lock. Returns the number of accounts changed.
    pub fn migrate_claude_identity_anchors(&self) -> Result<usize, String> {
        self.initialize_v2()?;
        let _guard = index_mutation_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (mut accounts, unknown) = self.read_index_partitioned()?;
        let mut changed = 0usize;

        for account in &mut accounts {
            if account.provider != Provider::Claude {
                continue;
            }
            let Some((uuid, email)) = (match &account.auth_source {
                AuthSource::CliProfile { profile_root, .. } => {
                    let (email, account_uuid, _org) =
                        crate::import::claude_oauth_identity_set_in(profile_root);
                    account_uuid.filter(|s| !s.is_empty()).map(|uuid| {
                        (
                            uuid,
                            email
                                .map(|mail| mail.trim().to_lowercase())
                                .filter(|mail| !mail.is_empty()),
                        )
                    })
                }
                _ => None,
            }) else {
                continue;
            };

            let mut touched = false;
            if let AuthSource::CliProfile {
                expected_identity, ..
            } = &mut account.auth_source
            {
                if *expected_identity != uuid {
                    *expected_identity = uuid;
                    touched = true;
                }
            }
            if let Some(email) = email {
                if account.label != email {
                    account.label = email;
                    touched = true;
                }
            }
            if touched {
                changed += 1;
            }
        }

        if changed > 0 {
            self.save_index_preserving(&accounts, &unknown)?;
        }
        Ok(changed)
    }

    pub fn add_secret(
        &self,
        provider: Provider,
        label: String,
        source: SecretSource,
        credentials: Credentials,
    ) -> Result<Account, String> {
        self.add_secret_with_ids(
            provider,
            label,
            source,
            credentials,
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
        )
    }

    fn add_secret_with_ids(
        &self,
        provider: Provider,
        label: String,
        source: SecretSource,
        credentials: Credentials,
        account_id: String,
        credential_id: String,
    ) -> Result<Account, String> {
        self.initialize_v2()?;
        Self::validate_secret_source(provider, &source)?;
        let auth_source = match source {
            SecretSource::BrowserOAuth => AuthSource::BrowserOAuth {
                credential_id: credential_id.clone(),
            },
            #[cfg(feature = "edition-pro")]
            SecretSource::XaiManagement { team_id } => AuthSource::XaiManagement {
                credential_id: credential_id.clone(),
                team_id,
            },
        };
        let _guard = index_mutation_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (mut accounts, unknown) = self.read_index_partitioned()?;
        if let Some(reason) = Self::duplicate_source(&accounts, &auth_source) {
            return Err(reason.to_string());
        }
        if matches!(auth_source, AuthSource::BrowserOAuth { .. }) {
            if let Some(reason) = self.oauth_duplicate(&accounts, provider, &label, &credentials) {
                return Err(reason.to_string());
            }
        }
        let account = Account {
            id: account_id,
            provider,
            label,
            auth_source,
        };
        let credential_path = self.credential_path_for_write(&credential_id)?;
        let json = serde_json::to_string_pretty(&credentials)
            .map_err(|e| format!("serialize credentials: {e}"))?;
        write_private_file(&credential_path, &json)?;
        accounts.push(account.clone());
        if let Err(error) = self.save_index_preserving(&accounts, &unknown) {
            let _ = fs::remove_file(&credential_path);
            return Err(error);
        }
        Ok(account)
    }

    /// Reads app-owned credentials by the `credential_id` stored in
    /// `BrowserOAuth` or `XaiManagement`.
    pub fn credentials(&self, credential_id: &str) -> Option<Credentials> {
        let path = self.credential_path_for_read(credential_id).ok()?;
        let json = fs::read_to_string(path).ok()?;
        serde_json::from_str(&json).ok()
    }

    pub fn update_credentials(
        &self,
        credential_id: &str,
        credentials: &Credentials,
    ) -> Result<(), String> {
        let referenced = self
            .list()
            .iter()
            .any(|account| Self::secret_credential_id(&account.auth_source) == Some(credential_id));
        if !referenced {
            return Err("credential is not referenced by an account".to_string());
        }
        let path = self.credential_path_for_write(credential_id)?;
        let json = serde_json::to_string_pretty(credentials)
            .map_err(|e| format!("serialize credentials: {e}"))?;
        write_private_file(&path, &json)
    }

    fn secret_credential_id(source: &AuthSource) -> Option<&str> {
        match source {
            AuthSource::BrowserOAuth { credential_id }
            | AuthSource::XaiManagement { credential_id, .. } => Some(credential_id),
            _ => None,
        }
    }

    pub fn remove(&self, account_id: &str) -> Result<Option<Account>, String> {
        let _guard = index_mutation_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (mut accounts, unknown) = self.read_index_partitioned()?;
        let Some(index) = accounts.iter().position(|account| account.id == account_id) else {
            return Ok(None);
        };
        let removed = accounts.remove(index);
        self.save_index_preserving(&accounts, &unknown)?;

        if let Some(credential_id) = Self::secret_credential_id(&removed.auth_source) {
            let still_referenced = accounts.iter().any(|account| {
                Self::secret_credential_id(&account.auth_source) == Some(credential_id)
            });
            if !still_referenced {
                let directory = self.root.join(CREDS_DIR);
                reject_symlink(&directory, "credential directory")?;
                let path = self.credential_path(credential_id)?;
                reject_symlink(&path, "credential file")?;
                match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!("remove {}: {error}", path.display()));
                    }
                }
            }
        }
        Ok(Some(removed))
    }

    /// Transitional compile shim for callers migrated in later plan tasks.
    pub fn add(
        &self,
        provider: Provider,
        label: String,
        credentials: Credentials,
    ) -> Result<Account, String> {
        match provider {
            Provider::Codex | Provider::Claude | Provider::Agy => {
                self.add_secret(provider, label, SecretSource::BrowserOAuth, credentials)
            }
            #[cfg(feature = "edition-pro")]
            Provider::Cursor => {
                // Derive identity from session (JWT sub or email)
                let db_path = crate::paths::cursor_state_vscdb()
                    .ok_or_else(|| "could not resolve Cursor database path".to_string())?;
                let session = crate::cursor_local::read_cursor_session(&db_path)
                    .map_err(|e| format!("Failed to read Cursor session: {}", e))?;
                self.add_reference(
                    provider,
                    label.clone(),
                    AuthSource::CursorDatabase {
                        database_path: db_path,
                        expected_identity: session.identity.clone(),
                    },
                )
            }
            #[cfg(feature = "edition-pro")]
            Provider::Grok => self.add_secret(
                provider,
                label,
                SecretSource::XaiManagement {
                    team_id: credentials.account_id.clone().unwrap_or_default(),
                },
                credentials,
            ),
            #[cfg(feature = "edition-pro")]
            Provider::Higgsfield => self.add_reference(
                provider,
                label.clone(),
                AuthSource::HiggsfieldCli {
                    expected_identity: label,
                },
            ),
        }
    }

    /// Transitional compile shim for callers migrated in later plan tasks.
    pub fn update_label(&self, id: &str, label: &str) {
        let _guard = index_mutation_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Ok((mut accounts, unknown)) = self.read_index_partitioned() else {
            return;
        };
        let mut changed = false;
        for account in &mut accounts {
            if account.id == id && account.label != label {
                account.label = label.to_string();
                changed = true;
            }
        }
        if changed {
            let _ = self.save_index_preserving(&accounts, &unknown);
        }
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
