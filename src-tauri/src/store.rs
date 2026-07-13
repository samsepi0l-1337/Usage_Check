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

use usage_core::account::{Account, AuthSource, Credentials, Provider};

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
    serde_json::from_str(s).unwrap_or_default()
}

fn set_private_dir_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
}

fn write_private_file(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create dir: {e}"))?;
        set_private_dir_permissions(parent);
    }
    fs::write(path, contents).map_err(|e| format!("write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
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
                return Err(format!("store root is not a directory: {}", self.root.display()));
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

    /// Transitional compile shim. Schema v2 rejects duplicates while adding
    /// each explicit source, so token/account-ID dedupe no longer exists.
    pub fn dedupe(&self) {}

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
                (AuthSource::HiggsfieldCli { .. }, AuthSource::HiggsfieldCli { .. }) => {
                    Some("Higgsfield CLI account already registered")
                }
                _ => None,
            })
    }

    pub fn add_reference(
        &self,
        provider: Provider,
        label: String,
        auth_source: AuthSource,
    ) -> Result<Account, String> {
        self.initialize_v2()?;
        Self::validate_reference_source(provider, &auth_source)?;
        let mut accounts = self.list();
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
        self.save_index(&accounts)?;
        Ok(account)
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
        let mut accounts = self.list();
        if let Some(reason) = Self::duplicate_source(&accounts, &auth_source) {
            return Err(reason.to_string());
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
        if let Err(error) = self.save_index(&accounts) {
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
        let mut accounts = self.list();
        let Some(index) = accounts.iter().position(|account| account.id == account_id) else {
            return Ok(None);
        };
        let removed = accounts.remove(index);
        self.save_index(&accounts)?;

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
        let mut accounts = self.list();
        let mut changed = false;
        for account in &mut accounts {
            if account.id == id && account.label != label {
                account.label = label.to_string();
                changed = true;
            }
        }
        if changed {
            let _ = self.save_index(&accounts);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use usage_core::account::{Account, AuthSource, ProfileOwnership, Provider};

    struct TestSandbox {
        root: PathBuf,
    }

    impl TestSandbox {
        fn new() -> Self {
            let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
            fs::create_dir_all(&root).expect("create test sandbox");
            Self { root }
        }

        fn store(&self) -> AccountStore {
            AccountStore::new_at(self.root.join("UsageCheck"))
        }
    }

    impl Drop for TestSandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn credentials(identity: &str) -> Credentials {
        Credentials {
            access_token: "test-only-placeholder".into(),
            refresh_token: None,
            account_id: Some(identity.into()),
            expires_at: None,
        }
    }

    #[test]
    fn index_roundtrips() {
        let accts = vec![Account {
            id: "1".into(),
            provider: Provider::Codex,
            label: "work".into(),
            auth_source: AuthSource::CliProfile {
                profile_root: "/profiles/codex-work".into(),
                ownership: ProfileOwnership::External,
                expected_identity: "work".into(),
            },
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

    #[test]
    fn v2_initialization_resets_only_the_injected_legacy_store() {
        let sandbox = TestSandbox::new();
        let store_root = sandbox.root.join("UsageCheck");
        let external_profile = sandbox.root.join("provider-profile");
        fs::create_dir_all(store_root.join("credentials")).unwrap();
        fs::create_dir_all(&external_profile).unwrap();
        fs::write(store_root.join("accounts.json"), "legacy").unwrap();
        fs::write(store_root.join("credentials").join("legacy.json"), "legacy").unwrap();
        fs::write(store_root.join("sibling-sentinel"), "keep").unwrap();
        fs::write(external_profile.join("provider-auth.json"), "keep").unwrap();

        sandbox.store().initialize_v2().unwrap();

        assert!(!store_root.join("accounts.json").exists());
        assert!(!store_root.join("credentials").exists());
        assert!(store_root.join("schema-v2").exists());
        assert!(store_root.join("accounts-v2.json").exists());
        assert_eq!(
            fs::read_to_string(store_root.join("sibling-sentinel")).unwrap(),
            "keep"
        );
        assert_eq!(
            fs::read_to_string(external_profile.join("provider-auth.json")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn v2_initialization_is_idempotent() {
        let sandbox = TestSandbox::new();
        let store = sandbox.store();
        store.initialize_v2().unwrap();
        let account = store
            .add_reference(
                Provider::Codex,
                "work".into(),
                AuthSource::CliProfile {
                    profile_root: sandbox.root.join("codex-profile"),
                    ownership: ProfileOwnership::External,
                    expected_identity: "work@example.com".into(),
                },
            )
            .unwrap();

        store.initialize_v2().unwrap();

        assert_eq!(store.list(), vec![account.clone()]);
        assert_eq!(store.account(&account.id), Some(account));
    }

    #[cfg(unix)]
    #[test]
    fn v2_rejects_symlinked_root_without_touching_target() {
        use std::os::unix::fs::symlink;

        let sandbox = TestSandbox::new();
        let outside = sandbox.root.join("outside-store");
        let linked_root = sandbox.root.join("linked-store");
        fs::create_dir_all(outside.join("credentials")).unwrap();
        fs::write(outside.join("accounts.json"), "legacy").unwrap();
        fs::write(outside.join("credentials").join("legacy.json"), "keep").unwrap();
        symlink(&outside, &linked_root).unwrap();

        let result = AccountStore::new_at(linked_root).initialize_v2();

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(outside.join("accounts.json")).unwrap(), "legacy");
        assert_eq!(
            fs::read_to_string(outside.join("credentials").join("legacy.json")).unwrap(),
            "keep"
        );
        assert!(!outside.join("schema-v2").exists());
        assert!(!outside.join("accounts-v2.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn v2_rejects_symlinked_index_and_marker_without_overwriting_targets() {
        use std::os::unix::fs::symlink;

        let sandbox = TestSandbox::new();
        let root = sandbox.root.join("UsageCheck");
        let outside_index = sandbox.root.join("outside-index");
        let outside_marker = sandbox.root.join("outside-marker");
        fs::create_dir_all(&root).unwrap();
        fs::write(&outside_index, "outside-index").unwrap();
        fs::write(&outside_marker, "2\n").unwrap();
        symlink(&outside_index, root.join("accounts-v2.json")).unwrap();
        symlink(&outside_marker, root.join("schema-v2")).unwrap();

        let result = sandbox.store().initialize_v2();

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(outside_index).unwrap(), "outside-index");
        assert_eq!(fs::read_to_string(outside_marker).unwrap(), "2\n");
    }

    #[cfg(all(unix, feature = "edition-pro"))]
    #[test]
    fn v2_rejects_symlinked_credentials_directory_without_writing_outside() {
        use std::os::unix::fs::symlink;

        let sandbox = TestSandbox::new();
        let store = sandbox.store();
        store.initialize_v2().unwrap();
        let outside = sandbox.root.join("outside-credentials");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("sentinel"), "keep").unwrap();
        symlink(&outside, sandbox.root.join("UsageCheck").join("credentials")).unwrap();

        let result = store.add_secret(
            Provider::Grok,
            "xAI API credits".into(),
            SecretSource::XaiManagement {
                team_id: "team-symlink".into(),
            },
            credentials("xai"),
        );

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(outside.join("sentinel")).unwrap(), "keep");
        assert_eq!(fs::read_dir(outside).unwrap().count(), 1);
    }

    #[cfg(all(unix, feature = "edition-pro"))]
    #[test]
    fn v2_rejects_symlinked_credential_file_without_overwriting_target() {
        use std::os::unix::fs::symlink;

        let sandbox = TestSandbox::new();
        let store = sandbox.store();
        let account = store
            .add_secret(
                Provider::Grok,
                "xAI API credits".into(),
                SecretSource::XaiManagement {
                    team_id: "team-file-symlink".into(),
                },
                credentials("before"),
            )
            .unwrap();
        let AuthSource::XaiManagement { credential_id, .. } = &account.auth_source else {
            unreachable!()
        };
        let credential_path = store.credential_path(credential_id).unwrap();
        fs::remove_file(&credential_path).unwrap();
        let outside = sandbox.root.join("outside-credential-file");
        fs::write(&outside, "keep").unwrap();
        symlink(&outside, &credential_path).unwrap();

        let result = store.update_credentials(credential_id, &credentials("after"));

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "keep");
    }

    #[cfg(feature = "edition-pro")]
    #[test]
    fn v2_grok_compatibility_add_uses_credential_id_for_credentials() {
        let sandbox = TestSandbox::new();
        let store = sandbox.store();
        let account = store
            .add(
                Provider::Grok,
                "xAI API credits".into(),
                credentials("team-compat"),
            )
            .unwrap();

        let AuthSource::XaiManagement { credential_id, .. } = &account.auth_source else {
            panic!("Grok compatibility add must use xAI Management credentials")
        };
        assert_eq!(store.credentials(credential_id), Some(credentials("team-compat")));
    }

    #[cfg(feature = "edition-pro")]
    #[test]
    fn cursor_add_reference_creates_no_secret_file() {
        let sandbox = TestSandbox::new();
        let store = sandbox.store();
        let database_path = sandbox.root.join("state.vscdb");
        let auth_source = AuthSource::CursorDatabase {
            database_path,
            expected_identity: "cursor@example.com".into(),
        };

        let account = store
            .add_reference(Provider::Cursor, "cursor".into(), auth_source.clone())
            .unwrap();

        assert_eq!(account.auth_source, auth_source);
        assert_eq!(store.account(&account.id), Some(account));
        assert!(!sandbox.root.join("UsageCheck").join("credentials").exists());
    }

    #[cfg(feature = "edition-pro")]
    #[test]
    fn v2_reference_sources_create_no_secret_files() {
        let sandbox = TestSandbox::new();
        let store = sandbox.store();
        store.initialize_v2().unwrap();

        store
            .add_reference(
                Provider::Codex,
                "codex".into(),
                AuthSource::CliProfile {
                    profile_root: sandbox.root.join("codex-profile"),
                    ownership: ProfileOwnership::Managed,
                    expected_identity: "codex@example.com".into(),
                },
            )
            .unwrap();
        store
            .add_reference(
                Provider::Cursor,
                "cursor".into(),
                AuthSource::CursorDatabase {
                    database_path: sandbox.root.join("state.vscdb"),
                    expected_identity: "cursor@example.com".into(),
                },
            )
            .unwrap();
        store
            .add_reference(
                Provider::Higgsfield,
                "higgsfield".into(),
                AuthSource::HiggsfieldCli {
                    expected_identity: "higgsfield@example.com".into(),
                },
            )
            .unwrap();

        assert_eq!(store.list().len(), 3);
        assert!(!sandbox.root.join("UsageCheck").join("credentials").exists());
    }

    #[cfg(feature = "edition-pro")]
    #[test]
    fn v2_app_owned_sources_resolve_and_update_credentials_by_credential_id() {
        let sandbox = TestSandbox::new();
        let store = sandbox.store();
        store.initialize_v2().unwrap();

        let browser = store
            .add_secret(
                Provider::Agy,
                "agy".into(),
                SecretSource::BrowserOAuth,
                credentials("agy-before"),
            )
            .unwrap();
        let xai = store
            .add_secret(
                Provider::Grok,
                "xAI API credits".into(),
                SecretSource::XaiManagement {
                    team_id: "team-1".into(),
                },
                credentials("xai"),
            )
            .unwrap();

        let AuthSource::BrowserOAuth {
            credential_id: browser_credential_id,
        } = &browser.auth_source
        else {
            panic!("browser account must own browser credentials");
        };
        let AuthSource::XaiManagement {
            credential_id: xai_credential_id,
            team_id,
        } = &xai.auth_source
        else {
            panic!("xAI account must own management credentials");
        };
        assert_eq!(team_id, "team-1");
        assert_eq!(
            store.credentials(browser_credential_id.as_str()),
            Some(credentials("agy-before"))
        );
        assert_eq!(
            store.credentials(xai_credential_id.as_str()),
            Some(credentials("xai"))
        );

        let updated = credentials("agy-after");
        store
            .update_credentials(browser_credential_id.as_str(), &updated)
            .unwrap();
        assert_eq!(
            store.credentials(browser_credential_id.as_str()),
            Some(updated)
        );
    }

    #[test]
    fn v2_remove_deletes_only_the_removed_accounts_app_owned_secret() {
        let sandbox = TestSandbox::new();
        let store = sandbox.store();
        store.initialize_v2().unwrap();
        let external_profile = sandbox.root.join("external-profile");
        fs::create_dir_all(&external_profile).unwrap();
        fs::write(external_profile.join("provider-auth.json"), "keep").unwrap();

        let reference = store
            .add_reference(
                Provider::Claude,
                "claude".into(),
                AuthSource::CliProfile {
                    profile_root: external_profile.clone(),
                    ownership: ProfileOwnership::External,
                    expected_identity: "claude@example.com".into(),
                },
            )
            .unwrap();
        let first = store
            .add_secret(
                Provider::Agy,
                "first".into(),
                SecretSource::BrowserOAuth,
                credentials("first"),
            )
            .unwrap();
        let second = store
            .add_secret(
                Provider::Agy,
                "second".into(),
                SecretSource::BrowserOAuth,
                credentials("second"),
            )
            .unwrap();
        let AuthSource::BrowserOAuth {
            credential_id: first_credential_id,
        } = &first.auth_source
        else {
            unreachable!()
        };
        let AuthSource::BrowserOAuth {
            credential_id: second_credential_id,
        } = &second.auth_source
        else {
            unreachable!()
        };
        let first_credential_id = first_credential_id.clone();
        let second_credential_id = second_credential_id.clone();

        assert_eq!(store.remove(&reference.id).unwrap(), Some(reference));
        assert!(external_profile.join("provider-auth.json").exists());
        assert!(store.credentials(&first_credential_id).is_some());
        assert!(store.credentials(&second_credential_id).is_some());

        assert_eq!(store.remove(&first.id).unwrap(), Some(first));
        assert!(store.credentials(&first_credential_id).is_none());
        assert!(store.credentials(&second_credential_id).is_some());
        assert_eq!(store.account(&second.id), Some(second));
    }

    #[test]
    fn v2_duplicate_profile_roots_are_rejected_without_overwrite() {
        let sandbox = TestSandbox::new();
        let store = sandbox.store();
        store.initialize_v2().unwrap();
        let profile_root = sandbox.root.join("shared-profile");
        let first = store
            .add_reference(
                Provider::Codex,
                "first".into(),
                AuthSource::CliProfile {
                    profile_root: profile_root.clone(),
                    ownership: ProfileOwnership::External,
                    expected_identity: "first@example.com".into(),
                },
            )
            .unwrap();

        let duplicate = store.add_reference(
            Provider::Codex,
            "replacement".into(),
            AuthSource::CliProfile {
                profile_root,
                ownership: ProfileOwnership::Managed,
                expected_identity: "replacement@example.com".into(),
            },
        );

        assert!(duplicate.is_err());
        assert_eq!(store.list(), vec![first]);
    }

    #[cfg(feature = "edition-pro")]
    #[test]
    fn v2_duplicate_cursor_identities_are_rejected_without_overwrite() {
        let sandbox = TestSandbox::new();
        let store = sandbox.store();
        store.initialize_v2().unwrap();
        let first = store
            .add_reference(
                Provider::Cursor,
                "first".into(),
                AuthSource::CursorDatabase {
                    database_path: sandbox.root.join("first.vscdb"),
                    expected_identity: "cursor-user".into(),
                },
            )
            .unwrap();

        let duplicate = store.add_reference(
            Provider::Cursor,
            "replacement".into(),
            AuthSource::CursorDatabase {
                database_path: sandbox.root.join("replacement.vscdb"),
                expected_identity: "cursor-user".into(),
            },
        );

        assert!(duplicate.is_err());
        assert_eq!(store.list(), vec![first]);
    }

    #[cfg(feature = "edition-pro")]
    #[test]
    fn v2_duplicate_xai_team_ids_are_rejected_without_overwrite() {
        let sandbox = TestSandbox::new();
        let store = sandbox.store();
        store.initialize_v2().unwrap();
        let first = store
            .add_secret(
                Provider::Grok,
                "first".into(),
                SecretSource::XaiManagement {
                    team_id: "team-1".into(),
                },
                credentials("first"),
            )
            .unwrap();
        let AuthSource::XaiManagement { credential_id, .. } = &first.auth_source else {
            unreachable!()
        };
        let credential_id = credential_id.clone();

        let duplicate = store.add_secret(
            Provider::Grok,
            "replacement".into(),
            SecretSource::XaiManagement {
                team_id: "team-1".into(),
            },
            credentials("replacement"),
        );

        assert!(duplicate.is_err());
        assert_eq!(store.list(), vec![first]);
        assert_eq!(
            store.credentials(&credential_id),
            Some(credentials("first"))
        );
        assert_eq!(
            fs::read_dir(sandbox.root.join("UsageCheck").join("credentials"))
                .unwrap()
                .count(),
            1
        );
    }
}
