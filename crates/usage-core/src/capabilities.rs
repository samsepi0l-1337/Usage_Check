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
        #[cfg(feature = "edition-pro")]
        Provider::Cursor => &[AuthMethod::LocalDatabase],
        #[cfg(feature = "edition-pro")]
        Provider::Grok => &[
            AuthMethod::ManagementKeyClipboard,
            AuthMethod::ManagementKeyEnvironment,
        ],
        #[cfg(feature = "edition-pro")]
        Provider::Higgsfield => &[AuthMethod::Cli],
    };
    AuthCapability { methods }
}
