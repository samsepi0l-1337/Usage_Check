//! Runtime license gate: a Pro license key unlocks the paid providers
//! (Cursor, Grok, Higgsfield) at runtime — there is no compile-time edition
//! (see `crate::edition`).
//!
//! STAGE B: the server's answer is a signed token (`license/token.rs`), not
//! plain JSON. The app persists the TOKEN verbatim and re-verifies its
//! Ed25519 signature (`license/pubkey.rs`) on EVERY status evaluation, so a
//! hand-edited cache file fails closed instead of granting Pro forever. See
//! `docs/LICENSE_API.md` for the full wire contract this implements.
//!
//! SECURITY: never log/print the license key, the token, or the raw device
//! UUID.

use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::store::{reject_symlink, write_private_file};

mod device;
mod http;
mod pubkey;
mod token;
mod watermark;

pub use http::{ActivationError, ActivationErrorClass};

const LICENSE_FILE: &str = "license.json";

/// How long a Pro activation remains valid without a fresh server
/// verification before it is treated as expired offline.
pub const OFFLINE_GRACE: Duration = Duration::days(14);

/// On-disk Pro activation record, stored at `<app_data>/license.json`.
///
/// Token-centric: this persists exactly what the server signed, not what was
/// derived from it. `plan`/`device`/`issued_at`/`expires_at` are read OUT OF
/// the verified token on every evaluation — never from separate mutable
/// fields a hand-edited file could set independently of the signature.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LicenseRecord {
    /// The signed token exactly as received from the server.
    pub token: String,
    /// The license key as entered (kept for display and for `refresh()`,
    /// which re-activates with this same key).
    pub key: String,
    /// Last successful SERVER verification, as observed by THIS client.
    ///
    /// SECURITY: this field is plain JSON, outside the token's signature —
    /// hand-editing it (e.g. to a fresh timestamp) is trivial and costs an
    /// attacker nothing. It therefore carries **no** authority: it is kept
    /// for DISPLAY/diagnostics only (e.g. a future "last verified" label in
    /// the UI) and has **zero influence** on [`decide_status`]. The
    /// freshness anchor `decide_status` actually uses is the token's own
    /// SIGNED `issued_at` (see [`token::TokenPayload::issued_at`]) — the
    /// only way to advance it is a new signature from the server, which is
    /// exactly what [`refresh`] performs.
    pub verified_at: DateTime<Utc>,
}

/// The runtime license state, derived from the on-disk record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LicenseStatus {
    /// No usable Pro activation: no record, unreadable/malformed record, an
    /// invalid/tampered token signature, wrong payload version/plan, or a
    /// record activated on a different device.
    Free,
    /// A valid, current Pro activation.
    Pro { expires_at: Option<DateTime<Utc>> },
    /// The token's `expires_at` is in the past.
    Expired,
    /// Not expired, but the token's own signed `issued_at` (H1 — never
    /// `verified_at`) is older than [`OFFLINE_GRACE`] — or a clock rollback
    /// was detected, or the rollback watermark is missing/unreadable (H2),
    /// any of which forces this state regardless of the naive
    /// `now - issued_at` gap (see [`decide_status`]).
    GracePeriodEnded,
}

