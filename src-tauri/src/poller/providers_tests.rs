use super::*;
use tempfile::TempDir;
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

