use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::SigningKey;
use std::ffi::OsString;

fn test_signing_key() -> SigningKey {
    SigningKey::from_bytes(&[42u8; 32])
}

fn other_signing_key() -> SigningKey {
    SigningKey::from_bytes(&[99u8; 32])
}

/// Well-formed (64 lowercase hex chars) test device ids — the pure
/// `decide_status` branch table below now enforces that format (item 2 fix:
/// `is_valid_device_id_format`), so a non-hex placeholder string would be
/// rejected as malformed before ever reaching the equality check these
/// tests actually mean to exercise.
const THIS_DEVICE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER_DEVICE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// Test-only shorthand: a watermark whose `max_seen` and `source_issued_at`
/// are the same value. Fine for every test that only cares about the
/// rollback check (`is_rolled_back`, keyed off `max_seen` vs `now`) — the F2
/// regression tests below construct a [`watermark::WatermarkRecord`]
/// directly instead, since they need the two fields to differ.
fn wm(max_seen: DateTime<Utc>) -> watermark::WatermarkRecord {
    watermark::WatermarkRecord {
        max_seen,
        source_issued_at: max_seen,
    }
}

#[allow(clippy::too_many_arguments)]
fn make_record(
    signing_key: &SigningKey,
    plan: &str,
    device: &str,
    issued_at: DateTime<Utc>,
    verified_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
) -> LicenseRecord {
    let payload = token::TokenPayload {
        v: 1,
        key_id: "key-1".into(),
        plan: plan.into(),
        device: device.into(),
        issued_at,
        expires_at,
    };
    LicenseRecord {
        token: token::encode_token(&payload, signing_key),
        key: "TEST-KEY".into(),
        verified_at,
    }
}

// ---------------------------------------------------------------------
// decide_status: pure branch table.
// ---------------------------------------------------------------------

// H2: with NO record present, a missing watermark stays benign (first run
// must not error, and must never read as GracePeriodEnded) — the fail-closed
// behavior in `missing_watermark_fails_closed_when_a_record_is_present`
// below applies ONLY once a record exists to protect.
#[test]
fn no_record_is_free() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    assert_eq!(
        decide_status(None, now, THIS_DEVICE, None, Some(&pk)),
        LicenseStatus::Free
    );
}

#[test]
fn no_public_key_is_free_even_with_a_valid_record() {
    let signing_key = test_signing_key();
    let now = Utc::now();
    let r = make_record(&signing_key, "pro", THIS_DEVICE, now - Duration::days(1), now, None);
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, None, None),
        LicenseStatus::Free
    );
}

#[test]
fn tampered_token_is_free() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let mut r = make_record(&signing_key, "pro", THIS_DEVICE, now - Duration::days(1), now, None);
    r.token = token::tamper_payload(&r.token, |v| {
        v["device"] = serde_json::Value::String(THIS_DEVICE.into());
    });
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, None, Some(&pk)),
        LicenseStatus::Free
    );
}

#[test]
fn token_signed_by_wrong_key_is_free() {
    let signing_key = test_signing_key();
    let wrong_pk = other_signing_key().verifying_key();
    let now = Utc::now();
    let r = make_record(&signing_key, "pro", THIS_DEVICE, now - Duration::days(1), now, None);
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, None, Some(&wrong_pk)),
        LicenseStatus::Free
    );
}

#[test]
fn malformed_token_string_is_free() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let mut r = make_record(&signing_key, "pro", THIS_DEVICE, now - Duration::days(1), now, None);
    r.token = "not-a-token".into();
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, None, Some(&pk)),
        LicenseStatus::Free
    );
}

#[test]
fn wrong_version_is_free() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let payload = token::TokenPayload {
        v: 2,
        key_id: "key-1".into(),
        plan: "pro".into(),
        device: THIS_DEVICE.into(),
        issued_at: now - Duration::days(1),
        expires_at: None,
    };
    let r = LicenseRecord {
        token: token::encode_token(&payload, &signing_key),
        key: "TEST-KEY".into(),
        verified_at: now,
    };
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, None, Some(&pk)),
        LicenseStatus::Free
    );
}

#[test]
fn wrong_plan_is_free() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let r = make_record(&signing_key, "free", THIS_DEVICE, now, now, None);
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, None, Some(&pk)),
        LicenseStatus::Free
    );
}

#[test]
fn wrong_device_is_free() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let r = make_record(&signing_key, "pro", OTHER_DEVICE, now, now, None);
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, None, Some(&pk)),
        LicenseStatus::Free
    );
}