/// True only for a well-formed device id: exactly 64 lowercase hex
/// characters (`0-9a-f`). This is precisely the shape [`device::device_id_in`]
/// / [`device::device_id_read_only_in`] produce (the SHA-256 hex digest of a
/// random UUID) — anything else, including the empty string, is not a value
/// a legitimate device id or a legitimately-signed token's `device` field
/// can ever take, and is therefore always treated as a mismatch by
/// [`decide_status`] rather than compared for equality.
fn is_valid_device_id_format(candidate: &str) -> bool {
    candidate.len() == 64 && candidate.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Pure decision: given the on-disk record (if any), the current time, this
/// machine's device id, the persisted clock-rollback watermark, and the
/// resolved public key, compute the license status. No I/O — the whole
/// branch table is unit-testable without touching the filesystem or the
/// network.
///
/// SECURITY (H1): freshness is anchored to the token's own SIGNED
/// `payload.issued_at`, never to `record.verified_at` — the latter is plain
/// JSON outside the signature and carries zero authority here (see its own
/// doc comment on [`LicenseRecord`]). Hand-editing `verified_at` therefore
/// cannot extend entitlement by even one bit.
///
/// SECURITY (H2): a MISSING clock-rollback watermark fails CLOSED whenever a
/// license record is present — treated exactly like an exhausted offline
/// grace period (`GracePeriodEnded`), not like "no rollback detected". An
/// attacker who deletes the watermark file to defeat rollback detection gets
/// forced back to online re-verification instead of silently regaining Pro.
/// Absence is benign ONLY when there is no record to protect, which is
/// handled earlier, before the watermark is ever consulted. This is
/// permanent, not one-shot (F1): nothing in a read/status path ever
/// recreates or repairs the watermark, so a missing/malformed watermark
/// keeps producing `GracePeriodEnded` on EVERY evaluation until a successful
/// ONLINE verification repairs it (F3, `watermark::repair_watermark_in`,
/// called only from [`activate_or_refresh_in`]).
///
/// SECURITY (F2): a watermark that IS present but whose `max_seen` predates
/// the CURRENT token's own signed `issued_at` is likewise treated as
/// tampering, not as "no rollback detected". A legitimate watermark can
/// never fall behind the `issued_at` of the token it is being evaluated
/// against — every legitimate advance sets `max_seen = max(now, issued_at)`
/// at the moment of a successful online verification of THAT SAME token —
/// so `max_seen < issued_at` is decisive evidence the watermark file was
/// rewritten by hand after the token was issued. This does not make the
/// watermark file unforgeable in general — a local owner who controls both
/// the clock and app-data can still hold `now` and `max_seen` together at a
/// value that never regresses past `issued_at` (see `docs/LICENSE_API.md`
/// §7 for the accepted limits) — it closes specifically the "silently
/// rewind the watermark file alone, leaving the token and the clock
/// untouched" case.
///
/// Order (each step short-circuits to a non-Pro status):
/// 1. verify the token signature (missing record, missing/placeholder public
///    key, or a failed verification all collapse to the same `Free` — never
///    distinguish "tampered" from "absent" to an attacker probing the app).
/// 2. payload `v` must be `1`, `plan` must be `"pro"`.
/// 3. both `this_device_id` and payload `device` must be a well-formed
///    64-character lowercase-hex device id (see [`is_valid_device_id_format`])
///    — anything else, INCLUDING the empty string an absent local device id
///    would otherwise fall back to, is treated as a mismatch and never
///    compared for equality. This closes the case where an absent device id
///    (empty string) could otherwise equal a validly-signed token whose own
///    `device` field also happens to be empty — `status_in` additionally
///    short-circuits to `Free` before ever calling this function when no
///    device id is persisted at all, so this check is belt-and-suspenders on
///    the pure side for any other caller.
/// 4. payload `device` must equal `this_device_id`.
/// 5. `issued_at` in the future → `Free` (an untrustworthy record — a
///    forward-rolled clock at signing time must not read as fresh).
/// 6. a MISSING watermark, a detected clock ROLLBACK (B0.1), or a watermark
///    whose `max_seen` predates this token's `issued_at` (F2) →
///    `GracePeriodEnded`, forcing a fresh online re-verification before Pro
///    can be granted again. Checked BEFORE the expiry check below, so
///    neither can be used to un-expire an already-expired license.
/// 7. `expires_at` present and `<= now` → `Expired`.
/// 8. `now - payload.issued_at > OFFLINE_GRACE` → `GracePeriodEnded`.
/// 9. otherwise → `Pro { expires_at }`.
pub fn decide_status(
    record: Option<&LicenseRecord>,
    now: DateTime<Utc>,
    this_device_id: &str,
    watermark: Option<watermark::WatermarkRecord>,
    public_key: Option<&ed25519_dalek::VerifyingKey>,
) -> LicenseStatus {
    let Some(record) = record else {
        return LicenseStatus::Free;
    };
    let Some(public_key) = public_key else {
        return LicenseStatus::Free;
    };
    let Ok(payload) = token::verify_token(&record.token, public_key) else {
        return LicenseStatus::Free;
    };
    if payload.v != 1 || payload.plan != "pro" {
        return LicenseStatus::Free;
    }
    // Format invariant: an absent local device id (which callers other than
    // `status_in` might still pass through as `""`) must never be able to
    // match a token whose own `device` also happens to be empty, and
    // neither a truncated/uppercase/non-hex value on either side may reach
    // the equality check below — both are treated as an outright mismatch.
    if !is_valid_device_id_format(this_device_id) || !is_valid_device_id_format(&payload.device) {
        return LicenseStatus::Free;
    }
    // An activation copied from another machine does not grant Pro here.
    if payload.device != this_device_id {
        return LicenseStatus::Free;
    }
    if payload.issued_at > now {
        return LicenseStatus::Free;
    }
    // H2: record is confirmed present at this point, so a missing watermark
    // is treated as tampering/loss, not as "nothing to check" — fail closed.
    let Some(watermark) = watermark else {
        return LicenseStatus::GracePeriodEnded;
    };
    if watermark::is_rolled_back(Some(watermark.max_seen), now) {
        return LicenseStatus::GracePeriodEnded;
    }
    // F2: a stored watermark that predates this token's own issued_at can
    // only be the result of hand-editing the watermark file — treat exactly
    // like a rollback.
    if watermark.max_seen < payload.issued_at {
        return LicenseStatus::GracePeriodEnded;
    }
    if let Some(expires_at) = payload.expires_at {
        if expires_at <= now {
            return LicenseStatus::Expired;
        }
    }
    // H1: grace is measured from the SIGNED `issued_at`, never `verified_at`.
    if now - payload.issued_at > OFFLINE_GRACE {
        return LicenseStatus::GracePeriodEnded;
    }
    LicenseStatus::Pro {
        expires_at: payload.expires_at,
    }
}

/// B0.2: rejects a symlinked app-data DIRECTORY (not just a symlinked
/// license file) before reading, mirroring the device-id and watermark read
/// paths.
fn read_record_in(app_data_dir: Option<&Path>) -> Option<LicenseRecord> {
    let dir = app_data_dir?;
    reject_symlink(dir, "app data directory").ok()?;
    let path = dir.join(LICENSE_FILE);
    reject_symlink(&path, "license file").ok()?;
    let json = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&json).ok()
}

