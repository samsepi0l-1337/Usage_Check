use super::*;

#[test]
fn is_rolled_back_true_when_now_is_well_before_watermark() {
    let watermark = Utc::now();
    let now = watermark - Duration::hours(1);
    assert!(is_rolled_back(Some(watermark), now));
}

#[test]
fn is_rolled_back_false_within_skew_tolerance() {
    let watermark = Utc::now();
    let now = watermark - Duration::minutes(2);
    assert!(!is_rolled_back(Some(watermark), now));
}

#[test]
fn is_rolled_back_false_exactly_at_skew_boundary() {
    let watermark = Utc::now();
    let now = watermark - ROLLBACK_SKEW;
    assert!(!is_rolled_back(Some(watermark), now), "boundary is inclusive of the tolerance");
}

#[test]
fn is_rolled_back_false_just_past_skew_boundary() {
    let watermark = Utc::now();
    let now = watermark - ROLLBACK_SKEW - Duration::seconds(1);
    assert!(is_rolled_back(Some(watermark), now));
}

#[test]
fn is_rolled_back_false_when_now_advances() {
    let watermark = Utc::now();
    let now = watermark + Duration::days(1);
    assert!(!is_rolled_back(Some(watermark), now));
}

#[test]
fn is_rolled_back_false_with_no_watermark() {
    assert!(!is_rolled_back(None, Utc::now()));
}

#[test]
fn watermark_round_trips_through_disk() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(read_watermark_in(Some(tmp.path())).is_none());

    let now = Utc::now();
    let issued_at = now - Duration::hours(1);
    repair_watermark_in(Some(tmp.path()), now, issued_at).expect("repair watermark");
    let read_back = read_watermark_in(Some(tmp.path())).expect("watermark should be persisted");
    // RFC3339 round-trip is second-precision; compare within a second.
    assert!((read_back.max_seen - now).num_seconds().abs() <= 1);
    assert!((read_back.source_issued_at - issued_at).num_seconds().abs() <= 1);
}

#[test]
fn repair_uses_the_later_of_now_and_source_issued_at() {
    let tmp = tempfile::tempdir().unwrap();
    let now = Utc::now();
    // A source `issued_at` in the future relative to `now` (clock skew
    // between client and server at the moment of repair) must still produce
    // a `max_seen` that is at least that `issued_at` — never one that is
    // itself already "behind" the token it was derived from.
    let future_issued_at = now + Duration::minutes(1);
    repair_watermark_in(Some(tmp.path()), now, future_issued_at).expect("repair watermark");
    let read_back = read_watermark_in(Some(tmp.path())).unwrap();
    assert!((read_back.max_seen - future_issued_at).num_seconds().abs() <= 1);
}

#[test]
fn repair_unconditionally_replaces_a_far_future_watermark() {
    // F3: repair must overwrite whatever was on disk, including a bogus
    // future value — never refuse to advance because the stored watermark
    // already looks "ahead".
    let tmp = tempfile::tempdir().unwrap();
    let now = Utc::now();
    repair_watermark_in(Some(tmp.path()), now + Duration::days(400), now + Duration::days(400))
        .expect("repair watermark");

    let repair_now = Utc::now();
    let issued_at = repair_now - Duration::hours(1);
    repair_watermark_in(Some(tmp.path()), repair_now, issued_at).expect("repair watermark");

    let read_back = read_watermark_in(Some(tmp.path())).unwrap();
    assert!(
        (read_back.max_seen - repair_now).num_seconds().abs() <= 1,
        "repair must replace a far-future watermark, not stay pinned to it"
    );
}

// (item 1 fix) `repair_watermark_in` no longer silently swallows a failure
// — with no app-data directory to write into, it now returns an `Err`
// instead of the previous silent no-op (never a panic either way).
#[test]
fn read_watermark_in_with_no_app_data_dir_is_none_and_repair_fails() {
    assert!(read_watermark_in(None).is_none());
    assert!(repair_watermark_in(None, Utc::now(), Utc::now()).is_err());
}

#[test]
fn watermark_ignores_malformed_file_content() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("clock-watermark"), "not json").unwrap();
    assert!(read_watermark_in(Some(tmp.path())).is_none());
}

// F1: a leftover plain-RFC3339-timestamp watermark from before this
// `{max_seen, source_issued_at}` JSON format existed must fail closed
// (parses as neither valid JSON nor a `WatermarkRecord`), never be
// half-interpreted.
#[test]
fn watermark_ignores_legacy_plain_timestamp_format() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("clock-watermark"), Utc::now().to_rfc3339()).unwrap();
    assert!(read_watermark_in(Some(tmp.path())).is_none());
}

#[cfg(unix)]
#[test]
fn watermark_rejects_a_symlinked_app_data_dir_on_read() {
    use std::os::unix::fs::symlink;

    let real = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let linked_dir = real.path().join("linked");
    symlink(target.path(), &linked_dir).unwrap();

    // Even if a real watermark file exists at the symlink target, reading
    // through the symlinked directory must refuse (B0.2).
    let now = Utc::now();
    repair_watermark_in(Some(target.path()), now, now).expect("repair watermark");

    assert!(read_watermark_in(Some(&linked_dir)).is_none());
}
