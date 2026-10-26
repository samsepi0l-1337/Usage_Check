use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::TimeZone;
use serde_json::json;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap()
}

fn jwt_with_exp(exp: i64) -> String {
    let payload = URL_SAFE_NO_PAD.encode(json!({ "exp": exp }).to_string());
    format!("aaa.{payload}.ccc")
}

fn future_jwt() -> String {
    jwt_with_exp(now().timestamp() + 3600)
}

fn write_auth(dir: &Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("auth.json");
    std::fs::write(&path, body).unwrap();
    path
}

fn sample() -> serde_json::Value {
    json!({
        "https://auth.x.ai::client-1": {
            "key": future_jwt(),
            "refresh_token": "refresh-example",
            "email": "user@example.com",
            "user_id": "user-1",
            "team_id": "team-1"
        }
    })
}

#[test]
fn parses_session_with_identity_and_email() {
    let entries = parse_grok_cli_auth_json(&sample(), now());
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert_eq!(e.identity, "user-1");
    assert_eq!(e.email.as_deref(), Some("user@example.com"));
    assert_eq!(e.credentials.refresh_token.as_deref(), Some("refresh-example"));
    assert_eq!(e.credentials.account_id.as_deref(), Some("user-1"));
    assert!(e.credentials.expires_at.is_some());
}

#[test]
fn identity_falls_back_to_email_then_map_key() {
    let root = json!({
        "slot-a": { "access_token": future_jwt(), "email": "a@example.com" },
        "slot-b": { "access_token": future_jwt() }
    });
    let mut ids: Vec<String> = parse_grok_cli_auth_json(&root, now())
        .into_iter()
        .map(|e| e.identity)
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["a@example.com".to_string(), "slot-b".to_string()]);
}

#[test]
fn expired_entries_need_a_refresh_token_to_be_kept() {
    let expired = jwt_with_exp(now().timestamp() - 10);
    let root = json!({
        "gone": { "key": expired.clone() },
        "kept": { "key": expired, "refresh_token": "r" }
    });
    let entries = parse_grok_cli_auth_json(&root, now());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].identity, "kept");
}

#[test]
fn malformed_shapes_yield_no_entries_without_panic() {
    for root in [
        json!(null),
        json!([1, 2, 3]),
        json!("text"),
        json!({ "a": 1, "b": null, "c": [], "d": { "key": 5 }, "e": { "key": "  " } }),
    ] {
        assert!(parse_grok_cli_auth_json(&root, now()).is_empty(), "{root}");
    }
}

#[test]
fn debug_output_never_contains_tokens() {
    let entries = parse_grok_cli_auth_json(&sample(), now());
    let shown = format!("{:?}", entries[0]);
    assert!(!shown.contains("refresh-example"));
    assert!(!shown.contains(&entries[0].credentials.access_token));
}

#[test]
fn load_from_file_maps_entries_to_labelled_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_auth(dir.path(), &sample().to_string());
    let accounts = load_grok_cli_auth_from(&path).unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].label, "user@example.com");
    assert_eq!(accounts[0].credentials.account_id.as_deref(), Some("user-1"));
}

#[test]
fn load_from_file_reports_missing_malformed_and_empty() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("none.json");
    assert!(load_grok_cli_auth_from(&missing).unwrap_err().contains("grok login"));
    let bad = write_auth(dir.path(), "{ not json");
    assert!(load_grok_cli_auth_from(&bad).unwrap_err().contains("not valid JSON"));
    let empty = write_auth(dir.path(), "{}");
    assert!(load_grok_cli_auth_from(&empty).unwrap_err().contains("no usable"));
}

#[test]
fn load_from_file_rejects_oversized_files() {
    let dir = tempfile::tempdir().unwrap();
    let big = format!("{{\"pad\":\"{}\"}}", "x".repeat(2 * 1024 * 1024));
    let path = write_auth(dir.path(), &big);
    assert!(load_grok_cli_auth_from(&path).is_err());
}

#[test]
fn reading_is_strictly_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_auth(dir.path(), &sample().to_string());
    let before = std::fs::read(&path).unwrap();
    let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    load_grok_cli_auth_from(&path).unwrap();
    read_grok_cli_credentials(&path, "user-1", now()).unwrap();
    assert!(read_grok_cli_credentials(&path, "other", now()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), mtime);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn read_credentials_selects_identity_and_types_failures() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_auth(dir.path(), &sample().to_string());
    let creds = read_grok_cli_credentials(&path, "user-1", now()).unwrap();
    assert_eq!(creds.account_id.as_deref(), Some("user-1"));
    assert_eq!(
        read_grok_cli_credentials(&path, "someone-else", now()).unwrap_err(),
        GrokCliReadError::IdentityNotFound
    );
    assert_eq!(
        read_grok_cli_credentials(&dir.path().join("none.json"), "user-1", now()).unwrap_err(),
        GrokCliReadError::Missing
    );
    let bad = write_auth(dir.path(), "[[[");
    assert_eq!(
        read_grok_cli_credentials(&bad, "user-1", now()).unwrap_err(),
        GrokCliReadError::Malformed
    );
    let expired = jwt_with_exp(now().timestamp() - 5);
    let path = write_auth(
        dir.path(),
        &json!({ "s": { "key": expired, "user_id": "u" } }).to_string(),
    );
    // Expired without refresh is filtered at parse time -> identity not found;
    // expired with refresh is returned as Expired (the CLI must renew it).
    assert_eq!(
        read_grok_cli_credentials(&path, "u", now()).unwrap_err(),
        GrokCliReadError::IdentityNotFound
    );
    let path = write_auth(
        dir.path(),
        &json!({ "s": { "key": jwt_with_exp(now().timestamp() - 5), "refresh_token": "r", "user_id": "u" } })
            .to_string(),
    );
    assert_eq!(
        read_grok_cli_credentials(&path, "u", now()).unwrap_err(),
        GrokCliReadError::Expired
    );
}

fn mgmt_key() -> ImportedAccount {
    ImportedAccount {
        label: "Grok · team team-1".into(),
        credentials: Credentials {
            access_token: "mgmt-key-example".into(),
            refresh_token: None,
            account_id: Some("team-1".into()),
            expires_at: None,
        },
    }
}

#[test]
fn management_key_wins_over_cli_session() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_auth(dir.path(), &sample().to_string());
    let chosen = load_grok_auth_from(Ok(mgmt_key()), &path).unwrap();
    assert_eq!(chosen.credentials.access_token, "mgmt-key-example");
}

#[test]
fn cli_session_is_the_fallback_without_management_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_auth(dir.path(), &sample().to_string());
    let chosen = load_grok_auth_from(Err("no key".into()), &path).unwrap();
    assert_eq!(chosen.label, "user@example.com");
    let none = dir.path().join("missing.json");
    let err = load_grok_auth_from(Err("set XAI_MGMT_KEY".into()), &none).unwrap_err();
    assert!(err.contains("XAI_MGMT_KEY") && err.contains("grok login"), "{err}");
}