#[test]
fn expired_license_is_expired() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let r = make_record(&signing_key, "pro", THIS_DEVICE, now - Duration::days(2), now, Some(now - Duration::days(1)));
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, Some(wm(now)), Some(&pk)),
        LicenseStatus::Expired
    );
}

#[test]
fn expiry_exactly_at_now_is_expired() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let r = make_record(&signing_key, "pro", THIS_DEVICE, now - Duration::days(1), now, Some(now));
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, Some(wm(now)), Some(&pk)),
        LicenseStatus::Expired
    );
}

#[test]
fn stale_verification_beyond_grace_ends_grace_period() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let r = make_record(
        &signing_key,
        "pro",
        THIS_DEVICE,
        now - OFFLINE_GRACE - Duration::days(2),
        now - OFFLINE_GRACE - Duration::hours(1),
        None,
    );
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, Some(wm(now)), Some(&pk)),
        LicenseStatus::GracePeriodEnded
    );
}

#[test]
fn stale_verification_beyond_grace_ends_grace_period_even_with_future_expiry() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let r = make_record(
        &signing_key,
        "pro",
        THIS_DEVICE,
        now - OFFLINE_GRACE - Duration::days(2),
        now - OFFLINE_GRACE - Duration::hours(1),
        Some(now + Duration::days(365)),
    );
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, Some(wm(now)), Some(&pk)),
        LicenseStatus::GracePeriodEnded
    );
}

#[test]
fn valid_license_within_grace_is_pro() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let r = make_record(&signing_key, "pro", THIS_DEVICE, now - Duration::days(1), now - Duration::days(1), None);
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, Some(wm(now)), Some(&pk)),
        LicenseStatus::Pro { expires_at: None }
    );
}

#[test]
fn valid_license_with_future_expiry_is_pro() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let expires = now + Duration::days(30);
    let r = make_record(&signing_key, "pro", THIS_DEVICE, now, now, Some(expires));
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, Some(wm(now)), Some(&pk)),
        LicenseStatus::Pro {
            expires_at: Some(expires)
        }
    );
}

// H1: `verified_at` is plain, unsigned JSON and must have ZERO influence on
// `decide_status` — freshness is anchored to the token's own SIGNED
// `issued_at`. A record whose `issued_at` is well beyond `OFFLINE_GRACE`
// must never read as Pro just because `verified_at` was (hand-)edited to
// look fresh.
#[test]
fn editing_verified_at_does_not_extend_entitlement_past_issued_at_grace() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let r = make_record(
        &signing_key,
        "pro",
        THIS_DEVICE,
        now - OFFLINE_GRACE - Duration::days(5), // issued_at: well beyond grace.
        now,                                     // verified_at: "freshly edited".
        None,
    );
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, Some(wm(now)), Some(&pk)),
        LicenseStatus::GracePeriodEnded,
        "a hand-edited verified_at must never extend entitlement past the signed issued_at grace"
    );
}

#[test]
fn issued_at_beyond_grace_is_not_pro_even_with_fresh_verified_at() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let issued_at = now - OFFLINE_GRACE - Duration::seconds(1);
    let r = make_record(&signing_key, "pro", THIS_DEVICE, issued_at, now, None);
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, Some(wm(now)), Some(&pk)),
        LicenseStatus::GracePeriodEnded
    );
}

#[test]
fn issued_at_exactly_at_grace_boundary_is_still_pro() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let issued_at = now - OFFLINE_GRACE; // exactly at the boundary: `>`, not `>=`.
    let r = make_record(&signing_key, "pro", THIS_DEVICE, issued_at, now, None);
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, Some(wm(now)), Some(&pk)),
        LicenseStatus::Pro { expires_at: None }
    );
}

// H2: a missing clock-rollback watermark must fail CLOSED whenever a license
// record is present — never silently treated as "no rollback detected".
#[test]
fn missing_watermark_fails_closed_when_a_record_is_present() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let r = make_record(&signing_key, "pro", THIS_DEVICE, now - Duration::days(1), now, None);
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, None, Some(&pk)),
        LicenseStatus::GracePeriodEnded,
        "H2: missing/corrupt/unwritable watermark must be treated as offline grace exhausted, not benign"
    );
}

#[test]
fn future_issued_at_is_not_pro() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let r = make_record(&signing_key, "pro", THIS_DEVICE, now + Duration::hours(1), now, None);
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, None, Some(&pk)),
        LicenseStatus::Free
    );
}

