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

mod amp;
mod augment;
mod bailian;
mod claude;
mod codex;
mod copilot;
mod deepseek;
mod factory;
mod fireworks;
mod grok;
mod higgsfield;
mod kimi;
mod kiro;
mod minimax;
mod novita;
mod opencode;
mod openrouter;
mod poe;
mod zai;

#[cfg(test)]
pub(crate) static CLAUDE_CONFIG_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[allow(unused_imports)]
pub(crate) use amp::{load_amp_cli_auth, parse_amp_secrets};
pub(crate) use augment::{fetch_augment_account_json, load_augment_cli_auth};
#[allow(unused_imports)]
pub(crate) use bailian::{fetch_bailian_token_plan_json, load_bailian_cli_auth};
#[cfg(test)]
use claude::claude_profile_is_default;
#[allow(unused_imports)]
pub(crate) use claude::{
    claude_oauth_identity_set_in, load_claude_cli_auth, load_claude_default_login_credentials,
    load_claude_profile_credentials, parse_claude_credentials_json,
};
pub(crate) use codex::{load_codex_cli_auth, parse_codex_auth_json};
#[allow(unused_imports)]
pub(crate) use copilot::{load_copilot_cli_auth, parse_copilot_oauth_token, parse_gh_hosts_yml};
pub(crate) use deepseek::{load_deepseek_cli_auth, parse_deepseek_api_key};
#[allow(unused_imports)]
pub(crate) use factory::{load_factory_cli_auth, parse_factory_auth_json};
#[allow(unused_imports)]
pub(crate) use fireworks::{load_fireworks_cli_auth, parse_fireworks_auth_ini};
#[allow(unused_imports)]
pub(crate) use grok::{
    grok_imported_account, import_grok_from_clipboard, load_grok_env_auth,
    validate_grok_management_key,
};
pub(crate) use higgsfield::load_higgsfield_cli_auth;
#[allow(unused_imports)]
pub(crate) use kimi::{load_kimi_cli_auth, parse_kimi_credentials_json};
#[allow(unused_imports)]
pub(crate) use kiro::{
    kiro_endpoints, kiro_region_from_token, kiro_region_ok, load_kiro_cli_auth,
    parse_kiro_auth_token, read_kiro_usage_state, region_from_profile_arn,
};
#[allow(unused_imports)]
pub(crate) use minimax::{fetch_minimax_quota_json, load_minimax_cli_auth};
#[allow(unused_imports)]
pub(crate) use novita::{load_novita_cli_auth, parse_novita_config};
#[allow(unused_imports)]
pub(crate) use opencode::{load_opencode_cli_auth, parse_opencode_go_auth_json};
#[allow(unused_imports)]
pub(crate) use openrouter::{
    load_openrouter_cli_auth, parse_openrouter_opencode_auth, parse_ori_api_key,
};
#[allow(unused_imports)]
pub(crate) use poe::{decrypt_poe_credentials_enc, load_poe_cli_auth, parse_poe_plaintext_key};
#[allow(unused_imports)]
pub(crate) use zai::{
    load_zai_cli_auth, parse_hermes_zai_auth, parse_opencode_zai_auth, parse_zcode_config_key,
};

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
        Provider::Cursor => crate::cursor_local::load_cursor_local_auth(),
        Provider::Grok => load_grok_env_auth(),
        Provider::Higgsfield => load_higgsfield_cli_auth(),
        Provider::MiniMax => load_minimax_cli_auth(),
        Provider::Augment => load_augment_cli_auth(),
        Provider::Kimi => load_kimi_cli_auth(),
        Provider::OpenCode => load_opencode_cli_auth(),
        Provider::DeepSeek => load_deepseek_cli_auth(),
        Provider::OpenRouter => load_openrouter_cli_auth(),
        Provider::Copilot => load_copilot_cli_auth(),
        Provider::Windsurf => crate::windsurf_local::load_windsurf_local_auth(),
        Provider::Poe => load_poe_cli_auth(),
        Provider::Fireworks => load_fireworks_cli_auth(),
        Provider::Novita => load_novita_cli_auth(),
        Provider::Amp => load_amp_cli_auth(),
        Provider::Zai => load_zai_cli_auth(),
        Provider::Bailian => load_bailian_cli_auth(),
        Provider::Trae => crate::trae_local::load_trae_local_auth(),
        Provider::Kiro => load_kiro_cli_auth(),
        Provider::Factory => load_factory_cli_auth(),
    }
}

#[cfg(test)]
pub(crate) fn write_fake_cli(
    dir: &std::path::Path,
    name: &str,
    stdout_json: &str,
) -> std::path::PathBuf {
    std::fs::write(dir.join("account.json"), stdout_json).expect("write fake CLI JSON");
    #[cfg(windows)]
    {
        let path = dir.join(format!("{name}.cmd"));
        std::fs::write(&path, "@echo off\r\ntype \"%~dp0account.json\"\r\n")
            .expect("write fake CLI cmd");
        path
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\ncat \"$(dirname \"$0\")/account.json\"\n")
            .expect("write fake CLI script");
        let mut perms = std::fs::metadata(&path)
            .expect("stat fake CLI")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod fake CLI");
        path
    }
}

#[cfg(test)]
#[path = "../import_tests.rs"]
mod tests;
