use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Codex,
    Claude,
    Agy,
    #[cfg(feature = "edition-pro")]
    Cursor,
    #[cfg(feature = "edition-pro")]
    Grok,
    #[cfg(feature = "edition-pro")]
    Higgsfield,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Codex => "codex",
            Provider::Claude => "claude",
            Provider::Agy => "agy",
            #[cfg(feature = "edition-pro")]
            Provider::Cursor => "cursor",
            #[cfg(feature = "edition-pro")]
            Provider::Grok => "grok",
            #[cfg(feature = "edition-pro")]
            Provider::Higgsfield => "higgsfield",
        }
    }
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Provider> {
        match s {
            "codex" => Some(Provider::Codex),
            "claude" => Some(Provider::Claude),
            "agy" => Some(Provider::Agy),
            #[cfg(feature = "edition-pro")]
            "cursor" => Some(Provider::Cursor),
            #[cfg(feature = "edition-pro")]
            "grok" => Some(Provider::Grok),
            #[cfg(feature = "edition-pro")]
            "higgsfield" => Some(Provider::Higgsfield),
            _ => None,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Provider::Codex => "Codex",
            Provider::Claude => "Claude",
            Provider::Agy => "Antigravity (agy)",
            #[cfg(feature = "edition-pro")]
            Provider::Cursor => "Cursor",
            #[cfg(feature = "edition-pro")]
            Provider::Grok => "xAI API credits",
            #[cfg(feature = "edition-pro")]
            Provider::Higgsfield => "Higgsfield",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub provider: Provider,
    pub label: String,
    pub auth_source: AuthSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ProfileOwnership {
    External,
    Managed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthSource {
    CliProfile {
        profile_root: PathBuf,
        ownership: ProfileOwnership,
        expected_identity: String,
    },
    BrowserOAuth {
        credential_id: String,
    },
    CursorDatabase {
        database_path: PathBuf,
        expected_identity: String,
    },
    XaiManagement {
        credential_id: String,
        team_id: String,
    },
    HiggsfieldCli {
        expected_identity: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Credentials {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub account_id: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{auth_capability, AuthMethod};

    fn assert_account_json_round_trip(provider: Provider, auth_source: AuthSource) {
        let account = Account {
            id: "account-1".into(),
            provider,
            label: "user@example.com".into(),
            auth_source,
        };
        let json = serde_json::to_string(&account).unwrap();
        let decoded: Account = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, account);
    }

    #[test]
    fn provider_roundtrips_lowercase() {
        assert_eq!(Provider::from_str("codex"), Some(Provider::Codex));
        assert_eq!(Provider::Agy.as_str(), "agy");
        let j = serde_json::to_string(&Provider::Claude).unwrap();
        assert_eq!(j, "\"claude\"");
    }

    #[test]
    fn capabilities_match_supported_auth_methods() {
        assert_eq!(
            auth_capability(Provider::Codex).methods,
            &[AuthMethod::Cli, AuthMethod::BrowserOAuth]
        );
        assert_eq!(
            auth_capability(Provider::Claude).methods,
            &[AuthMethod::Cli, AuthMethod::BrowserOAuth]
        );
        assert_eq!(
            auth_capability(Provider::Agy).methods,
            &[AuthMethod::BrowserOAuth]
        );
        #[cfg(feature = "edition-pro")]
        assert_eq!(
            auth_capability(Provider::Cursor).methods,
            &[AuthMethod::LocalDatabase]
        );
        #[cfg(feature = "edition-pro")]
        assert_eq!(
            auth_capability(Provider::Grok).methods,
            &[
                AuthMethod::ManagementKeyClipboard,
                AuthMethod::ManagementKeyEnvironment,
            ]
        );
        #[cfg(feature = "edition-pro")]
        assert_eq!(
            auth_capability(Provider::Higgsfield).methods,
            &[AuthMethod::Cli]
        );
    }

    #[test]
    fn cli_profile_account_round_trips_json() {
        assert_account_json_round_trip(
            Provider::Codex,
            AuthSource::CliProfile {
                profile_root: PathBuf::from("/profiles/codex-work"),
                ownership: ProfileOwnership::Managed,
                expected_identity: "user@example.com".into(),
            },
        );
    }

    #[test]
    fn browser_oauth_account_round_trips_json() {
        assert_account_json_round_trip(
            Provider::Agy,
            AuthSource::BrowserOAuth {
                credential_id: "agy-credential".into(),
            },
        );
    }

    #[cfg(feature = "edition-pro")]
    #[test]
    fn cursor_database_account_round_trips_json() {
        assert_account_json_round_trip(
            Provider::Cursor,
            AuthSource::CursorDatabase {
                database_path: PathBuf::from("/profiles/cursor/state.vscdb"),
                expected_identity: "user@example.com".into(),
            },
        );
    }

    #[cfg(feature = "edition-pro")]
    #[test]
    fn xai_management_account_round_trips_json() {
        assert_account_json_round_trip(
            Provider::Grok,
            AuthSource::XaiManagement {
                credential_id: "xai-credential".into(),
                team_id: "team-1".into(),
            },
        );
    }

    #[cfg(feature = "edition-pro")]
    #[test]
    fn higgsfield_cli_account_round_trips_json() {
        assert_account_json_round_trip(
            Provider::Higgsfield,
            AuthSource::HiggsfieldCli {
                expected_identity: "user@example.com".into(),
            },
        );
    }

    #[cfg(feature = "edition-pro")]
    #[test]
    fn grok_display_name_identifies_xai_api_credits() {
        assert_eq!(Provider::Grok.display_name(), "xAI API credits");
    }
}
