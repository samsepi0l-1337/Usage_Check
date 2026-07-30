use super::*;

#[test]
fn device_id_is_stable_across_calls() {
    let tmp = tempfile::tempdir().unwrap();
    let first = device_id_in(Some(tmp.path()));
    let second = device_id_in(Some(tmp.path()));
    assert_eq!(first, second);
}

#[test]
fn device_id_is_64_char_lowercase_hex() {
    let tmp = tempfile::tempdir().unwrap();
    let id = device_id_in(Some(tmp.path()));
    assert_eq!(id.len(), 64, "expected 64 hex chars, got: {id}");
    assert!(
        id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "expected lowercase hex, got: {id}"
    );
}

#[test]
fn device_id_differs_across_independent_roots() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    // Astronomically unlikely to collide; guards against a constant stub.
    assert_ne!(device_id_in(Some(a.path())), device_id_in(Some(b.path())));
}

// B0.3.
#[test]
fn device_id_with_no_app_data_dir_is_stable_across_calls() {
    // Before the B0.3 fix, `app_data_dir: None` bypassed the cache entirely
    // and minted (and hashed) a brand-new UUID on every single call.
    let first = device_id_in(None);
    let second = device_id_in(None);
    let third = device_id_in(None);
    assert_eq!(first, second);
    assert_eq!(second, third);
}

#[cfg(unix)]
#[test]
fn device_id_persistence_failure_does_not_flap_within_process() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    // Force persistence to fail on every call by making the device-id path
    // itself a symlink: `reject_symlink` refuses it regardless of whether
    // the link target exists, which is a portable way to force a
    // persistence failure (unlike file permission bits, which root ignores).
    symlink("/nonexistent-target", tmp.path().join(DEVICE_ID_FILE)).unwrap();

    let first = device_id_in(Some(tmp.path()));
    let second = device_id_in(Some(tmp.path()));
    assert_eq!(
        first, second,
        "an unwritable device-id path must not flap the device binding within one process"
    );
}

// B0.3: single-flight — recovery persists the CACHED value, never a fresh one.
#[cfg(unix)]
#[test]
fn device_id_persists_the_cached_value_once_the_path_becomes_writable() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    let device_id_path = tmp.path().join(DEVICE_ID_FILE);
    symlink("/nonexistent-target", &device_id_path).unwrap();

    let first = device_id_in(Some(tmp.path()));
    // Nothing was persisted yet — the path is still a symlink.
    assert!(read_persisted_uuid(Some(tmp.path())).is_none());

    std::fs::remove_file(&device_id_path).unwrap();
    let second = device_id_in(Some(tmp.path()));

    assert_eq!(
        first, second,
        "recovery must return the SAME id that was already cached and handed out"
    );
    let persisted_raw =
        read_persisted_uuid(Some(tmp.path())).expect("second call should have persisted it");
    assert_eq!(
        hash_device_uuid(&persisted_raw),
        second,
        "the value written to disk on recovery must be the cached raw uuid, not a freshly minted one"
    );
}

// F5: `device_id_checked_in` reports the durability of the id, distinctly
// from `device_id_in`'s bare string.
#[test]
fn device_id_checked_in_reports_persisted_true_on_a_writable_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let (id, persisted) = device_id_checked_in(Some(tmp.path()));
    assert!(persisted, "a normal writable tempdir should persist the freshly-minted id");
    assert_eq!(id, device_id_in(Some(tmp.path())));
}

#[cfg(unix)]
#[test]
fn device_id_checked_in_reports_persisted_false_when_the_path_is_unwritable() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    symlink("/nonexistent-target", tmp.path().join(DEVICE_ID_FILE)).unwrap();

    let (_, persisted) = device_id_checked_in(Some(tmp.path()));
    assert!(!persisted, "an unwritable device-id path must report persisted=false");
}

#[test]
fn device_id_checked_in_reports_persisted_false_with_no_app_data_dir() {
    // Nothing durable can ever be written when there is no app-data dir at
    // all — unlike the bare id, which stays stable in-process anyway.
    let (_, persisted) = device_id_checked_in(None);
    assert!(!persisted);
}

// B0.3: two concurrent first-time callers for the same directory must
// observe the same id — the single-flight lock, not a racy read-then-write.
#[test]
fn concurrent_first_time_callers_observe_the_same_id() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().to_path_buf();

    let ids: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let path = path.clone();
                scope.spawn(move || device_id_in(Some(&path)))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let first = &ids[0];
    for id in &ids {
        assert_eq!(id, first, "every concurrent first-time caller must agree");
    }
    let persisted_raw = read_persisted_uuid(Some(tmp.path())).expect("id must be persisted");
    assert_eq!(&hash_device_uuid(&persisted_raw), first);
}
