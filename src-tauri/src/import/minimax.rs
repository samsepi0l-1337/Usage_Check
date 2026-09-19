use std::path::{Path, PathBuf};
use std::process::Command;

use usage_core::account::{Credentials, Provider};

use crate::cli_bin::which_bin;

use super::{default_label, ImportedAccount};

const QUOTA_ARG_SETS: &[&[&str]] = &[
    &["quota", "show", "--output", "json"],
    &["quota", "--json"],
    &["quota", "--output", "json"],
    &["quota", "show", "--json"],
];

fn missing_mmx_bin_error() -> String {
    "MiniMax CLI not found on PATH or Homebrew — install mmx, then run `mmx auth login`".to_string()
}

pub(crate) fn mmx_bin() -> Option<PathBuf> {
    which_bin("mmx")
}

pub(crate) fn fetch_minimax_quota_json_from(bin: &Path) -> Result<serde_json::Value, String> {
    let mut last_err = "MiniMax CLI quota command failed — run `mmx auth login` first".to_string();
    for args in QUOTA_ARG_SETS {
        let output = Command::new(bin).args(*args).output().map_err(|_| {
            format!(
                "MiniMax CLI failed to run at {} — install mmx on PATH/Homebrew",
                bin.display()
            )
        })?;
        if !output.status.success() {
            last_err = "MiniMax CLI quota command failed — run `mmx auth login` first".to_string();
            continue;
        }
        match serde_json::from_slice(&output.stdout) {
            Ok(root) => return Ok(root),
            Err(_) => {
                last_err = "MiniMax CLI quota output is not valid JSON".to_string();
            }
        }
    }
    Err(last_err)
}

pub(crate) fn fetch_minimax_quota_json() -> Result<serde_json::Value, String> {
    let bin = mmx_bin().ok_or_else(missing_mmx_bin_error)?;
    fetch_minimax_quota_json_from(&bin)
}

pub fn load_minimax_cli_auth() -> Result<ImportedAccount, String> {
    load_minimax_cli_auth_with(mmx_bin())
}

pub(crate) fn load_minimax_cli_auth_with(bin: Option<PathBuf>) -> Result<ImportedAccount, String> {
    let bin = bin.ok_or_else(missing_mmx_bin_error)?;
    load_minimax_cli_auth_from(&bin)
}

pub(crate) fn load_minimax_cli_auth_from(bin: &Path) -> Result<ImportedAccount, String> {
    use usage_core::fetch::minimax::parse_minimax_quota;

    let root = fetch_minimax_quota_json_from(bin)?;
    let account = parse_minimax_quota(&root);
    Ok(ImportedAccount {
        label: account
            .email
            .unwrap_or_else(|| default_label(Provider::MiniMax)),
        credentials: Credentials {
            access_token: String::new(),
            refresh_token: None,
            account_id: None,
            expires_at: None,
        },
    })
}

#[cfg(test)]
pub(crate) fn write_fake_minimax_cli(dir: &Path, stdout_json: &str) -> PathBuf {
    super::write_fake_cli(dir, "mmx", stdout_json)
}
