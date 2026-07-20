use super::*;
use crate::api::{AccountUsageDto, TokenTotalsDto, UsageResponse};

fn dto(provider: Provider, id: &str, auth_kind: &'static str) -> AccountUsageDto {
    AccountUsageDto {
        id: id.into(),
        provider,
        auth_kind,
        display_name: format!("{id}@example.com"),
        plan: None,
        status: "ok".into(),
        five_hour: None,
        week: None,
        pools: Vec::new(),
        token_totals: TokenTotalsDto {
            five_hours: 0,
            week: 0,
            month: 0,
        },
        local_status: None,
        detail_suffix: None,
    }
}

#[test]
fn auth_kind_labels_are_stable_and_non_secret() {
    let cli = AuthSource::CliProfile {
        profile_root: "/p".into(),
        ownership: usage_core::account::ProfileOwnership::External,
        expected_identity: "id".into(),
    };
    assert_eq!(auth_kind(&cli), "cli_profile");
    let oauth = AuthSource::BrowserOAuth {
        credential_id: "cred".into(),
    };
    assert_eq!(auth_kind(&oauth), "browser_oauth");
}

#[test]
fn pro_auth_kind_labels_avoid_leak_denylist() {
    let xai = AuthSource::XaiManagement {
        credential_id: "cred".into(),
        team_id: "team".into(),
    };
    // Must NOT be "xai_management" — "management" is on the leak denylist.
    assert_eq!(auth_kind(&xai), "xai_key");
    let cursor = AuthSource::CursorDatabase {
        database_path: "/db".into(),
        expected_identity: "id".into(),
    };
    assert_eq!(auth_kind(&cursor), "cursor_database");
    let hf = AuthSource::HiggsfieldCli {
        expected_identity: "id".into(),
    };
    assert_eq!(auth_kind(&hf), "higgsfield_cli");
}

#[test]
fn accounts_response_projects_inventory() {
    let resp = UsageResponse {
        updated_at: None,
        count: 2,
        accounts: vec![
            dto(Provider::Codex, "a", "cli_profile"),
            dto(Provider::Agy, "b", "browser_oauth"),
        ],
    };
    let out = accounts_response(&resp);
    let json = serde_json::to_string(&out).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["count"], 2);
    assert_eq!(v["accounts"][0]["provider"], "codex");
    assert_eq!(v["accounts"][0]["auth_kind"], "cli_profile");
    assert_eq!(v["accounts"][1]["auth_kind"], "browser_oauth");
    // Inventory never carries secrets or the heavy usage payload.
    for forbidden in ["access_token", "credential_id", "profile_root", "five_hour"] {
        assert!(!json.contains(forbidden), "leaked {forbidden}");
    }
}
