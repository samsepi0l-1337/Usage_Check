use usage_core::account::Provider;

use usage_core::AuthMethod;


#[derive(Clone, Copy, Debug)]
pub struct AuthActionSpec {
    pub provider: Provider,
    pub method: AuthMethod,
    pub event_id: &'static str,
    pub label: &'static str,
}

pub fn auth_action_specs() -> &'static [AuthActionSpec] {
    &[
        AuthActionSpec {
            provider: Provider::Codex,
            method: AuthMethod::Cli,
            event_id: "add-codex-cli",
            label: "Add Codex (CLI)",
        },
        AuthActionSpec {
            provider: Provider::Codex,
            method: AuthMethod::BrowserOAuth,
            event_id: "add-codex-oauth",
            label: "Login Codex (browser)",
        },
        AuthActionSpec {
            provider: Provider::Claude,
            method: AuthMethod::Cli,
            event_id: "add-claude-cli",
            label: "Add Claude (CLI)",
        },
        AuthActionSpec {
            provider: Provider::Claude,
            method: AuthMethod::BrowserOAuth,
            event_id: "add-claude-oauth",
            label: "Login Claude (browser)",
        },
        AuthActionSpec {
            provider: Provider::Agy,
            method: AuthMethod::BrowserOAuth,
            event_id: "add-agy-oauth",
            label: "Login Antigravity (browser)",
        },
        #[cfg(feature = "edition-pro")]
        AuthActionSpec {
            provider: Provider::Cursor,
            method: AuthMethod::LocalDatabase,
            event_id: "add-cursor-local",
            label: "Import Cursor (local, Experimental)",
        },
        #[cfg(feature = "edition-pro")]
        AuthActionSpec {
            provider: Provider::Grok,
            method: AuthMethod::ManagementKeyClipboard,
            event_id: "add-grok-clipboard",
            label: "Import xAI API credits (clipboard)",
        },
        #[cfg(feature = "edition-pro")]
        AuthActionSpec {
            provider: Provider::Grok,
            method: AuthMethod::ManagementKeyEnvironment,
            event_id: "add-grok-env",
            label: "Import xAI API credits (env vars)",
        },
        #[cfg(feature = "edition-pro")]
        AuthActionSpec {
            provider: Provider::Higgsfield,
            method: AuthMethod::Cli,
            event_id: "add-higgsfield-cli",
            label: "Add Higgsfield (CLI)",
        },
    ]
}

/// Resolves an Add Account tray-menu event through the auth-action registry.
pub fn spec_for_event(event_id: &str) -> Option<AuthActionSpec> {
    auth_action_specs()
        .iter()
        .copied()
        .find(|spec| spec.event_id == event_id)
}
