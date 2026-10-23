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
const DEV_LOCAL_KEY: &str = "dev-local";

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
    /// Debug-build-only local verification override. This has no persisted
    /// record and is deliberately distinct from [`Self::Pro`] so every UI
    /// and API consumer can disclose that it is not a real activation.
    ProDevOverride,
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
    candidate.len() == 64
        && candidate
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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
/// license persistence repairs it (F3, `watermark::repair_watermark_in`,
/// called from [`activate_or_refresh_in`] and
/// [`persist_signed_pro_license_in`]).
///
/// SECURITY (F2): a watermark that IS present but whose `max_seen` predates
/// the CURRENT token's own signed `issued_at` is likewise treated as
/// tampering, not as "no rollback detected". A legitimate watermark can
/// never fall behind the `issued_at` of the token it is being evaluated
/// against — every legitimate advance sets `max_seen = max(now, issued_at)`
/// at the moment of a successful license persistence — so
/// `max_seen < issued_at` is decisive evidence the watermark file was
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
///    compared for equality.
/// 4. payload `device` must equal `this_device_id`.
/// 5. `issued_at` in the future → `Free`.
/// 6. a MISSING watermark, a detected clock ROLLBACK, or a watermark whose
///    `max_seen` predates this token's `issued_at` → `GracePeriodEnded`.
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
    if !is_valid_device_id_format(this_device_id) || !is_valid_device_id_format(&payload.device) {
        return LicenseStatus::Free;
    }
    if payload.device != this_device_id {
        return LicenseStatus::Free;
    }
    if payload.issued_at > now {
        return LicenseStatus::Free;
    }
    let Some(watermark) = watermark else {
        return LicenseStatus::GracePeriodEnded;
    };
    if watermark::is_rolled_back(Some(watermark.max_seen), now) {
        return LicenseStatus::GracePeriodEnded;
    }
    if watermark.max_seen < payload.issued_at {
        return LicenseStatus::GracePeriodEnded;
    }
    if let Some(expires_at) = payload.expires_at {
        if expires_at <= now {
            return LicenseStatus::Expired;
        }
    }
    if now - payload.issued_at > OFFLINE_GRACE {
        return LicenseStatus::GracePeriodEnded;
    }
    LicenseStatus::Pro {
        expires_at: payload.expires_at,
    }
}

