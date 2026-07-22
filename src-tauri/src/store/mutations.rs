use std::fs;

use usage_core::account::{Account, AuthSource, Credentials, Provider};

use super::index::index_mutation_lock;
use super::{reject_symlink, AccountStore, SecretSource, CREDS_DIR};

impl AccountStore {
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
        self.remove_cli_profile_credentials(&removed.id);
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
