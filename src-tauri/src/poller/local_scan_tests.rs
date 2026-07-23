use super::*;
use chrono::Utc;
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;


/// §6.1 Cap regression: write 300 real .jsonl files, scan them.
/// This test MUST call scan_local_events (real filesystem layer).
/// Guards against reintroducing the old MAX_FILES=200 cap.
#[tokio::test]
async fn test_cap_regression_300_real_files() {
    let dir = tempdir().expect("tempdir");
    let root_path = dir.path().to_path_buf();

    // Create 300 .jsonl files, each with one event
    for i in 0..300 {
        let file_path = root_path.join(format!("event_{:03}.jsonl", i));
        let now = Utc::now();
        let json = serde_json::json!({
            "timestamp": now.to_rfc3339(),
            "model": format!("test-model-{}", i),
            "tokens": 100,
            "dedupe_key": format!("key-{}", i)
        });
        fs::write(&file_path, format!("{}\n", json)).expect("write file");
    }

    let now = Utc::now();
    let result = scan_local_events(Provider::Codex, &[root_path], now).await;

    // Real assertion: all 300 events scanned
    assert_eq!(
        result.events.len(),
        300,
        "Should scan all 300 events (currently stub returns 0)"
    );
    // Provenance should be Ok, NOT Truncated (300 « budget)
    assert!(
        !result.health.truncated,
        "300 events should NOT trigger truncation"
    );
}

/// §6.2 mtime irrelevance: file mtime is 40 days old, event timestamp 1h old.
/// scan_local_events MUST scan by event timestamp, NOT mtime.
/// FAILS on stub, and WOULD FAIL if implementation uses mtime skip.
#[tokio::test]
async fn test_mtime_irrelevance_recent_timestamp() {
    use filetime::FileTime;

    let dir = tempdir().expect("tempdir");
    let root_path = dir.path().to_path_buf();
    let file_path = root_path.join("old_mtime.jsonl");

    let now = Utc::now();
    let event_ts = now - chrono::Duration::hours(1);

    let json = serde_json::json!({
        "timestamp": event_ts.to_rfc3339(),
        "model": "test",
        "tokens": 100,
        "dedupe_key": "test-key"
    });
    fs::write(&file_path, format!("{}\n", json)).expect("write file");

    // Set file mtime to 40 days ago
    let old_time = FileTime::from_system_time(
        std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 24 * 3600),
    );
    filetime::set_file_mtime(&file_path, old_time).expect("set mtime");

    let result = scan_local_events(Provider::Codex, &[root_path], now).await;

    // Real assertion: event must be scanned despite old mtime
    assert_eq!(
        result.events.len(),
        1,
        "Should scan event with 1h-old timestamp, even though file mtime is 40d old"
    );
    assert_eq!(
        result.events[0].tokens, 100,
        "Event tokens should be counted"
    );
}

/// §6.10 Error classification: a scan root that does not exist is NOT an error — e.g. a
/// managed CliProfile legitimately has no `<profile_root>/projects` dir. It should
/// contribute no events and must NOT set root_unreadable (which would surface a false
/// "(local: unavailable)" tray warning).
#[tokio::test]
async fn test_scan_missing_root_is_not_unreadable() {
    let nonexistent = PathBuf::from("/nonexistent/root/path");
    let now = Utc::now();

    let result = scan_local_events(Provider::Codex, &[nonexistent], now).await;

    // Root doesn't exist → this is NOT an error, so root_unreadable must stay false.
    assert!(
        !result.health.root_unreadable,
        "Missing root should NOT set root_unreadable=true"
    );
    // Events should be empty
    assert_eq!(result.events.len(), 0, "Missing root has no events");
    // scan_provenance must classify this as NoEvents, not Unavailable.
    assert_eq!(
        super::scan_provenance(&result),
        usage_core::models::LocalProvenance::NoEvents,
        "Missing root with no events should be NoEvents, not Unavailable"
    );
}

/// §6.10b Error classification: a root that exists but is a regular file (not a
/// directory) is a genuine anomaly and MUST still set root_unreadable → Unavailable.
#[tokio::test]
async fn test_scan_root_is_regular_file_is_unreadable() {
    let dir = tempdir().expect("tempdir");
    let file_path = dir.path().join("not_a_dir.txt");
    fs::write(&file_path, b"hello").expect("write file");
    let now = Utc::now();

    let result = scan_local_events(Provider::Codex, &[file_path], now).await;

    assert!(
        result.health.root_unreadable,
        "A root that is a regular file (not a dir) should set root_unreadable=true"
    );
    assert_eq!(result.events.len(), 0, "Regular-file root has no events");
    assert_eq!(
        super::scan_provenance(&result),
        usage_core::models::LocalProvenance::Unavailable,
        "A regular-file root should be classified Unavailable"
    );
}
