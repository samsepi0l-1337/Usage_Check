use std::fs;

use usage_core::account::{Account, AuthSource, Credentials, Provider};

use super::index::index_mutation_lock;
use super::{reject_symlink, AccountStore, SecretSource, CREDS_DIR};

impl AccountStore {
    /// Production entry point: reads live license state. Tests use
    /// [`Self::add_reference_with`] (D3) — except the explicitly-marked
    /// bare-wrapper wiring tests in `store_tests.rs`, which exist precisely to
    /// prove THIS line is wired to real license state.
    pub fn add_reference(
        &self,
        provider: Provider,
        label: String,
        auth_source: AuthSource,
    ) -> Result<Account, String> {
        self.add_reference_with(provider, label, auth_source, crate::license::is_pro)
    }

    /// Core of [`Self::add_reference`], with entitlement injected so it is
    /// evaluated at the linearization point inside the index mutation lock
    /// (plan §03.0).
    ///
    /// `is_pro` is invoked WHILE `index_mutation_lock()` is held and MUST NOT
    /// touch `AccountStore`, directly or transitively — that mutex is not
    /// reentrant and re-entry self-deadlocks. `fn` rather than `impl Fn` so the
    /// callback cannot capture a store to begin with.
    ///
    /// `pub(crate)`, never `pub`: it accepts an arbitrary entitlement, so a
    /// public version would be a supported bypass of the gate this function is
    /// the authority for (D3). Mirrors `poller::poll_all_with`, likewise private.
    pub(crate) fn add_reference_with(
        &self,
        provider: Provider,
        label: String,
        auth_source: AuthSource,
        is_pro: fn() -> bool,
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
        // Linearization point: entitlement is read HERE, under the same lock
        // acquisition as `accounts` and the write below (§03.0).
        if let Some(reason) = Self::free_limit_rejection(&accounts, provider, is_pro()) {
            return Err(reason);
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

    /// Production entry point: reads live license state and forwards it to
    /// [`Self::add_with`].
    pub fn add(
        &self,
        provider: Provider,
        label: String,
        credentials: Credentials,
    ) -> Result<Account, String> {
        self.add_with(provider, label, credentials, crate::license::is_pro)
    }

    /// Core of [`Self::add`], with entitlement injected into the provider's
    /// authoritative store mutation.
    ///
    /// `is_pro` is invoked while `index_mutation_lock()` is held by the
    /// delegated `_with` core. It MUST NOT touch `AccountStore`, directly or
    /// transitively, because that mutex is not reentrant and re-entry
    /// self-deadlocks. `fn` rather than `impl Fn` prevents captured stores.
    pub(crate) fn add_with(
        &self,
        provider: Provider,
        label: String,
        credentials: Credentials,
        is_pro: fn() -> bool,
    ) -> Result<Account, String> {
        match provider {
            Provider::Codex | Provider::Claude | Provider::Agy => self.add_secret_with(
                provider,
                label,
                SecretSource::BrowserOAuth,
                credentials,
                is_pro,
            ),
            Provider::Cursor => {
                // Derive identity from session (JWT sub or email)
                let db_path = crate::paths::cursor_state_vscdb()
                    .ok_or_else(|| "could not resolve Cursor database path".to_string())?;
                let session = crate::cursor_local::read_cursor_session(&db_path)
                    .map_err(|e| format!("Failed to read Cursor session: {}", e))?;
                self.add_reference_with(
                    provider,
                    label.clone(),
                    AuthSource::CursorDatabase {
                        database_path: db_path,
                        expected_identity: session.identity.clone(),
                    },
                    is_pro,
                )
            }
            Provider::Grok => self.add_secret_with(
                provider,
                label,
                SecretSource::XaiManagement {
                    team_id: credentials.account_id.clone().unwrap_or_default(),
                },
                credentials,
                is_pro,
            ),
            Provider::Higgsfield => self.add_reference_with(
                provider,
                label.clone(),
                AuthSource::HiggsfieldCli {
                    expected_identity: label,
                },
                is_pro,
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