// B0.1: clock rollback.
#[test]
fn rollback_beyond_skew_forces_grace_period_ended_even_within_naive_grace() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    // verified_at is recent (well within OFFLINE_GRACE by naive comparison).
    let r = make_record(&signing_key, "pro", THIS_DEVICE, now - Duration::days(1), now - Duration::hours(1), None);
    let watermark = now + Duration::days(10); // clock was rolled back hard.
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, Some(wm(watermark)), Some(&pk)),
        LicenseStatus::GracePeriodEnded,
        "a detected rollback must force re-verification, never silently extend Pro"
    );
}

#[test]
fn rollback_beyond_skew_prevents_unexpiring_an_expired_license() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let r = make_record(
        &signing_key,
        "pro",
        THIS_DEVICE,
        now - Duration::days(2),
        now - Duration::days(1),
        Some(now - Duration::hours(1)), // already expired at `now`.
    );
    // Rolling the clock back far enough could otherwise make `expires_at <=
    // now` read as false. Rollback detection must still gate first.
    let watermark = now + Duration::days(10);
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, Some(wm(watermark)), Some(&pk)),
        LicenseStatus::GracePeriodEnded
    );
}

#[test]
fn rollback_within_skew_tolerance_does_not_affect_a_valid_license() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let r = make_record(&signing_key, "pro", THIS_DEVICE, now - Duration::days(1), now - Duration::hours(1), None);
    let watermark = now + Duration::minutes(2); // within ROLLBACK_SKEW.
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, Some(wm(watermark)), Some(&pk)),
        LicenseStatus::Pro { expires_at: None }
    );
}

// F2: a watermark whose `max_seen` predates the CURRENT token's own
// `issued_at` is tampering — a legitimate watermark can never fall behind
// the `issued_at` of the token it is evaluated against.
#[test]
fn watermark_max_seen_earlier_than_current_token_issued_at_is_tampering() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let issued_at = now - Duration::hours(1);
    let r = make_record(&signing_key, "pro", THIS_DEVICE, issued_at, now, None);
    // A VALID (parseable, not corrupt) watermark record, but one whose
    // `max_seen` is well before this token's own signed `issued_at` — only
    // possible if the file was hand-edited after the token was issued.
    let regressed_watermark = watermark::WatermarkRecord {
        max_seen: issued_at - Duration::days(1),
        source_issued_at: issued_at - Duration::days(1),
    };
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, Some(regressed_watermark), Some(&pk)),
        LicenseStatus::GracePeriodEnded,
        "a watermark predating the current token's issued_at must be treated as tampering"
    );
}

#[test]
fn watermark_max_seen_exactly_at_current_token_issued_at_is_not_tampering() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let issued_at = now - Duration::hours(1);
    let r = make_record(&signing_key, "pro", THIS_DEVICE, issued_at, now, None);
    let boundary_watermark = watermark::WatermarkRecord {
        max_seen: issued_at, // exactly equal: `<`, not `<=`.
        source_issued_at: issued_at,
    };
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, Some(boundary_watermark), Some(&pk)),
        LicenseStatus::Pro { expires_at: None }
    );
}

// ---------------------------------------------------------------------
// item 2 fix: an absent device id must never be able to match a token, and
// the 64-hex-lowercase format invariant is enforced on BOTH sides.
// ---------------------------------------------------------------------

// The exact vulnerability: before this fix, `status_in` fell back to `""`
// for an absent local device id, and `decide_status` compared that directly
// against `payload.device` — so a validly-signed token whose own `device`
// field was ALSO `""` would match and reach `Pro`.
#[test]
fn a_signed_token_with_an_empty_device_field_never_reaches_pro_even_with_an_empty_local_id() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let r = make_record(&signing_key, "pro", "", now - Duration::days(1), now, None);
    assert_eq!(
        decide_status(Some(&r), now, "", Some(wm(now)), Some(&pk)),
        LicenseStatus::Free,
        "an empty-string device id standing in for 'absent' must never match a token whose own \
         device field is also empty"
    );
}

