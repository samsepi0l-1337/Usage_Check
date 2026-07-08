use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Codex,
    Claude,
    Agy,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Codex => "codex",
            Provider::Claude => "claude",
            Provider::Agy => "agy",
        }
    }
    pub fn from_str(s: &str) -> Option<Provider> {
        match s {
            "codex" => Some(Provider::Codex),
            "claude" => Some(Provider::Claude),
            "agy" => Some(Provider::Agy),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub provider: Provider,
    pub label: String,
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

    #[test]
    fn provider_roundtrips_lowercase() {
        assert_eq!(Provider::from_str("codex"), Some(Provider::Codex));
        assert_eq!(Provider::Agy.as_str(), "agy");
        let j = serde_json::to_string(&Provider::Claude).unwrap();
        assert_eq!(j, "\"claude\"");
    }
}