fn write_record_in(app_data_dir: Option<&Path>, record: &LicenseRecord) -> Result<(), String> {
    let dir = app_data_dir.ok_or_else(|| "could not resolve app data directory".to_string())?;
    reject_symlink(dir, "app data directory")?;
    let path = dir.join(LICENSE_FILE);
    reject_symlink(&path, "license file")?;
    let json =
        serde_json::to_string_pretty(record).map_err(|e| format!("serialize license record: {e}"))?;
    write_private_file(&path, &json)
}

/// F1: side-effect free — a read/status path must never create or repair
/// the watermark. The watermark is advanced ONLY by
/// [`watermark::repair_watermark_in`], called exclusively from
/// [`activate_or_refresh_in`] on a SUCCESSFUL server-authenticated
/// verification (F3). Reading here therefore only ever reflects the
/// watermark state left by the most recent online verification (or its
/// absence/corruption), never mutates it.
///
/// (Bx) Genuinely read-only end to end: the device id lookup below
/// ([`device::device_id_read_only_in`]) never mints or persists a device id
/// either — unlike [`device::device_id_in`], which is reserved for the
/// activation path, where durability is already a precondition (F5).
///
/// A MISSING device id returns `Free` immediately, before `decide_status` is
/// even called — there is nothing on this machine to match a token's
/// `device` field against. This is deliberately NOT implemented by falling
/// back to the empty string and letting `decide_status`'s device check catch
/// it: a validly-signed token whose own `device` field also happens to be
/// empty would otherwise match an empty-string placeholder for "absent",
/// silently granting Pro. `decide_status`'s own format check (item 2 fix)
/// closes the same hole for any OTHER caller that passes it an empty or
/// malformed device id directly, but this early return is what keeps
/// `status_in` itself from ever constructing that placeholder in the first
/// place.
fn status_in(app_data_dir: Option<&Path>) -> LicenseStatus {
    let Some(this_device_id) = device::device_id_read_only_in(app_data_dir) else {
        return LicenseStatus::Free;
    };
    let record = read_record_in(app_data_dir);
    let now = Utc::now();
    let this_watermark = watermark::read_watermark_in(app_data_dir);
    let public_key = pubkey::resolve_public_key();
    decide_status(
        record.as_ref(),
        now,
        &this_device_id,
        this_watermark,
        public_key.as_ref(),
    )
}

