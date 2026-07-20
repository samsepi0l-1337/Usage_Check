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

#[cfg(test)]
use usage_core::account::Credentials;

mod credentials;
mod index;
mod mutations;
mod validation;

pub(crate) use index::{parse_index, serialize_index, serialize_index_preserving};

const LEGACY_INDEX_FILE: &str = "accounts.json";
const INDEX_FILE: &str = "accounts-v2.json";
const SCHEMA_MARKER: &str = "schema-v2";
const CREDS_DIR: &str = "credentials";

pub(super) fn set_private_dir_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
}

pub(super) fn write_private_file(path: &Path, contents: &str) -> Result<(), String> {
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

pub(super) fn reject_symlink(path: &Path, description: &str) -> Result<(), String> {
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

    pub(super) fn ensure_root(&self) -> Result<(), String> {
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

    pub(super) fn index_path(&self) -> PathBuf {
        self.root.join(INDEX_FILE)
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
}

#[cfg(test)]
#[path = "../store_tests.rs"]
mod tests;