/// Returns whether an optional environment value is one of the accepted
/// truthy spellings after trimming and ASCII case folding. Missing, blank,
/// falsy, and unknown values return `false`; this function performs no I/O.
fn env_flag_is_truthy(raw: Option<&str>) -> bool {
    raw.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Returns the trimmed value when an optional environment value contains
/// non-whitespace text, otherwise returns `None`. The returned slice borrows
/// the caller's input and no environment is read here.
fn trimmed_nonempty_env(raw: Option<&str>) -> Option<&str> {
    raw.map(str::trim).filter(|value| !value.is_empty())
}

/// Whether the raw `USAGECHECK_FORCE_PRO` value enables the local debug
/// override. `false` is returned for every release-build input and for
/// missing or non-truthy values.
///
/// `debug_build` is `cfg!(debug_assertions)` at the runtime call site, so a
/// release binary cannot be unlocked with this environment variable.
fn force_pro_enabled_for(debug_build: bool, raw: Option<&str>) -> bool {
    debug_build && env_flag_is_truthy(raw)
}

/// Returns the trimmed startup HTTP activation key only in a debug build.
/// Release builds, missing values, and blank values return `None`. The key
/// is borrowed for dispatch and must never be written to a log.
///
/// `debug_build` is `cfg!(debug_assertions)` at the runtime call site.
fn http_auto_activate_key_for(debug_build: bool, raw: Option<&str>) -> Option<&str> {
    debug_build.then(|| trimmed_nonempty_env(raw)).flatten()
}

/// Returns whether local persisted development licensing is enabled.
/// Debug builds default to enabled when the variable is absent; explicit
/// values enable it only when truthy. Release builds always return `false`.
///
/// `debug_build` is `cfg!(debug_assertions)` at the runtime call site.
fn dev_license_enabled_for(debug_build: bool, raw: Option<&str>) -> bool {
    if !debug_build {
        return false;
    }
    raw.is_none_or(|value| env_flag_is_truthy(Some(value)))
}

/// Returns `true` only when debug startup should persist the local
/// placeholder-signed Pro license. FORCE_PRO, a non-empty HTTP key, an
/// existing Pro status, an explicit disable, or a release build returns
/// `false`. This pure helper neither reads environment variables nor logs
/// the HTTP key or the `dev-local` dummy key.
///
/// `debug_build` is `cfg!(debug_assertions)` at the runtime call site.
fn should_seed_local_dev_license_for(
    debug_build: bool,
    license_key_raw: Option<&str>,
    force_pro_raw: Option<&str>,
    dev_license_raw: Option<&str>,
    already_pro: bool,
) -> bool {
    debug_build
        && !force_pro_enabled_for(debug_build, force_pro_raw)
        && http_auto_activate_key_for(debug_build, license_key_raw).is_none()
        && !already_pro
        && dev_license_enabled_for(debug_build, dev_license_raw)
}

/// The one debug-startup licensing effect selected by the pure priority
/// table. `HttpActivate` carries a trimmed borrowed key which callers must
/// never log; `Skip` and `Seed` carry no secret data.
enum DebugStartupLicenseAction<'a> {
    Skip,
    HttpActivate(&'a str),
    Seed,
}

/// Selects one startup licensing action without reading environment or
/// performing I/O. Release and FORCE_PRO inputs skip all work; a non-empty
/// HTTP key wins over existing Pro state and local seeding; otherwise an
/// eligible non-Pro debug build seeds locally. No key is logged.
///
/// `debug_build` is `cfg!(debug_assertions)` at the runtime call site.
fn debug_startup_license_action_for<'a>(
    debug_build: bool,
    force_pro_raw: Option<&str>,
    license_key_raw: Option<&'a str>,
    dev_license_raw: Option<&str>,
    already_pro: bool,
) -> DebugStartupLicenseAction<'a> {
    if !debug_build || force_pro_enabled_for(debug_build, force_pro_raw) {
        return DebugStartupLicenseAction::Skip;
    }
    if let Some(key) = http_auto_activate_key_for(debug_build, license_key_raw) {
        return DebugStartupLicenseAction::HttpActivate(key);
    }
    if should_seed_local_dev_license_for(
        debug_build,
        license_key_raw,
        force_pro_raw,
        dev_license_raw,
        already_pro,
    ) {
        return DebugStartupLicenseAction::Seed;
    }
    DebugStartupLicenseAction::Skip
}

/// Reads the real FORCE_PRO environment value and returns its debug-only
/// decision. Missing and unrecognized values leave the override disabled.
fn force_pro_enabled() -> bool {
    force_pro_enabled_for(
        cfg!(debug_assertions),
        std::env::var("USAGECHECK_FORCE_PRO").ok().as_deref(),
    )
}

/// Logs the debug FORCE_PRO override at most once per process. No license
/// key, token, or device identifier is included.
fn log_force_pro_override_once() {
    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| {
        eprintln!("license: USAGECHECK_FORCE_PRO is set — Pro forced (debug build only)");
    });
}

/// Rejects symlinked app-data and license paths, then returns a parsed
/// license record. Missing, unreadable, malformed, or rejected paths return
/// `None` without modifying the filesystem.
fn read_record_in(app_data_dir: Option<&Path>) -> Option<LicenseRecord> {
    let dir = app_data_dir?;
    reject_symlink(dir, "app data directory").ok()?;
    let path = dir.join(LICENSE_FILE);
    reject_symlink(&path, "license file").ok()?;
    let json = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&json).ok()
}