#[test]
fn a_token_device_that_is_not_64_hex_never_reaches_pro_even_when_it_matches_exactly() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();

    // Too short.
    let too_short = "a".repeat(63);
    let r = make_record(&signing_key, "pro", &too_short, now - Duration::days(1), now, None);
    assert_eq!(
        decide_status(Some(&r), now, &too_short, Some(wm(now)), Some(&pk)),
        LicenseStatus::Free,
        "a device id shorter than 64 hex chars must never reach Pro"
    );

    // Uppercase hex.
    let uppercase = "A".repeat(64);
    let r = make_record(&signing_key, "pro", &uppercase, now - Duration::days(1), now, None);
    assert_eq!(
        decide_status(Some(&r), now, &uppercase, Some(wm(now)), Some(&pk)),
        LicenseStatus::Free,
        "uppercase hex must never reach Pro"
    );

    // Non-hex characters.
    let non_hex = "g".repeat(64);
    let r = make_record(&signing_key, "pro", &non_hex, now - Duration::days(1), now, None);
    assert_eq!(
        decide_status(Some(&r), now, &non_hex, Some(wm(now)), Some(&pk)),
        LicenseStatus::Free,
        "non-hex characters must never reach Pro"
    );
}

#[test]
fn a_matching_well_formed_64_hex_device_id_still_reaches_pro() {
    let signing_key = test_signing_key();
    let pk = signing_key.verifying_key();
    let now = Utc::now();
    let r = make_record(&signing_key, "pro", THIS_DEVICE, now - Duration::days(1), now, None);
    assert_eq!(
        decide_status(Some(&r), now, THIS_DEVICE, Some(wm(now)), Some(&pk)),
        LicenseStatus::Pro { expires_at: None },
        "a well-formed, matching 64-hex device id must still reach Pro — the format invariant \
         must not itself break the ordinary matching case"
    );
}

// ---------------------------------------------------------------------
// status_in / is_pro_in: the impure wrapper, against a filesystem-isolated
// tempdir AND a debug-only pubkey env override (so the real global
// `resolve_public_key()` path is genuinely exercised end to end).
// ---------------------------------------------------------------------

use super::LICENSE_ENV_LOCK as ENV_LOCK;
const PUBKEY_ENV: &str = "USAGECHECK_LICENSE_PUBKEY";

struct PubkeyEnvGuard(Option<OsString>);

impl PubkeyEnvGuard {
    fn set(signing_key: &SigningKey) -> Self {
        let previous = std::env::var_os(PUBKEY_ENV);
        let b64 = STANDARD.encode(signing_key.verifying_key().to_bytes());
        std::env::set_var(PUBKEY_ENV, b64);
        Self(previous)
    }
}

impl Drop for PubkeyEnvGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(previous) => std::env::set_var(PUBKEY_ENV, previous),
            None => std::env::remove_var(PUBKEY_ENV),
        }
    }
}

#[test]
fn status_in_with_no_app_data_dir_is_free() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    assert_eq!(status_in(None), LicenseStatus::Free);
}

// (B) A status/read evaluation must be genuinely side-effect free: no
// device-id file, no license.json, no watermark — nothing at all, even on
// a completely fresh app-data directory with nothing pre-existing.
#[test]
fn status_in_on_a_fresh_app_data_dir_creates_no_files_and_is_non_pro() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let before: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(before.is_empty(), "sanity: tempdir should start empty");

    let status = status_in(Some(tmp.path()));
    assert!(
        !matches!(status, LicenseStatus::Pro { .. }),
        "a fresh app-data dir with nothing on disk must never read as Pro, got {status:?}"
    );

    let after: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        before, after,
        "status_in must create NO files at all — device id minting/persisting is exclusive to \
         the activation path"
    );
}

#[test]
fn is_pro_matches_only_the_pro_variant() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _env = PubkeyEnvGuard::set(&signing_key);

    let tmp = tempfile::tempdir().unwrap();
    assert!(!is_pro_in(Some(tmp.path())), "no record on disk is not Pro");

    let device = device::device_id_in(Some(tmp.path()));
    let now = Utc::now();

    let expired = make_record(&signing_key, "pro", &device, now - Duration::days(2), now, Some(now - Duration::days(1)));
    std::fs::write(
        tmp.path().join("license.json"),
        serde_json::to_string_pretty(&expired).unwrap(),
    )
    .unwrap();
    assert!(!is_pro_in(Some(tmp.path())), "expired record is not Pro");

    let grace_ended = make_record(
        &signing_key,
        "pro",
        &device,
        now - OFFLINE_GRACE - Duration::days(2),
        now - OFFLINE_GRACE - Duration::hours(1),
        None,
    );
    std::fs::write(
        tmp.path().join("license.json"),
        serde_json::to_string_pretty(&grace_ended).unwrap(),
    )
    .unwrap();
    assert!(!is_pro_in(Some(tmp.path())), "grace-period-ended record is not Pro");

    let valid_issued_at = now - Duration::days(1);
    let valid = make_record(&signing_key, "pro", &device, valid_issued_at, now, None);
    std::fs::write(
        tmp.path().join("license.json"),
        serde_json::to_string_pretty(&valid).unwrap(),
    )
    .unwrap();
    // F1: a read/status path never creates or repairs the watermark, so — as
    // with a real prior successful activation/refresh — one must already be
    // on disk for a valid record to read as Pro.
    watermark::repair_watermark_in(Some(tmp.path()), now, valid_issued_at).expect("repair watermark");
    assert!(is_pro_in(Some(tmp.path())), "valid record is Pro");
}

