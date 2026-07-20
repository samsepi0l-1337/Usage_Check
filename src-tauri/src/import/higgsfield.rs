use std::process::Command;

use usage_core::account::{Credentials, Provider};

use super::{default_label, ImportedAccount};

#[cfg(feature = "edition-pro")]
/// Higgsfield CLI account reference from `higgsfield account status --json`.
pub fn load_higgsfield_cli_auth() -> Result<ImportedAccount, String> {
    use usage_core::fetch::higgsfield::parse_higgsfield_account;

    let output = Command::new("higgsfield")
        .args(["account", "status", "--json"])
        .output()
        .map_err(|_| {
            "Higgsfield CLI unavailable — run `higgsfield auth login` first".to_string()
        })?;
    if !output.status.success() {
        return Err(
            "Higgsfield CLI status command failed — run `higgsfield auth login` first".into(),
        );
    }
    let root: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "Higgsfield CLI status output is not valid JSON".to_string())?;
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
