use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Codex,
    Claude,
    Agy,
    Cursor,
    Grok,
    Higgsfield,
    Kimi,
    OpenCode,
    DeepSeek,
    OpenRouter,
    Copilot,
    Windsurf,
    MiniMax,
    Augment,
    Poe,
    Fireworks,
    Novita,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Codex => "codex",
            Provider::Claude => "claude",
            Provider::Agy => "agy",
            Provider::Cursor => "cursor",
            Provider::Grok => "grok",
            Provider::Higgsfield => "higgsfield",
            Provider::Kimi => "kimi",
            Provider::OpenCode => "opencode",
            Provider::DeepSeek => "deepseek",
            Provider::OpenRouter => "openrouter",
            Provider::Copilot => "copilot",
            Provider::Windsurf => "windsurf",
            Provider::MiniMax => "minimax",
            Provider::Augment => "augment",
            Provider::Poe => "poe",
            Provider::Fireworks => "fireworks",
            Provider::Novita => "novita",
        }
    }
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Provider> {
        match s {
            "codex" => Some(Provider::Codex),
            "claude" => Some(Provider::Claude),
            "agy" => Some(Provider::Agy),
            "cursor" => Some(Provider::Cursor),
            "grok" => Some(Provider::Grok),
            "higgsfield" => Some(Provider::Higgsfield),
            "kimi" => Some(Provider::Kimi),
            "opencode" => Some(Provider::OpenCode),
            "deepseek" => Some(Provider::DeepSeek),
            "openrouter" => Some(Provider::OpenRouter),
            "copilot" => Some(Provider::Copilot),
            "windsurf" => Some(Provider::Windsurf),
            "minimax" => Some(Provider::MiniMax),
            "augment" => Some(Provider::Augment),
            "poe" => Some(Provider::Poe),
            "fireworks" => Some(Provider::Fireworks),
            "novita" => Some(Provider::Novita),
            _ => None,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Provider::Codex => "Codex",
            Provider::Claude => "Claude",
            Provider::Agy => "Antigravity (agy)",
            Provider::Cursor => "Cursor",
            Provider::Grok => "xAI API credits",
            Provider::Higgsfield => "Higgsfield",
            Provider::Kimi => "Kimi Code",
            Provider::OpenCode => "OpenCode Go",
            Provider::DeepSeek => "DeepSeek",
            Provider::OpenRouter => "OpenRouter",
            Provider::Copilot => "GitHub Copilot",
            Provider::Windsurf => "Windsurf",
            Provider::MiniMax => "MiniMax",
            Provider::Augment => "Augment",
            Provider::Poe => "Poe",
            Provider::Fireworks => "Fireworks",
            Provider::Novita => "Novita",
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
    WindsurfDatabase {
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
    #[serde(rename = "minimax_cli")]
    MiniMaxCli {
        expected_identity: String,
    },
    AugmentCli {
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
        assert_eq!(
            auth_capability(Provider::Cursor).methods,
            &[AuthMethod::LocalDatabase]
        );
        assert_eq!(
            auth_capability(Provider::Grok).methods,
            &[
                AuthMethod::ManagementKeyClipboard,
                AuthMethod::ManagementKeyEnvironment,
            ]
        );
        assert_eq!(
            auth_capability(Provider::Higgsfield).methods,
            &[AuthMethod::Cli]
        );
        assert_eq!(auth_capability(Provider::Kimi).methods, &[AuthMethod::Cli]);
        assert_eq!(
            auth_capability(Provider::OpenCode).methods,
            &[AuthMethod::Cli]
        );
        assert_eq!(
            auth_capability(Provider::DeepSeek).methods,
            &[AuthMethod::Cli]
        );
        assert_eq!(
            auth_capability(Provider::OpenRouter).methods,
            &[AuthMethod::Cli]
        );
        assert_eq!(
            auth_capability(Provider::Copilot).methods,
            &[AuthMethod::Cli]
        );
        assert_eq!(
            auth_capability(Provider::Windsurf).methods,
            &[AuthMethod::LocalDatabase]
        );
        assert_eq!(
            auth_capability(Provider::MiniMax).methods,
            &[AuthMethod::Cli]
        );
        assert_eq!(
            auth_capability(Provider::Augment).methods,
            &[AuthMethod::Cli]
        );
        assert_eq!(auth_capability(Provider::Poe).methods, &[AuthMethod::Cli]);
        assert_eq!(
            auth_capability(Provider::Fireworks).methods,
            &[AuthMethod::Cli]
        );
        assert_eq!(
            auth_capability(Provider::Novita).methods,
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

    #[test]
    fn windsurf_database_account_round_trips_json() {
        assert_account_json_round_trip(
            Provider::Windsurf,
            AuthSource::WindsurfDatabase {
                database_path: PathBuf::from("/profiles/windsurf/state.vscdb"),
                expected_identity: "user@example.com".into(),
            },
        );
        let json = serde_json::to_value(AuthSource::WindsurfDatabase {
            database_path: PathBuf::from("/profiles/windsurf/state.vscdb"),
            expected_identity: "user@example.com".into(),
        })
        .unwrap();
        assert_eq!(json["kind"], "windsurf_database");
        let cursor = serde_json::to_value(AuthSource::CursorDatabase {
            database_path: PathBuf::from("/profiles/cursor/state.vscdb"),
            expected_identity: "user@example.com".into(),
        })
        .unwrap();
        assert_eq!(cursor["kind"], "cursor_database");
    }

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

    #[test]
    fn higgsfield_cli_account_round_trips_json() {
        assert_account_json_round_trip(
            Provider::Higgsfield,
            AuthSource::HiggsfieldCli {
                expected_identity: "user@example.com".into(),
            },
        );
    }

    #[test]
    fn minimax_cli_account_round_trips_json() {
        assert_account_json_round_trip(
            Provider::MiniMax,
            AuthSource::MiniMaxCli {
                expected_identity: "user@example.com".into(),
            },
        );
        let json = serde_json::to_value(AuthSource::MiniMaxCli {
            expected_identity: "user@example.com".into(),
        })
        .unwrap();
        assert_eq!(json["kind"], "minimax_cli");
    }

    #[test]
    fn augment_cli_account_round_trips_json() {
        assert_account_json_round_trip(
            Provider::Augment,
            AuthSource::AugmentCli {
                expected_identity: "user@example.com".into(),
            },
        );
        let json = serde_json::to_value(AuthSource::AugmentCli {
            expected_identity: "user@example.com".into(),
        })
        .unwrap();
        assert_eq!(json["kind"], "augment_cli");
    }

    #[test]
    fn grok_display_name_identifies_xai_api_credits() {
        assert_eq!(Provider::Grok.display_name(), "xAI API credits");
    }

    #[test]
    fn new_pro_providers_roundtrip_slugs_and_names() {
        assert_eq!(Provider::from_str("kimi"), Some(Provider::Kimi));
        assert_eq!(Provider::Kimi.as_str(), "kimi");
        assert_eq!(Provider::Kimi.display_name(), "Kimi Code");
        assert_eq!(Provider::OpenCode.as_str(), "opencode");
        assert_eq!(Provider::OpenCode.display_name(), "OpenCode Go");
        assert_eq!(Provider::DeepSeek.as_str(), "deepseek");
        assert_eq!(Provider::DeepSeek.display_name(), "DeepSeek");
        assert_eq!(Provider::OpenRouter.as_str(), "openrouter");
        assert_eq!(Provider::OpenRouter.display_name(), "OpenRouter");
        assert_eq!(Provider::from_str("copilot"), Some(Provider::Copilot));
        assert_eq!(Provider::Copilot.as_str(), "copilot");
        assert_eq!(Provider::Copilot.display_name(), "GitHub Copilot");
        assert_eq!(Provider::from_str("windsurf"), Some(Provider::Windsurf));
        assert_eq!(Provider::Windsurf.as_str(), "windsurf");
        assert_eq!(Provider::Windsurf.display_name(), "Windsurf");
        assert_eq!(Provider::from_str("minimax"), Some(Provider::MiniMax));
        assert_eq!(Provider::MiniMax.as_str(), "minimax");
        assert_eq!(Provider::MiniMax.display_name(), "MiniMax");
        assert_eq!(Provider::from_str("augment"), Some(Provider::Augment));
        assert_eq!(Provider::Augment.as_str(), "augment");
        assert_eq!(Provider::Augment.display_name(), "Augment");
        assert_eq!(Provider::from_str("poe"), Some(Provider::Poe));
        assert_eq!(Provider::Poe.as_str(), "poe");
        assert_eq!(Provider::Poe.display_name(), "Poe");
        assert_eq!(Provider::from_str("fireworks"), Some(Provider::Fireworks));
        assert_eq!(Provider::Fireworks.as_str(), "fireworks");
        assert_eq!(Provider::Fireworks.display_name(), "Fireworks");
        assert_eq!(Provider::from_str("novita"), Some(Provider::Novita));
        assert_eq!(Provider::Novita.as_str(), "novita");
        assert_eq!(Provider::Novita.display_name(), "Novita");
    }
}
