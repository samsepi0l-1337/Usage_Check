//! Clock-rollback / tamper detection (B0.1, Stage-A residual fix; F1-F3
//! rework, adversarial-review follow-up).
//!
//! Unifying rule: **the watermark may only ever be ADVANCED by
//! SERVER-AUTHENTICATED time** — a freshly-signed token's own `issued_at`
//! (see [`repair_watermark_in`], called only from
//! `super::activate_or_refresh_in`, and only once the candidate has passed
//! signature/device/replay verification AND the FULL `decide_status`
//! evaluation — against an in-memory candidate watermark, not this one —
//! has resolved to `Pro` (A): a candidate that verifies but is expired,
//! future-issued, or otherwise rejected never reaches this call at all, so
//! it can never advance the watermark either). It is never created or
//! repaired by a read/status path — see
//! `super::status_in`, which only ever READS the watermark now. Rolling the
//! wall clock back to just after the token's signed `issued_at` (H1: the
//! actual freshness anchor — never `verified_at`, which carries no
//! authority) would otherwise make `now - issued_at` never exceed
//! `OFFLINE_GRACE`, restoring Pro indefinitely on a rolled-back clock. This
//! module persists the highest wall-clock time a SUCCESSFUL online
//! verification has ever observed, together with the `issued_at` of the
//! token that set it, so both a clock rollback (`is_rolled_back`) and a
//! hand-edited/regressed watermark file (the `max_seen < issued_at` check in
//! `super::decide_status`) are detectable. A MISSING watermark is treated
//! the same way whenever a license record exists (H2, enforced in
//! `decide_status` — not here, since a bare read cannot tell "no record to
//! protect" apart from "record present, watermark lost"; see
//! `decide_status`, which never lets a rollback or a lost/regressed
//! watermark EXTEND entitlement — either can only ever force
//! re-verification, never skip it).

use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::store::{reject_symlink, write_private_file};

const WATERMARK_FILE: &str = "clock-watermark";

/// Tolerance for a legitimate small backward NTP correction. A rollback
/// larger than this is treated as clock tampering, not a correction.
pub(super) const ROLLBACK_SKEW: Duration = Duration::minutes(5);

/// Persisted watermark shape (F2): `max_seen` alone cannot distinguish a
/// legitimate advance from a hand-edited regression, so it is stored
/// together with the `issued_at` of the token that produced it. A
/// legitimate `max_seen` can never fall behind the `issued_at` of the token
/// currently being evaluated — every legitimate advance sets
/// `max_seen = max(now, issued_at) >= issued_at` at the moment of a
/// successful online verification (`repair_watermark_in`) — so
/// `max_seen < (current token's) issued_at` is decisive evidence the file
/// was rewritten by hand after that token was issued.
// `pub(crate)`, not `pub(super)`: `decide_status` (which takes this type)
// is itself `pub fn` — reachable crate-wide (the enclosing `mod license;` in
// `main.rs` is private, so "crate-wide" is this crate's own effective
// ceiling) — and a parameter type may never be less visible than the
// function it appears on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WatermarkRecord {
    pub(crate) max_seen: DateTime<Utc>,
    pub(crate) source_issued_at: DateTime<Utc>,
}

/// Reads the persisted watermark record, if any. Missing, malformed
/// (including a leftover plain-timestamp file from before this format
/// existed), unreadable, or symlinked all collapse to `None` — deliberately
/// indistinguishable to a caller, since `decide_status` (H2) treats every
/// one of those cases identically: fail closed.
///
/// B0.2: rejects a symlinked app-data DIRECTORY (not just a symlinked
/// watermark file) before reading, mirroring the license-record and
/// device-id read paths.
///
/// Side-effect free — F1: reading must never create, repair, or otherwise
/// write the watermark. Nothing in this function touches disk.
pub(super) fn read_watermark_in(app_data_dir: Option<&Path>) -> Option<WatermarkRecord> {
    let dir = app_data_dir?;
    reject_symlink(dir, "app data directory").ok()?;
    let path = dir.join(WATERMARK_FILE);
    reject_symlink(&path, "clock watermark file").ok()?;
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// F3: REPLACES the persisted watermark with
/// `{ max_seen: max(now, source_issued_at), source_issued_at }`, ignoring
/// whatever was previously on disk (missing, corrupt, or far-future). This
/// is the ONLY way the watermark ever advances — called exclusively from
/// `super::activate_or_refresh_in`, and only AFTER the candidate has passed
/// signature/device/replay verification AND the full `decide_status`
/// evaluation (run first, against an in-memory candidate watermark computed
/// with this exact same formula — see `super::activate_or_refresh_in`) has
/// resolved to `Pro` (A) — never called for a candidate that fails that
/// evaluation, so a rejected candidate can never advance this file. Safe to
/// ignore whatever was previously on disk once reached, because the value
/// it writes is anchored to a signature the client cannot forge: a genuine
/// online verification that already resolved to `Pro` is definitionally
/// more trustworthy than any locally-held state it is about to overwrite.
///
/// (item 1 fix) Called FIRST on the success path of an online verification
/// that is committing — BEFORE the license record write, not after (see the
/// caller, `super::activate_or_refresh_in`) — and its failure now
/// PROPAGATES as an `Err` rather than being silently swallowed. Previously
/// this was best-effort (mirroring the old monotonic `update_watermark_in`)
/// on the theory that the file holds only timestamps, nothing sensitive; the
/// actual consequence of swallowing a failure here, though, was that the
/// caller could go on to publish a Pro license record with NO protecting
/// watermark behind it. Propagating the error, combined with the
/// watermark-before-record ordering at the call site, ensures the record
/// write is never even attempted once this one has failed.
pub(super) fn repair_watermark_in(
    app_data_dir: Option<&Path>,
    now: DateTime<Utc>,
    source_issued_at: DateTime<Utc>,
) -> Result<(), String> {
    let dir = app_data_dir.ok_or_else(|| "could not resolve app data directory".to_string())?;
    reject_symlink(dir, "app data directory")?;
    let path = dir.join(WATERMARK_FILE);
    reject_symlink(&path, "clock watermark file")?;
    let record = WatermarkRecord {
        max_seen: now.max(source_issued_at),
        source_issued_at,
    };
    let json = serde_json::to_string(&record).map_err(|e| format!("serialize watermark: {e}"))?;
    write_private_file(&path, &json)
}

/// Pure: true when `now` is earlier than `watermark` by more than
/// [`ROLLBACK_SKEW`] — i.e. the wall clock was rolled back far enough to
/// look like tampering rather than an NTP correction.
pub(super) fn is_rolled_back(watermark: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    match watermark {
        Some(w) => now < w - ROLLBACK_SKEW,
        None => false,
    }
}

#[cfg(test)]
#[path = "watermark_tests.rs"]
mod tests;
