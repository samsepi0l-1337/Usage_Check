use usage_core::account::{Account, AuthSource, Credentials, Provider};

use super::{AccountStore, SecretSource};

impl AccountStore {
    pub(super) fn validate_reference_source(
        provider: Provider,
        source: &AuthSource,
    ) -> Result<(), String> {
        let valid = matches!(
            (provider, source),
            (
                Provider::Codex | Provider::Claude,
                AuthSource::CliProfile { .. }
            ) | (Provider::Cursor, AuthSource::CursorDatabase { .. })
                | (Provider::Higgsfield, AuthSource::HiggsfieldCli { .. })
        );
        if valid {
            Ok(())
        } else {
            Err("authentication source is not a reference for this provider".to_string())
        }
    }

    pub(super) fn validate_secret_source(
        provider: Provider,
        source: &SecretSource,
    ) -> Result<(), String> {
        let valid = match source {
            SecretSource::BrowserOAuth => {
                matches!(provider, Provider::Codex | Provider::Claude | Provider::Agy)
            }
            SecretSource::XaiManagement { .. } => provider == Provider::Grok,
        };
        if valid {
            Ok(())
        } else {
            Err("authentication source is not an app-owned secret for this provider".to_string())
        }
    }

    pub(super) fn duplicate_source(
        accounts: &[Account],
        source: &AuthSource,
    ) -> Option<&'static str> {
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
    pub(super) fn oauth_identity_keys(label: &str, creds: &Credentials) -> Vec<String> {
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
    pub(super) fn oauth_duplicate(
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

    /// Free-state per-provider account cap. Returns the user-facing reason when
    /// a Free installation may NOT register another `provider` account.
    ///
    /// Checked AFTER `duplicate_source`/`oauth_duplicate` (see the call sites):
    /// re-adding an account that is ALREADY registered must report "already
    /// registered" — the true cause — rather than a cap that would only have
    /// applied to a genuinely new account.
    ///
    /// This refuses a WRITE; it never deletes. Accounts already past the cap
    /// keep their index entry and surface as `pro_required` at poll time
    /// (`poller::poll_all_with`). The text comes from
    /// `usage_core::edition::free_limit_reason` so the store and the tray emit
    /// the identical sentence (D8).
    pub(super) fn free_limit_rejection(
        accounts: &[Account],
        provider: Provider,
        is_pro: bool,
    ) -> Option<String> {
        if is_pro || !usage_core::edition::free_limit_reached(accounts, provider) {
            return None;
        }
        Some(usage_core::edition::free_limit_reason(provider))
    }
}
