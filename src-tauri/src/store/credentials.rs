use std::fs;
use std::path::PathBuf;

use usage_core::account::{Account, AuthSource, Credentials, Provider};

use super::index::index_mutation_lock;
use super::{reject_symlink, set_private_dir_permissions, write_private_file, AccountStore, SecretSource, CREDS_DIR};

/// App-owned refreshable token cache for CLI-profile accounts, keyed by
/// `account_id`. Restores the v0.1.0 model: the app owns a COPY of the imported
/// CLI credentials and rotates it in its OWN store, so an expired-but-refreshable
/// CLI login shows live usage instead of `needs_login`. The Claude Code keychain
/// is never written. Dedicated subdir so it never interacts with the
/// credential-id lifecycle (index / account removal).
const CLI_TOKEN_CACHE_DIR: &str = "cli-token-cache";

impl AccountStore {
    pub(super) fn credential_path(&self, credential_id: &str) -> Result<PathBuf, String> {
        uuid::Uuid::parse_str(credential_id).map_err(|_| "invalid credential id".to_string())?;
        Ok(self
            .root
            .join(CREDS_DIR)
            .join(format!("{credential_id}.json")))
    }

    pub(super) fn credential_path_for_write(
        &self,
        credential_id: &str,
    ) -> Result<PathBuf, String> {
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

    pub(super) fn credential_path_for_read(
        &self,
        credential_id: &str,
    ) -> Result<PathBuf, String> {
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

    /// Reads the app-owned refreshable token copy for a CLI-profile account
    /// (keyed by `account_id`). `None` when absent/unreadable/invalid id.
    /// SECURITY: never log the returned tokens.
    pub fn cli_profile_credentials(&self, account_id: &str) -> Option<Credentials> {
        uuid::Uuid::parse_str(account_id).ok()?;
        let path = self
            .root
            .join(CLI_TOKEN_CACHE_DIR)
            .join(format!("{account_id}.json"));
        reject_symlink(&path, "cli token cache file").ok()?;
        let json = fs::read_to_string(&path).ok()?;
        serde_json::from_str(&json).ok()
    }

    /// Persists the app-owned refreshable token copy (0600, atomic private write via
    /// `write_private_file`). The Claude Code keychain is never touched.
    pub fn set_cli_profile_credentials(
        &self,
        account_id: &str,
        credentials: &Credentials,
    ) -> Result<(), String> {
        uuid::Uuid::parse_str(account_id).map_err(|_| "invalid account id".to_string())?;
        let directory = self.root.join(CLI_TOKEN_CACHE_DIR);
        reject_symlink(&directory, "cli token cache directory")?;
        let path = directory.join(format!("{account_id}.json"));
        reject_symlink(&path, "cli token cache file")?;
        let json = serde_json::to_string_pretty(credentials)
            .map_err(|e| format!("serialize credentials: {e}"))?;
        write_private_file(&path, &json)
    }

    pub(super) fn remove_cli_profile_credentials(&self, account_id: &str) {
        if uuid::Uuid::parse_str(account_id).is_err() {
            return;
        }
        let path = self
            .root
            .join(CLI_TOKEN_CACHE_DIR)
            .join(format!("{account_id}.json"));
        if let Err(error) = reject_symlink(&path, "cli token cache file") {
            eprintln!("failed to remove {}: {error}", path.display());
            return;
        }
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => eprintln!("failed to remove {}: {error}", path.display()),
        }
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

    pub(super) fn secret_credential_id(source: &AuthSource) -> Option<&str> {
        match source {
            AuthSource::BrowserOAuth { credential_id }
            | AuthSource::XaiManagement { credential_id, .. } => Some(credential_id),
            _ => None,
        }
    }
}