/// Current license status, read from disk and re-verified against the
/// embedded (or debug-override) public key.
pub fn status() -> LicenseStatus {
    status_in(crate::paths::usagecheck_app_data_dir().as_deref())
}

/// Test-only injectable core of [`is_pro`], mirroring `status_in` so
/// `is_pro`'s branch table is unit-testable against a filesystem-isolated
/// tempdir instead of global license state.
#[cfg(test)]
fn is_pro_in(app_data_dir: Option<&Path>) -> bool {
    matches!(status_in(app_data_dir), LicenseStatus::Pro { .. })
}

/// True only when [`status`] is [`LicenseStatus::Pro`].
pub fn is_pro() -> bool {
    matches!(status(), LicenseStatus::Pro { .. })
}

/// True when a license record is persisted on disk, regardless of whether it
/// currently verifies as Pro (e.g. `Expired`/`GracePeriodEnded`/a tampered
/// record all still leave a file to remove). Used by the tray to decide
/// whether to offer "Deactivate license" even when the record isn't
/// currently granting Pro — an expired or broken record is still something
/// the user should be able to clear.
pub fn has_stored_license() -> bool {
    has_stored_license_in(crate::paths::usagecheck_app_data_dir().as_deref())
}

/// (item 3 fix) Deliberately NOT gated on successful JSON parsing, unlike
/// the previous `read_record_in(app_data_dir).is_some()` implementation: a
/// malformed `license.json` (`decide_status`/`status_in` already correctly
/// reads it as [`LicenseStatus::Free`] via `read_record_in`'s own parse
/// failure — that entitlement decision is untouched by this fix) must still
/// be reported as "something to remove", or `should_show_deactivate` hides
/// the only row that can clear it, leaving the user with no way to fix a
/// broken file short of finding it on disk manually. Mirrors
/// `read_record_in`'s symlink rejection so a symlinked path is never
/// reported as removable — `deactivate_in` would refuse to touch it either.
fn has_stored_license_in(app_data_dir: Option<&Path>) -> bool {
    let Some(dir) = app_data_dir else {
        return false;
    };
    if reject_symlink(dir, "app data directory").is_err() {
        return false;
    }
    let path = dir.join(LICENSE_FILE);
    if reject_symlink(&path, "license file").is_err() {
        return false;
    }
    std::fs::read_to_string(&path).is_ok()
}

/// Stable, privacy-safe per-install identifier — see `license/device.rs`.
///
/// Not yet called from outside this module (Stage C's key-entry UI is the
/// first caller that will display/report it) — fully implemented and
/// unit-tested now via `device::device_id_in`.
#[allow(dead_code)]
pub fn device_id() -> String {
    device::device_id_in(crate::paths::usagecheck_app_data_dir().as_deref())
}