#[test]
fn status_in_reads_a_valid_record_from_disk_as_pro() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _env = PubkeyEnvGuard::set(&signing_key);

    let tmp = tempfile::tempdir().unwrap();
    let device = device::device_id_in(Some(tmp.path()));
    let now = Utc::now();
    let issued_at = now - Duration::days(1);
    let r = make_record(&signing_key, "pro", &device, issued_at, now, None);
    std::fs::write(
        tmp.path().join("license.json"),
        serde_json::to_string_pretty(&r).unwrap(),
    )
    .unwrap();
    // H2/F1: `status_in` fails closed on a missing watermark whenever a
    // record is present, and never repairs it itself — so establish one
    // first, mirroring what a real prior `activate()`/`refresh()` call
    // would already have written (F3's `repair_watermark_in`).
    watermark::repair_watermark_in(Some(tmp.path()), now, issued_at).expect("repair watermark");

    assert_eq!(status_in(Some(tmp.path())), LicenseStatus::Pro { expires_at: None });
}

// ---------------------------------------------------------------------
// F1: fail-closed must PERSIST across repeated evaluations, not self-heal.
// Before this fix, `status_in` recreated a missing/corrupt watermark on
// every call, so deleting the file produced exactly ONE `GracePeriodEnded`
// evaluation and then Pro again on the very next call.
// ---------------------------------------------------------------------

/// Persists a valid Pro-eligible record AND a matching watermark for `dir`,
/// mirroring a real prior successful activation — the common setup for the
/// F1 tests below, which then go on to delete/corrupt/regress the watermark
/// specifically.
fn persist_valid_pro_record_and_watermark(
    dir: &std::path::Path,
    signing_key: &SigningKey,
) -> DateTime<Utc> {
    let device = device::device_id_in(Some(dir));
    let now = Utc::now();
    let issued_at = now - Duration::days(1);
    let r = make_record(signing_key, "pro", &device, issued_at, now, None);
    std::fs::write(dir.join("license.json"), serde_json::to_string_pretty(&r).unwrap()).unwrap();
    watermark::repair_watermark_in(Some(dir), now, issued_at).expect("repair watermark");
    issued_at
}

#[test]
fn deleting_the_watermark_stays_non_pro_across_repeated_evaluations() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _env = PubkeyEnvGuard::set(&signing_key);
    let tmp = tempfile::tempdir().unwrap();
    persist_valid_pro_record_and_watermark(tmp.path(), &signing_key);
    assert_eq!(status_in(Some(tmp.path())), LicenseStatus::Pro { expires_at: None });

    std::fs::remove_file(tmp.path().join("clock-watermark")).unwrap();

    // Two consecutive evaluations, neither of which may self-heal.
    assert_eq!(
        status_in(Some(tmp.path())),
        LicenseStatus::GracePeriodEnded,
        "first evaluation after deletion must fail closed"
    );
    assert_eq!(
        status_in(Some(tmp.path())),
        LicenseStatus::GracePeriodEnded,
        "a read/status path must never recreate the watermark, so a SECOND \
         evaluation must fail closed too, not silently return to Pro"
    );
    assert!(
        !tmp.path().join("clock-watermark").exists(),
        "status_in must not write the watermark back to disk on a read"
    );
}

#[test]
fn corrupt_watermark_stays_non_pro_across_repeated_evaluations() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _env = PubkeyEnvGuard::set(&signing_key);
    let tmp = tempfile::tempdir().unwrap();
    persist_valid_pro_record_and_watermark(tmp.path(), &signing_key);

    std::fs::write(tmp.path().join("clock-watermark"), "{ not valid json").unwrap();

    assert_eq!(status_in(Some(tmp.path())), LicenseStatus::GracePeriodEnded);
    assert_eq!(
        status_in(Some(tmp.path())),
        LicenseStatus::GracePeriodEnded,
        "a corrupt watermark must not be repaired by a read/status path either"
    );
}

