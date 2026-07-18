use super::*;
use tempfile::TempDir;

#[test]
fn cli_profile_rate_limited_failure_is_assembled_as_rate_limited() {
    let account = usage_core::account::Account {
        id: "claude-cli".into(),
        provider: usage_core::account::Provider::Claude,
        label: "user@example.com".into(),
        auth_source: usage_core::account::AuthSource::CliProfile {
            profile_root: std::path::PathBuf::from("/profile"),
            ownership: usage_core::account::ProfileOwnership::External,
            expected_identity: "user@example.com".into(),
        },
    };
    let local = LocalUsage::none(usage_core::models::LocalProvenance::NoLocalProfile);

    let usage = assemble_cli_profile_usage(
        &account,
        CliProfileOutcome::Live(FetchOutcome::Failed { status: Some(429) }),
        local,
    );

    assert_eq!(usage.status, "rate_limited");
    assert_eq!(usage.five_hour, None);
    assert_eq!(usage.week, None);
}

#[test]
fn auth_source_claude_snapshot_missing_is_waiting() {
    use std::path::Path;
    assert!(matches!(
        read_claude_snapshot_outcome(Path::new("/nonexistent"), "id"),
        CliProfileOutcome::WaitingForUsage
    ));
}

#[test]
fn auth_source_claude_snapshot_live() {
    let temp = TempDir::new().expect("create temp directory");
    let snapshot = temp.path().join("snapshot.json");
    std::fs::write(
        &snapshot,
        r#"{"identity":"id","rate_limits":{"five_hour":{"utilization":30.0},"seven_day":{"utilization":55.0}}}"#,
    )
    .expect("write snapshot");

    let CliProfileOutcome::Live(FetchOutcome::Live {
        five_hour,
        week,
        plan,
        email,
    }) = read_claude_snapshot_outcome(&snapshot, "id")
    else {
        panic!("expected live Claude snapshot outcome");
    };

    assert_eq!(five_hour.map(|quota| quota.percent), Some(30.0));
    assert_eq!(week.map(|quota| quota.percent), Some(55.0));
    assert_eq!(plan, None);
    assert_eq!(email.as_deref(), Some("id"));
}

#[tokio::test]
async fn claude_cli_profile_falls_back_to_snapshot_without_profile_credentials() {
    let temp = TempDir::new().expect("create temp directory");
    let profile_root = temp.path().join("profile");
    std::fs::create_dir(&profile_root).expect("create empty profile directory");
    let snapshot = temp.path().join("snapshot.json");
    std::fs::write(
        &snapshot,
        r#"{"identity":"id","rate_limits":{"five_hour":{"utilization":30.0},"seven_day":{"utilization":55.0}}}"#,
    )
    .expect("write snapshot");

    let client = reqwest::Client::new();
    assert!(matches!(
        poll_claude_cli_profile(&client, &profile_root, "id", &snapshot).await,
        CliProfileOutcome::Live(FetchOutcome::Live { .. })
    ));
    assert!(matches!(
        poll_claude_cli_profile(
            &client,
            &profile_root,
            "id",
            &temp.path().join("missing.json"),
        )
        .await,
        CliProfileOutcome::WaitingForUsage
    ));
}

#[test]
fn auth_source_claude_snapshot_identity_mismatch() {
    let temp = TempDir::new().expect("create temp directory");
    let snapshot = temp.path().join("snapshot.json");
    std::fs::write(
        &snapshot,
        r#"{"identity":"other","rate_limits":{"five_hour":{"utilization":30.0}}}"#,
    )
    .expect("write snapshot");

    assert!(matches!(
        read_claude_snapshot_outcome(&snapshot, "id"),
        CliProfileOutcome::IdentityChanged
    ));
}