/// D: serializes the ENTIRE read-validate-commit sequence — reading whatever
/// is currently stored, the network round trip, and the final commit — for
/// [`activate_in`], [`refresh_in`], AND [`deactivate_in`], across every
/// app-data directory. Acquired at the very top of each of those functions
/// (before "previous"/the record is even read) and held until they return.
///
/// (item 1 fix) [`deactivate_in`] takes this SAME lock too — not just
/// activate/refresh. Without it, an activation or refresh already in
/// flight (HTTP round trip in progress, lock not yet released) could commit
/// its watermark+record writes AFTER a concurrent deactivate has already
/// removed `license.json`, silently RECREATING the file and resurrecting a
/// license the user just asked to remove. Serializing deactivate through
/// this lock guarantees it always runs strictly before or strictly after
/// any in-flight activate/refresh commit — never in the gap between "file
/// removed" and "file rewritten" — so a deactivate that returns success
/// never has its result silently undone by a race. See
/// `http_tests.rs::deactivate_never_loses_to_a_refresh_that_commits_after_it_starts`
/// for the race this closes.
///
/// Chosen over a compare-and-swap-at-commit-time (re-read the stored token
/// and abort if its `issued_at` changed since the snapshot) because a single
/// lock is simpler to reason about and to review than a CAS/retry path, and
/// the section it guards — one HTTP round trip plus two small file writes —
/// is short enough that serializing it end to end costs nothing observable.
/// A single GLOBAL lock (not keyed per directory) is deliberate too: a real
/// build only ever operates against ONE app-data directory
/// ([`crate::paths::usagecheck_app_data_dir`]), so a global lock already
/// matches the actual concurrency this exists to close — a manual
/// "Activate" click racing the 24h periodic background refresh
/// ([`maybe_periodic_refresh`]). `tokio::sync::Mutex`, not
/// `std::sync::Mutex`: the guard is held across `.await` points (the network
/// request), which an async-aware mutex supports without blocking the
/// executor thread.
fn activate_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Shared core of [`activate`] and [`refresh`] (H3: validate fully BEFORE
/// persisting; never destroy a good record).
///
/// Runs the FULL [`decide_status`] evaluation — signature, `v`, `plan`,
/// device, watermark/rollback, expiry, offline grace — against an
/// IN-MEMORY candidate watermark (A: never against disk state that this same
/// call has already mutated) before writing anything to disk at all. Once
/// that evaluation resolves to `Pro`, the WATERMARK is written FIRST and the
/// RECORD is written SECOND — deliberately in that order, and NOT as one
/// atomic commit of both (they are two separate file writes; see the commit
/// site below for exactly what that ordering does and does not guarantee).
/// On any other outcome, or any earlier error (network, invalid token,
/// replay, device mismatch), NEITHER is written — the previously-stored
/// record and watermark (if any) are left completely untouched and an error
/// is returned instead. Called only with [`activate_lock`] already held by
/// the caller (D) — see [`activate_in`] / [`refresh_in`].
///
/// `previous`, when `Some`, is whatever record is CURRENTLY stored for this
/// `app_data_dir` (F4: this applies uniformly to both `activate` and
/// `refresh` — not just refresh — since a stored record's replay exposure
/// does not depend on which entry point produced the request that returned
/// it) — used for the H1 replay guard: when `previous`'s own token still
/// verifies, the candidate token's signed `issued_at` must be STRICTLY newer
/// than it, or the response is rejected as [`ActivationError::ReplayedToken`]
/// before any further processing (no persistence, no watermark repair).
/// When `previous` is `None`, or its token no longer verifies, there is no
/// trustworthy prior to compare against, so the guard is skipped and the
/// candidate proceeds through the rest of full validation instead
/// (signature, `v`/`plan`, device, `issued_at` freshness) — never a
/// distinct, weaker check.
async fn activate_or_refresh_in(
    app_data_dir: Option<&Path>,
    key: &str,
    previous: Option<&LicenseRecord>,
) -> Result<LicenseStatus, ActivationError> {
    // F5: refuse before contacting the server at all when the device id
    // cannot be durably persisted — binding a signed token to a
    // process-only id that a restart would replace is worse than refusing
    // outright, since the user would find themselves silently locked out
    // after that restart.
    let (this_device_id, device_persisted) = device::device_id_checked_in(app_data_dir);
    if !device_persisted {
        return Err(ActivationError::DeviceNotPersisted);
    }
    let endpoint = http::resolve_endpoint();
    let raw_token = http::request_token(&endpoint, key, &this_device_id).await?;

    let public_key = pubkey::resolve_public_key();
    let payload = match &public_key {
        Some(pk) => token::verify_token(&raw_token, pk)
            .map_err(|e| ActivationError::InvalidToken(e.to_string()))?,
        None => {
            return Err(ActivationError::InvalidToken(
                "no usable public key (release build with placeholder key?)".into(),
            ))
        }
    };
    if payload.v != 1 || payload.plan != "pro" {
        return Err(ActivationError::InvalidToken(
            "unexpected token payload (version/plan)".into(),
        ));
    }
    if payload.device != this_device_id {
        return Err(ActivationError::DeviceMismatch);
    }

    // H1 replay guard: reject a refresh response whose signed `issued_at`
    // does not strictly advance past the STORED token's own `issued_at`.
    // Nothing below this point runs on rejection — no persistence, no
    // watermark advance. When the previous token itself no longer verifies
    // (e.g. it was already invalid), there is no trustworthy prior
    // `issued_at` to compare against, so the guard is skipped rather than
    // blocking a legitimate re-activation.
    if let Some(previous) = previous {
        if let Some(prev_pk) = &public_key {
            if let Ok(prev_payload) = token::verify_token(&previous.token, prev_pk) {
                if payload.issued_at <= prev_payload.issued_at {
                    return Err(ActivationError::ReplayedToken);
                }
            }
        }
    }

    let now = Utc::now();
    let candidate = LicenseRecord {
        token: raw_token,
        key: key.to_string(),
        verified_at: now,
    };

    // A: evaluate against an IN-MEMORY candidate watermark — computed with
    // the exact same formula `watermark::repair_watermark_in` uses
    // (`max(now, issued_at)`), but WITHOUT touching disk — instead of
    // repairing the real watermark first and evaluating afterward. Nothing
    // on disk has been written yet at this point, for ANY outcome: a
    // candidate that fails this evaluation (expired, still within its own
    // signed grace but stale, a detected rollback, ...) leaves both
    // `license.json` and the watermark file completely untouched, matching
    // exactly what a rejected candidate does to `license.json` today.
    let candidate_watermark = watermark::WatermarkRecord {
        max_seen: now.max(payload.issued_at),
        source_issued_at: payload.issued_at,
    };
    let status = decide_status(
        Some(&candidate),
        now,
        &this_device_id,
        Some(candidate_watermark),
        public_key.as_ref(),
    );
    if !matches!(status, LicenseStatus::Pro { .. }) {
        return Err(ActivationError::NotEntitled(status));
    }

    // (item 1 fix) Write the WATERMARK first, then the RECORD — the reverse
    // of this function's previous order, and each write's failure now
    // PROPAGATES instead of being silently swallowed. Nothing above this
    // point wrote anything, and both writes use `write_private_file`'s
    // stage-into-a-temp-file-then-rename sequence, so a failure at either
    // step can never truncate or corrupt whatever was previously on disk.
    //
    // Why this order, and not the reverse: a crash/failure strictly BETWEEN
    // the two writes is still possible — they are two separate filesystem
    // operations, never one atomic transaction — so the question is which
    // of the two half-committed states is safe. With the watermark written
    // first:
    //   - watermark present, record absent: `read_record_in` returns
    //     `None`, `decide_status`'s very first check returns `Free` — no
    //     record, no entitlement, harmless.
    //   - watermark present, record present (both writes succeeded): the
    //     normal, fully-committed case.
    // The reverse order (the previous behavior) could instead leave a
    // PUBLISHED Pro record with no protecting watermark — H2 already forces
    // `decide_status` to fail that combination closed to
    // `GracePeriodEnded` rather than `Pro`, but the watermark write's error
    // being swallowed meant that split state was silently reachable at all,
    // which this fix removes at the source rather than relying solely on
    // the downstream fail-closed check. With THIS ordering, a record is
    // never even attempted to be written once the watermark write has
    // already failed, so "record present, watermark absent" cannot happen
    // as a result of this function.
    watermark::repair_watermark_in(app_data_dir, now, payload.issued_at)
        .map_err(ActivationError::Persist)?;
    write_record_in(app_data_dir, &candidate).map_err(ActivationError::Persist)?;

    Ok(status)
}

async fn activate_in(
    app_data_dir: Option<&Path>,
    key: &str,
) -> Result<LicenseStatus, ActivationError> {
    // D: hold the lock across the ENTIRE read-validate-commit sequence,
    // starting here — before "previous" is even read — so a concurrent
    // `activate`/`refresh` call can never observe a stale snapshot. See
    // `activate_lock`'s own doc comment for why this is a single global lock.
    let _guard = activate_lock().lock().await;
    // F4: apply the same replay guard `refresh_in` already applies — read
    // whatever record is currently stored (if any) and let
    // `activate_or_refresh_in` compare against it, rather than passing
    // `None` unconditionally and skipping the guard entirely.
    let previous = read_record_in(app_data_dir);
    activate_or_refresh_in(app_data_dir, key, previous.as_ref()).await
}

async fn refresh_in(app_data_dir: Option<&Path>) -> Result<LicenseStatus, ActivationError> {
    // D: see `activate_in` — same lock, same reasoning, held from before the
    // stored record is read through to the final commit.
    let _guard = activate_lock().lock().await;
    let record = read_record_in(app_data_dir).ok_or(ActivationError::NoStoredLicense)?;
    let key = record.key.clone();
    activate_or_refresh_in(app_data_dir, &key, Some(&record)).await
}

/// [`deactivate`] wraps this with the real app-data path; called from the
/// tray's "Deactivate license" row (`menu_actions::deactivate_license`) and
/// exercised end to end by `license/http_tests.rs`.
///
/// H6: rejects a symlinked app-data directory or license file before
/// touching either, matching the read (`read_record_in`) and write
/// (`write_record_in`) paths — a symlink here could otherwise be used to
/// delete an arbitrary file this process has permission to remove.
///
/// (item 1 fix) Async, and takes [`activate_lock`] for the whole sequence —
/// see that function's doc comment for why a concurrent activate/refresh
/// must never be able to commit after this has already removed the file.
async fn deactivate_in(app_data_dir: Option<&Path>) -> Result<(), String> {
    let _guard = activate_lock().lock().await;
    let Some(dir) = app_data_dir else {
        return Ok(());
    };
    reject_symlink(dir, "app data directory")?;
    let path = dir.join(LICENSE_FILE);
    reject_symlink(&path, "license file")?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove {}: {e}", path.display())),
    }
}

