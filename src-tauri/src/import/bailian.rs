use std::path::{Path, PathBuf};
use std::process::Command;

use usage_core::account::{Credentials, Provider};

use crate::cli_bin::which_bin;

use super::{default_label, ImportedAccount};

const TOKEN_PLAN_ARG_SETS: &[&[&str]] = &[
    &["usage", "token-plan", "--output", "json"],
    &[
        "usage",
        "token-plan",
        "--console-site",
        "international",
        "--output",
        "json",
    ],
];

fn missing_bl_bin_error() -> String {
    "Bailian CLI not found on PATH or Homebrew — install Bailian CLI (`bl`), then run `bl auth login`"
        .to_string()
}

pub(crate) fn bl_bin() -> Option<PathBuf> {
    which_bin("bl")
}

pub(crate) fn fetch_bailian_token_plan_json_from(bin: &Path) -> Result<serde_json::Value, String> {
    let mut last_err =
        "Bailian CLI token-plan command failed — run `bl auth login` first".to_string();
    for args in TOKEN_PLAN_ARG_SETS {
        let output = Command::new(bin).args(*args).output().map_err(|_| {
            format!(
                "Bailian CLI failed to run at {} — install Bailian CLI (`bl`) on PATH/Homebrew",
                bin.display()
            )
        })?;
        if !output.status.success() {
            last_err =
                "Bailian CLI token-plan command failed — run `bl auth login` first".to_string();
            continue;
        }
        match serde_json::from_slice(&output.stdout) {
            Ok(root) => return Ok(root),
            Err(_) => {
                last_err = "Bailian CLI token-plan output is not valid JSON".to_string();
            }
        }
    }
    Err(last_err)
}

pub(crate) fn fetch_bailian_token_plan_json() -> Result<serde_json::Value, String> {
    let bin = bl_bin().ok_or_else(missing_bl_bin_error)?;
    fetch_bailian_token_plan_json_from(&bin)
}

pub fn load_bailian_cli_auth() -> Result<ImportedAccount, String> {
    load_bailian_cli_auth_with(bl_bin())
}

pub(crate) fn load_bailian_cli_auth_with(bin: Option<PathBuf>) -> Result<ImportedAccount, String> {
    let bin = bin.ok_or_else(missing_bl_bin_error)?;
    load_bailian_cli_auth_from(&bin)
}

pub(crate) fn load_bailian_cli_auth_from(bin: &Path) -> Result<ImportedAccount, String> {
    let _root = fetch_bailian_token_plan_json_from(bin)?;
    Ok(ImportedAccount {
        label: default_label(Provider::Bailian),
        credentials: Credentials {
            access_token: String::new(),
            refresh_token: None,
            account_id: None,
            expires_at: None,
        },
    })
}

#[cfg(test)]
pub(crate) fn write_fake_bailian_cli(dir: &Path, stdout_json: &str) -> PathBuf {
    super::write_fake_cli(dir, "bl", stdout_json)
}