/// Serializes and privately writes a license record under the supplied
/// app-data directory. Missing or rejected paths and serialization or write
/// failures are returned as strings.
fn write_record_in(app_data_dir: Option<&Path>, record: &LicenseRecord) -> Result<(), String> {
    let dir = app_data_dir.ok_or_else(|| "could not resolve app data directory".to_string())?;
    reject_symlink(dir, "app data directory")?;
    let path = dir.join(LICENSE_FILE);
    reject_symlink(&path, "license file")?;
    let json = serde_json::to_string_pretty(record)
        .map_err(|e| format!("serialize license record: {e}"))?;
    write_private_file(&path, &json)
}

/// Persists a perpetual signed Pro record bound to a durable device id,
/// writing the rollback watermark before the license record. Returns an
/// error without writing a record when the device id is not persisted, and
/// propagates watermark or record write failures. The key, token, and device
/// id are never logged.
fn persist_signed_pro_license_in(
    app_data_dir: Option<&Path>,
    signing_key: &ed25519_dalek::SigningKey,
    key: &str,
    key_id: &str,
) -> Result<(), String> {
    let (this_device_id, persisted) = device::device_id_checked_in(app_data_dir);
    if !persisted {
        return Err("device id is not persisted".into());
    }

    let now = Utc::now();
    let issued_at = now - Duration::hours(1);
    let payload = token::TokenPayload {
        v: 1,
        key_id: key_id.to_string(),
        plan: "pro".to_string(),
        device: this_device_id,
        issued_at,
        expires_at: None,
    };
    let record = LicenseRecord {
        token: token::encode_token(&payload, signing_key),
        key: key.to_string(),
        verified_at: now,
    };

    watermark::repair_watermark_in(app_data_dir, now, issued_at)?;
    write_record_in(app_data_dir, &record)
}

/// Reads and verifies current status without creating or repairing any
/// device, record, or watermark files. Missing device state returns Free;
/// FORCE_PRO can return the explicitly marked debug override.
fn status_in(app_data_dir: Option<&Path>) -> LicenseStatus {
    if force_pro_enabled() {
        log_force_pro_override_once();
        return LicenseStatus::ProDevOverride;
    }
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
    matches!(
        status_in(app_data_dir),
        LicenseStatus::Pro { .. } | LicenseStatus::ProDevOverride
    )
}

/// True only when [`status`] grants Pro, including the explicitly-marked
/// debug-only [`LicenseStatus::ProDevOverride`] local verification state.
pub fn is_pro() -> bool {
    matches!(
        status(),
        LicenseStatus::Pro { .. } | LicenseStatus::ProDevOverride
    )
}

/// True when a license record is persisted on disk, regardless of whether it
/// currently verifies as Pro.
pub fn has_stored_license() -> bool {
    has_stored_license_in(crate::paths::usagecheck_app_data_dir().as_deref())
}

/// Returns whether a regular readable license file exists, even when its
/// contents are malformed or do not currently grant Pro. Missing, symlinked,
/// or unreadable paths return `false`.
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

