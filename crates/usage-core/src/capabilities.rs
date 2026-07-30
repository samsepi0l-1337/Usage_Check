use serde::{Deserialize, Serialize};

use crate::account::Provider;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AuthMethod {
    Cli,
    BrowserOAuth,
    LocalDatabase,
    ManagementKeyClipboard,
    ManagementKeyEnvironment,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthCapability {
    pub methods: &'static [AuthMethod],
}

pub fn auth_capability(provider: Provider) -> AuthCapability {
    let methods: &'static [AuthMethod] = match provider {
        Provider::Codex | Provider::Claude => &[AuthMethod::Cli, AuthMethod::BrowserOAuth],
        Provider::Agy => &[AuthMethod::BrowserOAuth],
        Provider::Cursor => &[AuthMethod::LocalDatabase],
        Provider::Grok => &[
            AuthMethod::ManagementKeyClipboard,
            AuthMethod::ManagementKeyEnvironment,
        ],
        Provider::Higgsfield => &[AuthMethod::Cli],
    };
    AuthCapability { methods }
}
