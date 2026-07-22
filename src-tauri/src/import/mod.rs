//! Import credentials from CLI config files.
//!
//! Codex: `~/.codex/auth.json` (or `$CODEX_HOME/auth.json`)
//! Claude: macOS Keychain / Windows Credential Manager service
//!   `Claude Code-credentials` (preferred), then
//!   `~/.claude/.credentials.json` (or `$CLAUDE_CONFIG_DIR/...`)
//! Agy: not imported from CLI token DBs — use browser OAuth (`add-agy-oauth`).
//!
//! SECURITY: never log/print access_token or refresh_token values.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
#[cfg(test)]
use chrono::Utc;
use usage_core::account::{Credentials, Provider};

mod claude;
mod codex;
#[cfg(feature = "edition-pro")]
mod grok;
#[cfg(feature = "edition-pro")]
mod higgsfield;

#[cfg(test)]
pub(crate) static CLAUDE_CONFIG_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[allow(unused_imports)]
pub(crate) use claude::{
    claude_oauth_identity_set_in, load_claude_cli_auth, load_claude_default_login_credentials,
    load_claude_profile_credentials, parse_claude_credentials_json,
};
#[cfg(test)]
use claude::claude_profile_is_default;
pub(crate) use codex::{parse_codex_auth_json, load_codex_cli_auth};
#[cfg(feature = "edition-pro")]
#[allow(unused_imports)]
pub(crate) use grok::{
    import_grok_from_clipboard, load_grok_env_auth, grok_imported_account,
    validate_grok_management_key,
};
#[cfg(feature = "edition-pro")]
pub(crate) use higgsfield::load_higgsfield_cli_auth;

/// Result of a CLI import: credentials plus a human-readable label
/// (email when available).
#[derive(Clone, Debug)]
pub struct ImportedAccount {
    pub credentials: Credentials,
    pub label: String,
}

/// Extracts `email` from a JWT payload (Codex id_token). Pure — never logs.
pub fn email_from_jwt(jwt: &str) -> Option<String> {
    let payload_b64 = jwt.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let root: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    root.get("email")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub(crate) fn default_label(provider: Provider) -> String {
    provider.display_name().to_string()
}

/// Loads credentials for `provider` from the local CLI config.
/// Agy has no CLI auth import — use browser OAuth (`add-agy-oauth`).
pub fn import_from_cli(provider: Provider) -> Result<ImportedAccount, String> {
    match provider {
        Provider::Agy => Err(
            "Antigravity is not imported from local CLI token DBs — use Login Antigravity (browser)"
                .into(),
        ),
        Provider::Codex => load_codex_cli_auth(),
        Provider::Claude => load_claude_cli_auth(),
        #[cfg(feature = "edition-pro")]
        Provider::Cursor => crate::cursor_local::load_cursor_local_auth(),
        #[cfg(feature = "edition-pro")]
        Provider::Grok => load_grok_env_auth(),
        #[cfg(feature = "edition-pro")]
        Provider::Higgsfield => load_higgsfield_cli_auth(),
    }
}

#[cfg(test)]
#[path = "../import_tests.rs"]
mod tests;