/// Activates a Pro license: POSTs `key` to the license API, verifies the
/// returned token's signature and payload BEFORE persisting anything, and on
/// success persists the record and returns the resulting status. Nothing is
/// written to disk when verification fails at any step.
///
/// Called from the tray's "Activate from clipboard" row
/// (`menu_actions::activate_license_from_clipboard`); also exercised end to
/// end by `license/http_tests.rs`.
pub async fn activate(key: &str) -> Result<LicenseStatus, ActivationError> {
    activate_in(crate::paths::usagecheck_app_data_dir().as_deref(), key).await
}

/// Re-runs activation with the stored key to obtain a freshly-signed token
/// (a newer `issued_at` — the actual freshness anchor, H1). Used by
/// the periodic background refresh (see [`maybe_periodic_refresh`]).
pub async fn refresh() -> Result<LicenseStatus, ActivationError> {
    refresh_in(crate::paths::usagecheck_app_data_dir().as_deref()).await
}

/// Removes the persisted license record (leaves `device-id` alone — the
/// device id is not license-specific).
///
/// Called from the tray's "Deactivate license" row
/// (`menu_actions::deactivate_license`); also exercised end to end by
/// `license/http_tests.rs`. Async (item 1 fix): serialized through the same
/// [`activate_lock`] as [`activate`]/[`refresh`], so it can never lose a
/// race against one already in flight.
pub async fn deactivate() -> Result<(), String> {
    deactivate_in(crate::paths::usagecheck_app_data_dir().as_deref()).await
}

