use std::path::{Path, PathBuf};
use std::process::Command;

use usage_core::account::{Credentials, Provider};

use crate::cli_bin::which_bin;

use super::{default_label, ImportedAccount};

/// Absolute Higgsfield CLI path from PATH plus Homebrew/user-local dirs.
pub(crate) fn higgsfield_bin() -> Option<PathBuf> {
    which_bin("higgsfield")
}

fn missing_higgsfield_bin_error() -> String {
    "Higgsfield CLI not found on PATH or Homebrew — install higgsfield, then run `higgsfield auth login`"
        .to_string()
}

/// `higgsfield account status --json` from an already-resolved binary.
pub(crate) fn fetch_higgsfield_account_json_from(bin: &Path) -> Result<serde_json::Value, String> {
    let output = Command::new(bin)
        .args(["account", "status", "--json"])
        .output()
        .map_err(|_| {
            format!(
                "Higgsfield CLI failed to run at {} — install higgsfield on PATH/Homebrew",
                bin.display()
            )
        })?;
    if !output.status.success() {
        return Err(
            "Higgsfield CLI status command failed — run `higgsfield auth login` first".into(),
        );
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|_| "Higgsfield CLI status output is not valid JSON".to_string())
}

/// Higgsfield CLI account reference from `higgsfield account status --json`.
pub fn load_higgsfield_cli_auth() -> Result<ImportedAccount, String> {
    load_higgsfield_cli_auth_with(higgsfield_bin())
}

pub(crate) fn load_higgsfield_cli_auth_with(
    bin: Option<PathBuf>,
) -> Result<ImportedAccount, String> {
    let bin = bin.ok_or_else(missing_higgsfield_bin_error)?;
    load_higgsfield_cli_auth_from(&bin)
}

pub(crate) fn load_higgsfield_cli_auth_from(bin: &Path) -> Result<ImportedAccount, String> {
    use usage_core::fetch::higgsfield::parse_higgsfield_account;

    let root = fetch_higgsfield_account_json_from(bin)?;
    let account = parse_higgsfield_account(&root);
    Ok(ImportedAccount {
        label: account
            .email
            .unwrap_or_else(|| default_label(Provider::Higgsfield)),
        credentials: Credentials {
            access_token: String::new(),
            refresh_token: None,
            account_id: None,
            expires_at: None,
        },
    })
}

#[cfg(test)]
pub(crate) fn write_fake_higgsfield_cli(dir: &Path, stdout_json: &str) -> PathBuf {
    std::fs::write(dir.join("account.json"), stdout_json).expect("write fake higgsfield JSON");
    #[cfg(windows)]
    {
        let path = dir.join("higgsfield.cmd");
        std::fs::write(&path, "@echo off\r\ntype \"%~dp0account.json\"\r\n")
            .expect("write fake higgsfield.cmd");
        path
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("higgsfield");
        std::fs::write(&path, "#!/bin/sh\ncat \"$(dirname \"$0\")/account.json\"\n")
            .expect("write fake higgsfield script");
        let mut perms = std::fs::metadata(&path)
            .expect("stat fake higgsfield")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod fake higgsfield");
        path
    }
}