#[test]
fn a_regressed_but_syntactically_valid_watermark_stays_non_pro() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _env = PubkeyEnvGuard::set(&signing_key);
    let tmp = tempfile::tempdir().unwrap();
    let issued_at = persist_valid_pro_record_and_watermark(tmp.path(), &signing_key);

    // Overwrite with a WELL-FORMED watermark record whose `max_seen`
    // predates the stored token's own `issued_at` (F2) — hand-editable
    // without touching the (signed, unforgeable) token itself.
    let regressed = watermark::WatermarkRecord {
        max_seen: issued_at - Duration::days(1),
        source_issued_at: issued_at - Duration::days(1),
    };
    std::fs::write(
        tmp.path().join("clock-watermark"),
        serde_json::to_string(&regressed).unwrap(),
    )
    .unwrap();

    assert_eq!(status_in(Some(tmp.path())), LicenseStatus::GracePeriodEnded);
    assert_eq!(
        status_in(Some(tmp.path())),
        LicenseStatus::GracePeriodEnded,
        "a regressed watermark must not be repaired by a read/status path either"
    );
}

#[test]
fn status_in_ignores_malformed_record_as_free() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("license.json"), "not json").unwrap();
    assert_eq!(status_in(Some(tmp.path())), LicenseStatus::Free);
}

// --- item 3 fix: `has_stored_license_in` must depend on FILE PRESENCE,
// independent of whether the file parses — a malformed record is still
// something the tray must offer a way to remove. The entitlement decision
// itself (`decide_status`/`status_in`, exercised above) is unchanged. ---

#[test]
fn has_stored_license_in_is_true_for_a_malformed_record() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("license.json"), "not json").unwrap();
    assert!(
        has_stored_license_in(Some(tmp.path())),
        "a malformed license.json is still something the user should be able to remove"
    );
    assert_eq!(
        status_in(Some(tmp.path())),
        LicenseStatus::Free,
        "entitlement itself must stay Free for a malformed record"
    );
}

#[test]
fn has_stored_license_in_is_false_when_no_file_exists() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(!has_stored_license_in(Some(tmp.path())));
}

#[test]
fn has_stored_license_in_is_false_when_no_app_data_dir() {
    assert!(!has_stored_license_in(None));
}

#[test]
fn status_in_hand_edited_pro_without_valid_signature_is_free() {
    // The exact forgery Stage B exists to defeat: hand-write a JSON record
    // that LOOKS like an activated Pro license, but whose `token` field is
    // not a token this build can verify at all.
    let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _env = PubkeyEnvGuard::set(&signing_key);

    let tmp = tempfile::tempdir().unwrap();
    let device = device::device_id_in(Some(tmp.path()));
    let forged = LicenseRecord {
        token: "forged.token".into(),
        key: "FREE-KEY".into(),
        verified_at: Utc::now(),
    };
    let _ = device; // documents intent: even the RIGHT device id doesn't help a forged token.
    std::fs::write(
        tmp.path().join("license.json"),
        serde_json::to_string_pretty(&forged).unwrap(),
    )
    .unwrap();

    assert_eq!(status_in(Some(tmp.path())), LicenseStatus::Free);
}

// item 2 fix: `status_in` returns Free IMMEDIATELY when no device id is
// persisted, rather than falling through to `decide_status` with an
// empty-string placeholder. The record on disk here has a token whose own
// `device` field is ALSO empty — the exact shape that, before this fix,
// would have matched an absent local device id and reached Pro.
#[test]
fn status_in_with_no_persisted_device_id_is_free_even_with_a_matching_empty_device_token() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _env = PubkeyEnvGuard::set(&signing_key);

    let tmp = tempfile::tempdir().unwrap();
    let now = Utc::now();
    let issued_at = now - Duration::days(1);
    let r = make_record(&signing_key, "pro", "", issued_at, now, None);
    std::fs::write(
        tmp.path().join("license.json"),
        serde_json::to_string_pretty(&r).unwrap(),
    )
    .unwrap();
    watermark::repair_watermark_in(Some(tmp.path()), now, issued_at).expect("repair watermark");

    // Deliberately no `device-id` file on disk: `device_id_read_only_in`
    // returns `None`.
    assert!(!tmp.path().join("device-id").exists(), "sanity: no device id persisted");
    assert_eq!(status_in(Some(tmp.path())), LicenseStatus::Free);
}