const PERIODIC_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// H5: throttle state uses a MONOTONIC clock (`std::time::Instant`), not wall
/// clock — a backward NTP/manual clock correction must never suppress the
/// periodic refresh by making "time since last attempt" look negative or
/// small. `Instant` is process-local and not persisted across restarts by
/// design (a restart may therefore trigger one extra refresh sooner than 24h
/// after the last one; that is an acceptable, harmless cost — refreshing more
/// often than required is never a correctness problem, only skipping it is).
fn last_refresh_attempt() -> &'static std::sync::Mutex<Option<std::time::Instant>> {
    static LAST: std::sync::OnceLock<std::sync::Mutex<Option<std::time::Instant>>> =
        std::sync::OnceLock::new();
    LAST.get_or_init(|| std::sync::Mutex::new(None))
}

/// Attempts a background refresh at most once every
/// [`PERIODIC_REFRESH_INTERVAL`] (24h, measured on a monotonic clock — see
/// [`last_refresh_attempt`]), and only when a record already exists. A failed
/// refresh is logged and otherwise ignored — the offline grace period in
/// [`decide_status`] is what governs entitlement between successful
/// verifications, not this loop; a transient network failure must not
/// downgrade the user immediately. Never panics. Callers should `spawn` this
/// rather than `.await` it inline, so it never blocks the tray/poller.
pub async fn maybe_periodic_refresh() {
    let app_data_dir = crate::paths::usagecheck_app_data_dir();
    if read_record_in(app_data_dir.as_deref()).is_none() {
        return;
    }

    let now = std::time::Instant::now();
    {
        let mut last = last_refresh_attempt()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(previous) = *last {
            if now.saturating_duration_since(previous) < PERIODIC_REFRESH_INTERVAL {
                return;
            }
        }
        *last = Some(now);
    }

    // H4: NEVER log server-provided text (`error`'s `Display`, which for
    // `ActivationError::Server{code, message}` embeds attacker-controlled
    // server response text). Log only a fixed message plus the coarse,
    // self-generated `classify()` — the full error remains available to any
    // caller that wants it for UI display (`refresh()`'s `Result`), just
    // never printed here.
    if let Err(error) = refresh().await {
        eprintln!(
            "license: periodic refresh failed (offline grace still governs); class={}",
            error.classify()
        );
    }
}