/// Persists a placeholder-signed perpetual Pro license for local debug use
/// unless current status is already Pro or FORCE_PRO is active. Existing
/// non-Pro records are replaced. Persistence errors are returned unchanged;
/// neither the dummy key nor the signed token is logged.
fn seed_local_dev_license_in(app_data_dir: Option<&Path>) -> Result<(), String> {
    if matches!(
        status_in(app_data_dir),
        LicenseStatus::Pro { .. } | LicenseStatus::ProDevOverride
    ) {
        return Ok(());
    }

    let signing_key = ed25519_dalek::SigningKey::from_bytes(&[0u8; 32]);
    persist_signed_pro_license_in(app_data_dir, &signing_key, DEV_LOCAL_KEY, DEV_LOCAL_KEY)
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

/// Serializes the full activate/refresh/deactivate sequence across every
/// app-data directory. The async mutex is held across network waits so
/// deactivation cannot race with a later activation commit.
fn activate_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Validates a server token against the durable device and any trustworthy
/// previous token, then writes watermark first and record second. Every
/// rejected or pre-commit error leaves the license record untouched.
async fn activate_or_refresh_in(
    app_data_dir: Option<&Path>,
    key: &str,
    previous: Option<&LicenseRecord>,
) -> Result<LicenseStatus, ActivationError> {
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

    watermark::repair_watermark_in(app_data_dir, now, payload.issued_at)
        .map_err(ActivationError::Persist)?;
    write_record_in(app_data_dir, &candidate).map_err(ActivationError::Persist)?;

    Ok(status)
}

/// Activates one key under the shared mutation lock. It compares the
/// response against any currently stored trustworthy token and returns the
/// resulting status or the activation failure.
async fn activate_in(
    app_data_dir: Option<&Path>,
    key: &str,
) -> Result<LicenseStatus, ActivationError> {
    let _guard = activate_lock().lock().await;
    let previous = read_record_in(app_data_dir);
    activate_or_refresh_in(app_data_dir, key, previous.as_ref()).await
}

/// Refreshes the stored key under the shared mutation lock. Missing records
/// return `NoStoredLicense`; all other failures come from validation,
/// transport, or persistence.
async fn refresh_in(app_data_dir: Option<&Path>) -> Result<LicenseStatus, ActivationError> {
    let _guard = activate_lock().lock().await;
    let record = read_record_in(app_data_dir).ok_or(ActivationError::NoStoredLicense)?;
    let key = record.key.clone();
    activate_or_refresh_in(app_data_dir, &key, Some(&record)).await
}

/// Removes the license record under the same lock used by activation and
/// refresh. Missing app-data or record paths succeed; rejected symlinks and
/// other filesystem failures are returned.
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

/// Attempts startup HTTP activation only when a debug build receives a
/// non-empty key. Disabled inputs return `Ok(None)` without resolving an
/// endpoint or sending a request; enabled inputs return the activation
/// status or error. This function never logs the key or token.
async fn auto_activate_from_key_in(
    app_data_dir: Option<&Path>,
    debug_build: bool,
    raw_key: Option<&str>,
) -> Result<Option<LicenseStatus>, ActivationError> {
    let Some(key) = http_auto_activate_key_for(debug_build, raw_key) else {
        return Ok(None);
    };
    activate_in(app_data_dir, key).await.map(Some)
}

/// Returns the fixed startup activation failure line for an internally
/// generated error class. Server text, keys, and tokens are never included.
fn auto_activate_failure_log(class: ActivationErrorClass) -> String {
    format!("license: auto-activate failed; class={class}")
}

/// Applies the selected debug-startup action to one app-data directory.
/// HTTP failure is logged by class and never falls through to local seeding;
/// seed failure is logged with a fixed message. Every failure returns
/// normally so startup continues, and no key or token is logged.
async fn apply_debug_startup_license_in(
    app_data_dir: Option<&Path>,
    debug_build: bool,
    force_pro_raw: Option<&str>,
    license_key_raw: Option<&str>,
    dev_license_raw: Option<&str>,
) {
    let already_pro = matches!(
        status_in(app_data_dir),
        LicenseStatus::Pro { .. } | LicenseStatus::ProDevOverride
    );

    match debug_startup_license_action_for(
        debug_build,
        force_pro_raw,
        license_key_raw,
        dev_license_raw,
        already_pro,
    ) {
        DebugStartupLicenseAction::Skip => {}
        DebugStartupLicenseAction::HttpActivate(key) => {
            match auto_activate_from_key_in(app_data_dir, debug_build, Some(key)).await {
                Ok(Some(_)) => {
                    eprintln!("license: auto-activated from USAGECHECK_LICENSE_KEY");
                }
                Ok(None) => {}
                Err(error) => {
                    eprintln!("{}", auto_activate_failure_log(error.classify()));
                }
            }
        }
        DebugStartupLicenseAction::Seed => match seed_local_dev_license_in(app_data_dir) {
            Ok(()) => {
                eprintln!("license: seeded local debug Pro license");
            }
            Err(_) => {
                eprintln!("license: local debug license seed failed");
            }
        },
    }
}

/// Applies debug startup licensing before the first tray menu is built.
/// Release builds return before reading environment variables. Debug builds
/// run async activation on a dedicated thread with a current-thread Tokio
/// runtime, join it synchronously, and continue startup after runtime or
/// worker failure without logging keys or tokens.
pub fn apply_debug_startup_license() {
    if !cfg!(debug_assertions) {
        return;
    }

    let app_data_dir = crate::paths::usagecheck_app_data_dir();
    let force_pro_raw = std::env::var("USAGECHECK_FORCE_PRO").ok();
    let license_key_raw = std::env::var("USAGECHECK_LICENSE_KEY").ok();
    let dev_license_raw = std::env::var("USAGECHECK_DEV_LICENSE").ok();

    let worker = std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(_) => {
                eprintln!("license: debug startup runtime unavailable");
                return;
            }
        };

        runtime.block_on(apply_debug_startup_license_in(
            app_data_dir.as_deref(),
            cfg!(debug_assertions),
            force_pro_raw.as_deref(),
            license_key_raw.as_deref(),
            dev_license_raw.as_deref(),
        ));
    });

    if worker.join().is_err() {
        eprintln!("license: debug startup worker failed");
    }
}

