use super::*;

#[test]
fn test_read_claude_identity_from_json_prefers_account_uuid_and_email_label() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".claude.json"),
        r#"{"oauthAccount":{"emailAddress":"User@Example.COM","organizationUuid":"org-abc","accountUuid":"acc-1"}}"#,
    )
    .unwrap();

    assert_eq!(
        read_claude_identity_from_json(dir.path()),
        Some((
            "acc-1".to_string(),
            "user@example.com".to_string(),
            "unknown".to_string()
        ))
    );
}

#[test]
fn test_read_claude_identity_from_json_falls_back_to_org_uuid_with_email_label() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".claude.json"),
        r#"{"oauthAccount":{"emailAddress":"  Foo@Bar.COM ","organizationUuid":"org-abc"}}"#,
    )
    .unwrap();

    assert_eq!(
        read_claude_identity_from_json(dir.path()),
        Some((
            "org-abc".to_string(),
            "foo@bar.com".to_string(),
            "unknown".to_string()
        ))
    );
}

#[test]
fn test_read_claude_identity_from_json_falls_back_to_normalized_email() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".claude.json"),
        r#"{"oauthAccount":{"emailAddress":"  Foo@Bar.COM "}}"#,
    )
    .unwrap();

    assert_eq!(
        read_claude_identity_from_json(dir.path()),
        Some((
            "foo@bar.com".to_string(),
            "foo@bar.com".to_string(),
            "unknown".to_string()
        ))
    );
}

#[test]
fn test_read_claude_identity_from_json_returns_none_without_file() {
    let dir = tempfile::tempdir().unwrap();

    assert_eq!(read_claude_identity_from_json(dir.path()), None);
}

#[test]
fn test_read_claude_identity_from_json_returns_none_without_oauth_account() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".claude.json"), r#"{}"#).unwrap();

    assert_eq!(read_claude_identity_from_json(dir.path()), None);
}

#[test]
fn test_parse_claude_status_with_org_id() {
    let json = r#"{"loggedIn":true,"orgId":"org-123","email":"user@example.com","subscriptionType":"pro"}"#;
    let (anchor, label, plan) = parse_claude_status(json).unwrap();
    assert_eq!(anchor, "org-123");
    assert_eq!(label, "user@example.com");
    assert_eq!(plan, "pro");
}

#[test]
fn test_parse_claude_status_fallback_email() {
    let json = r#"{"loggedIn":true,"email":"  User@Example.COM  ","subscriptionType":"free"}"#;
    let (anchor, label, plan) = parse_claude_status(json).unwrap();
    assert_eq!(anchor, "user@example.com");
    assert_eq!(label, "user@example.com");
    assert_eq!(plan, "free");
}

#[test]
fn test_parse_claude_status_not_logged_in() {
    let json = r#"{"loggedIn":false}"#;
    assert!(parse_claude_status(json).is_err());
}

#[test]
fn test_parse_claude_status_missing_identity() {
    let json = r#"{"loggedIn":true,"subscriptionType":"pro"}"#;
    assert!(parse_claude_status(json).is_err());
}

#[test]
fn test_which_claude() {
    let _ = which_claude();
}
