//! Explicit API-key imports. Polling uses persisted app-owned credentials.
use sha2::{Digest, Sha256};
use usage_core::account::{Credentials, Provider};

use super::{default_label, ImportedAccount};

pub(crate) fn api_key_account(provider: Provider, text: &str) -> Result<ImportedAccount, String> {
    if !matches!(provider, Provider::Moonshot | Provider::NanoGpt) {
        return Err("provider does not support API-key clipboard import".into());
    }
    let key = text.trim();
    if key.is_empty() || key.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("copy one API key without internal whitespace, then import again".into());
    }
    let digest = Sha256::digest(key.as_bytes());
    let fingerprint: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(ImportedAccount {
        label: format!("{} · key {}", default_label(provider), &fingerprint[..8]),
        credentials: Credentials {
            access_token: key.into(),
            refresh_token: None,
            account_id: Some(format!("key:{fingerprint}")),
            expires_at: None,
        },
    })
}

pub(crate) fn import_api_key_from_clipboard(provider: Provider) -> Result<ImportedAccount, String> {
    let text = arboard::Clipboard::new()
        .map_err(|_| "clipboard unavailable".to_string())?
        .get_text()
        .map_err(|_| "clipboard has no text — copy your API key, then import again".to_string())?;
    api_key_account(provider, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_wallet_keys_use_the_existing_persisted_account_path() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::AccountStore::new_at(dir.path().into());
        for provider in [Provider::Moonshot, Provider::NanoGpt] {
            let imported = api_key_account(provider, "wallet-key").unwrap();
            let account = store
                .add_with(provider, imported.label, imported.credentials, || true)
                .unwrap();
            assert_eq!(account.provider, provider);
            let same = api_key_account(provider, "wallet-key").unwrap();
            assert!(store
                .add_with(provider, same.label, same.credentials, || true)
                .unwrap_err()
                .contains("already registered"));
            let different = api_key_account(provider, "other-wallet-key").unwrap();
            assert_ne!(account.label, different.label);
            assert!(store
                .add_with(provider, different.label, different.credentials, || true)
                .is_ok());
            assert_eq!(
                store
                    .credentials(crate::store::AccountStore::credential_key(&account))
                    .unwrap()
                    .access_token,
                "wallet-key"
            );
        }
    }

    #[test]
    fn imports_only_a_single_key_and_never_echoes_rejected_secrets() {
        for provider in [Provider::Moonshot, Provider::NanoGpt] {
            let imported = api_key_account(provider, "  secret-key\n").unwrap();
            assert_eq!(imported.credentials.access_token, "secret-key");
            assert!(!imported.label.contains("secret-key"));
            assert_eq!(
                imported.label,
                api_key_account(provider, "secret-key").unwrap().label
            );
            assert!(imported.credentials.refresh_token.is_none());
            for input in ["", "secret-key\nsecond-key", "secret-key\u{0000}"] {
                let error = api_key_account(provider, input).unwrap_err();
                assert!(!error.contains("secret-key"));
            }
        }
        assert!(api_key_account(Provider::Codex, "secret-key").is_err());
    }
}
