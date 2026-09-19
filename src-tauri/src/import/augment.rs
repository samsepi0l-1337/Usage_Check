use std::path::{Path, PathBuf};
use std::process::Command;

use usage_core::account::{Credentials, Provider};

use crate::cli_bin::which_bin;

use super::{default_label, ImportedAccount};

fn missing_auggie_bin_error() -> String {
    "Augment CLI not found on PATH or Homebrew — install auggie (`npm i -g @augmentcode/auggie`), then run `auggie login`"
        .to_string()
}

pub(crate) fn auggie_bin() -> Option<PathBuf> {
    which_bin("auggie")
}

pub(crate) fn fetch_augment_account_json_from(bin: &Path) -> Result<serde_json::Value, String> {
    let output = Command::new(bin)
        .args(["account", "status", "--json"])
        .output()
        .map_err(|_| {
            format!(
                "Augment CLI failed to run at {} — install auggie on PATH/Homebrew",
                bin.display()
            )
        })?;
    if !output.status.success() {
        return Err("Augment CLI status command failed — run `auggie login` first".into());
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|_| "Augment CLI status output is not valid JSON".to_string())
}

pub(crate) fn fetch_augment_account_json() -> Result<serde_json::Value, String> {
    let bin = auggie_bin().ok_or_else(missing_auggie_bin_error)?;
    fetch_augment_account_json_from(&bin)
}

pub fn load_augment_cli_auth() -> Result<ImportedAccount, String> {
    load_augment_cli_auth_with(auggie_bin())
}

pub(crate) fn load_augment_cli_auth_with(bin: Option<PathBuf>) -> Result<ImportedAccount, String> {
    let bin = bin.ok_or_else(missing_auggie_bin_error)?;
    load_augment_cli_auth_from(&bin)
}

pub(crate) fn load_augment_cli_auth_from(bin: &Path) -> Result<ImportedAccount, String> {
    use usage_core::fetch::augment::parse_augment_account;

    let root = fetch_augment_account_json_from(bin)?;
    let account = parse_augment_account(&root);
    Ok(ImportedAccount {
        label: account
            .email
            .unwrap_or_else(|| default_label(Provider::Augment)),
        credentials: Credentials {
            access_token: String::new(),
            refresh_token: None,
            account_id: None,
            expires_at: None,
        },
    })
}

#[cfg(test)]
pub(crate) fn write_fake_augment_cli(dir: &Path, stdout_json: &str) -> PathBuf {
    super::write_fake_cli(dir, "auggie", stdout_json)
}