/// Activates a Pro license: POSTs `key` to the license API, verifies the
/// returned token's signature and payload before persisting anything, and
/// returns the resulting status. Nothing is written when verification fails.
///
/// Called from the tray's "Activate from clipboard" row and the debug startup
/// HTTP auto-activation path; also exercised end to end by license HTTP tests.
pub async fn activate(key: &str) -> Result<LicenseStatus, ActivationError> {
    activate_in(crate::paths::usagecheck_app_data_dir().as_deref(), key).await
}

/// Re-runs activation with the stored key to obtain a freshly-signed token.
/// Returns the resulting status or the refresh failure.
pub async fn refresh() -> Result<LicenseStatus, ActivationError> {
    refresh_in(crate::paths::usagecheck_app_data_dir().as_deref()).await
}

/// Removes the persisted license record while leaving the durable device id.
/// Missing records succeed; filesystem and symlink failures are returned.
pub async fn deactivate() -> Result<(), String> {
    deactivate_in(crate::paths::usagecheck_app_data_dir().as_deref()).await
}

const PERIODIC_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Returns the process-local monotonic timestamp of the latest periodic
/// refresh attempt. Poisoned lock recovery is handled by the caller.
fn last_refresh_attempt() -> &'static std::sync::Mutex<Option<std::time::Instant>> {
    static LAST: std::sync::OnceLock<std::sync::Mutex<Option<std::time::Instant>>> =
        std::sync::OnceLock::new();
    LAST.get_or_init(|| std::sync::Mutex::new(None))
}

/// Attempts a background refresh at most once every 24 hours and only when a
/// record exists. Failures are logged by coarse class and otherwise ignored,
/// leaving offline grace to govern entitlement.
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

    if let Err(error) = refresh().await {
        eprintln!(
            "license: periodic refresh failed (offline grace still governs); class={}",
            error.classify()
        );
    }
}

/// Test-only seam that persists the same signed perpetual Pro shape used by
/// menu-action integration tests. Persistence failures panic at this explicit
/// test boundary; production code calls the fallible helper directly.
#[cfg(test)]
pub(crate) fn testing_persist_pro_license(
    app_data_dir: &Path,
    signing_key: &ed25519_dalek::SigningKey,
) {
    persist_signed_pro_license_in(
        Some(app_data_dir),
        signing_key,
        "TEST-KEY",
        "menu-actions-test",
    )
    .expect("persist test license and watermark");
}

/// Shared across every test that reads or mutates any process-wide license
/// environment variable: `USAGECHECK_LICENSE_PUBKEY`,
/// `USAGECHECK_LICENSE_API`, `USAGECHECK_APP_DATA_DIR`,
/// `USAGECHECK_LICENSE_KEY`, `USAGECHECK_DEV_LICENSE`, and
/// `USAGECHECK_FORCE_PRO`. One lock prevents one test from redirecting
/// another test's key, endpoint, app-data directory, or startup decision.
#[cfg(test)]
pub(crate) static LICENSE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
#[path = "debug_startup_http_tests.rs"]
mod debug_startup_http_tests;

#[cfg(test)]
#[path = "status_tests.rs"]
mod status_tests;