/// Test-only seam for `menu_actions_tests.rs` (B0.4 dispatch-gate
/// integration tests): persists a genuinely valid, signed Pro
/// [`LicenseRecord`] for `app_data_dir` — mirrors exactly what a real
/// [`activate`] call persists, without needing a mock HTTP server. The
/// caller is responsible for pointing [`pubkey::resolve_public_key`] at the
/// matching public key (`USAGECHECK_LICENSE_PUBKEY`, debug-only — see
/// [`LICENSE_ENV_LOCK`]) so `status()`/`is_pro()` verify it exactly as they
/// would a real activation. Exists here (rather than exposing the `device`/
/// `token` submodules crate-wide) so the license module's internal API
/// surface widens by exactly one test-only function, not by whole modules.
#[cfg(test)]
pub(crate) fn testing_persist_pro_license(app_data_dir: &Path, signing_key: &ed25519_dalek::SigningKey) {
    let this_device_id = device::device_id_in(Some(app_data_dir));
    let now = Utc::now();
    let issued_at = now - Duration::hours(1);
    let payload = token::TokenPayload {
        v: 1,
        key_id: "menu-actions-test".into(),
        plan: "pro".into(),
        device: this_device_id,
        issued_at,
        expires_at: None,
    };
    let record = LicenseRecord {
        token: token::encode_token(&payload, signing_key),
        key: "TEST-KEY".into(),
        verified_at: now,
    };
    write_record_in(Some(app_data_dir), &record).expect("persist test license record");
    // H2/F1: `decide_status` fails CLOSED on a missing watermark whenever a
    // record is present, and a read/status path never repairs it (F1) — so
    // establish one here too, mirroring what a real `activate()`/`refresh()`
    // call does (F3's `repair_watermark_in`), so callers of this test seam
    // get the same Pro verdict a genuine activation would produce.
    watermark::repair_watermark_in(Some(app_data_dir), now, issued_at)
        .expect("persist test watermark");
}

/// Shared across every test that reads or mutates ANY of the three
/// process-wide license env vars — `USAGECHECK_LICENSE_PUBKEY`,
/// `USAGECHECK_LICENSE_API`, and `USAGECHECK_APP_DATA_DIR` (`paths.rs`'s
/// debug-only test seam) — to exercise the REAL `pubkey::resolve_public_key()`
/// / `http::resolve_endpoint()` / app-data-dir resolution paths against a
/// throwaway test key/mock server/tempdir. Used by (at least)
/// `status_tests.rs`, `http/http_tests.rs`, `../menu_actions_tests.rs`, and
/// `../paths.rs`'s test module.
///
/// This MUST be the only lock any of those tests take for this purpose: a
/// single global lock is required because `cargo test` runs every
/// `#[cfg(test)]` module in one process, concurrently by default, and two
/// independent per-var locks would not prevent one test's env mutation from
/// racing another's — e.g. test A sets `USAGECHECK_LICENSE_API` to its own
/// one-shot mock server while holding only a pubkey-scoped lock, test B
/// (holding only an app-data-scoped lock) overwrites the same var to point
/// at ITS mock server, and A's `activate_in` then posts to B's server, so
/// A's server never receives a request and blocks forever in
/// `handle.join()`. Previously this was two separate mutexes
/// (`PUBKEY_ENV_LOCK`, `APP_DATA_ENV_LOCK`) that some tests took in
/// different combinations — exactly the gap that produced that race. There
/// is now exactly one lock and no way to take two in a different order.
#[cfg(test)]
pub(crate) static LICENSE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
#[path = "status_tests.rs"]
mod status_tests;
