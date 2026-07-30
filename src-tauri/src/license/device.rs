//! Stable, privacy-safe per-install device id.
//!
//! B0.3 (Stage-A residual fix): the previous implementation was a racy
//! read-then-write — two concurrent first-time callers could each mint and
//! return a DIFFERENT raw UUID, only one of which ended up persisted. This
//! version resolves the id under a single process-wide lock covering the
//! entire read-or-mint-or-persist sequence, and caches the RAW uuid for
//! every outcome (including `app_data_dir: None`, where persistence is
//! structurally impossible, and a persistence failure that later recovers)
//! so every caller within this process converges on exactly one value, and a
//! later successful persist writes the CACHED value rather than minting a
//! fresh one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use uuid::Uuid;

use crate::store::{reject_symlink, write_private_file};

const DEVICE_ID_FILE: &str = "device-id";

#[derive(Clone)]
struct CachedDeviceId {
    raw_uuid: String,
    persisted: bool,
}

fn registry() -> &'static Mutex<HashMap<Option<PathBuf>, CachedDeviceId>> {
    static REGISTRY: OnceLock<Mutex<HashMap<Option<PathBuf>, CachedDeviceId>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// B0.2: rejects a symlinked app-data DIRECTORY (not just a symlinked
/// device-id file) before reading — mirrors the write path, which already
/// rejected a symlinked directory.
fn read_persisted_uuid(app_data_dir: Option<&Path>) -> Option<String> {
    let dir = app_data_dir?;
    reject_symlink(dir, "app data directory").ok()?;
    let path = dir.join(DEVICE_ID_FILE);
    reject_symlink(&path, "device id file").ok()?;
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    Uuid::parse_str(trimmed).ok()?;
    Some(trimmed.to_string())
}

fn try_persist(app_data_dir: Option<&Path>, raw_uuid: &str) -> bool {
    let Some(dir) = app_data_dir else {
        return false;
    };
    if reject_symlink(dir, "app data directory").is_err() {
        return false;
    }
    let path = dir.join(DEVICE_ID_FILE);
    if reject_symlink(&path, "device id file").is_err() {
        return false;
    }
    write_private_file(&path, raw_uuid).is_ok()
}

/// Single-flight, cache-first resolution of the RAW per-install UUID, plus
/// whether it is DURABLY persisted to disk right now (F5). Never exposed
/// outside this module — the public surface only ever sees
/// [`hash_device_uuid`]'s output (see [`super::device_id`] /
/// [`device_id_checked_in`]).
///
/// `app_data_dir: None` structurally cannot persist anything, so it always
/// reports `persisted: false` here — unlike the pre-F5 behavior (which
/// treated a `None` dir as "good enough" and returned the cached id without
/// distinguishing it from a real persisted one), a caller that cares about
/// durability (F5) must see this case as NOT persisted too.
fn resolve_raw_device_id(app_data_dir: Option<&Path>) -> (String, bool) {
    let key = app_data_dir.map(Path::to_path_buf);
    let mut registry = registry().lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    if let Some(cached) = registry.get(&key).cloned() {
        if cached.persisted {
            // C: a cached `persisted = true` is not proof the file is STILL
            // there right now — it only proves it was there the last time
            // this process checked. The file can be deleted or replaced out
            // from under the process (manual deletion, a dropped network
            // volume the app-data dir lives on, ...), so re-read the durable
            // state and confirm it still exists AND still matches the
            // cached value before trusting the cached flag.
            if app_data_dir.is_none() {
                // No disk state is possible in this branch to begin with —
                // "persisted" here only ever meant "stable in-process".
                return (cached.raw_uuid, true);
            }
            if read_persisted_uuid(app_data_dir).as_deref() == Some(cached.raw_uuid.as_str()) {
                return (cached.raw_uuid, true);
            }
            // The on-disk file is gone, replaced, or no longer matches:
            // re-persist the SAME cached value now — never mint a new one,
            // so this process (and every earlier caller that already
            // observed this id) converges back onto durable state before
            // anything trusts it again.
            let persisted = try_persist(app_data_dir, &cached.raw_uuid);
            registry.insert(
                key,
                CachedDeviceId {
                    raw_uuid: cached.raw_uuid.clone(),
                    persisted,
                },
            );
            return (cached.raw_uuid, persisted);
        }
        if app_data_dir.is_none() {
            return (cached.raw_uuid, false);
        }
        // A previous mint on this dir could not be persisted. Retry
        // persisting the SAME cached value now — never mint a new one, so a
        // later-recovering write lands the id every earlier caller already
        // observed, not a fresh random one.
        if try_persist(app_data_dir, &cached.raw_uuid) {
            registry.insert(
                key,
                CachedDeviceId {
                    raw_uuid: cached.raw_uuid.clone(),
                    persisted: true,
                },
            );
            return (cached.raw_uuid, true);
        }
        return (cached.raw_uuid, false);
    }

    if let Some(existing) = read_persisted_uuid(app_data_dir) {
        registry.insert(
            key,
            CachedDeviceId {
                raw_uuid: existing.clone(),
                persisted: true,
            },
        );
        return (existing, true);
    }

    let fresh = Uuid::new_v4().to_string();
    let persisted = try_persist(app_data_dir, &fresh);
    registry.insert(
        key,
        CachedDeviceId {
            raw_uuid: fresh.clone(),
            persisted,
        },
    );
    (fresh, persisted)
}

/// (B) Genuinely read-only device id lookup for the READ/status path: returns
/// the SHA-256 hash of the persisted UUID if `<app_data>/device-id` already
/// exists and parses, `None` otherwise. Deliberately bypasses the in-process
/// cache/registry entirely (unlike [`device_id_in`] / [`device_id_checked_in`],
/// which are single-flight and cache-first for the activation path) — a
/// status/read evaluation must never mint or persist a device id, and never
/// via a side door through a cache that some earlier activation attempt
/// might have populated with a NOT-actually-persisted value. `None` here
/// means "no device id to match the token's device against", which the
/// caller ([`super::status_in`]) treats as non-Pro, never as license to mint
/// one — minting/persisting stays exclusive to the activation path, where
/// durability is already a precondition (F5).
pub(super) fn device_id_read_only_in(app_data_dir: Option<&Path>) -> Option<String> {
    read_persisted_uuid(app_data_dir).map(|raw| hash_device_uuid(&raw))
}

fn hash_device_uuid(raw_uuid: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(raw_uuid.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Stable, privacy-safe per-install identifier: the SHA-256 hex of a random
/// UUID v4 generated on first use and persisted at `<app_data>/device-id`.
/// Stable across restarts; contains no personal data. Never returns the raw
/// UUID — see [`resolve_raw_device_id`].
pub(super) fn device_id_in(app_data_dir: Option<&Path>) -> String {
    hash_device_uuid(&resolve_raw_device_id(app_data_dir).0)
}

/// F5: same identifier as [`device_id_in`], plus whether it is DURABLY
/// persisted to disk right now. `super::activate_or_refresh_in` refuses to
/// contact the server at all when this reports `false` — binding a signed
/// token to a process-only id that a restart would replace with a
/// DIFFERENT id would silently deny a legitimate user after that restart.
pub(super) fn device_id_checked_in(app_data_dir: Option<&Path>) -> (String, bool) {
    let (raw_uuid, persisted) = resolve_raw_device_id(app_data_dir);
    (hash_device_uuid(&raw_uuid), persisted)
}

#[cfg(test)]
#[path = "device_tests.rs"]
mod tests;
