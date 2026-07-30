//! Resolution of the Ed25519 public key used to verify license tokens.
//!
//! The site (autoworkit.com) holds the PRIVATE key and signs activation
//! tokens; the app only ever holds the PUBLIC key, embedded at build time.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::VerifyingKey;

/// Placeholder Ed25519 public key — the verifying key derived from an
/// all-zero SIGNING key (never an arbitrary/invalid byte string: it must
/// always successfully decode as SOME point, so `is_placeholder` below can
/// tell "not replaced yet" apart from "corrupt embedded constant").
///
/// THIS MUST BE REPLACED WITH THE REAL autoworkit.com PRODUCTION PUBLIC KEY
/// BEFORE SHIPPING A RELEASE BUILD. See `docs/LICENSE_API.md` for how the
/// site generates and publishes the real key. Until it is replaced, a
/// *release* build fails closed (every token fails verification, so the
/// license is always `Free`) rather than silently trusting an unset key —
/// see [`resolve_public_key`]. `pubkey_tests.rs` re-derives this value at
/// test time and asserts it matches, so a typo here is caught mechanically.
pub(super) const EMBEDDED_PUBLIC_KEY_B64: &str = "O2onvM62pC1io6jQKm8Nc2UyFXcd4kOmOsBIoYtZ2ik=";

/// Debug-build-only override so integration tests (and a developer pointed
/// at a staging server) can supply a real key without touching the embedded
/// constant. Read ONLY when `cfg!(debug_assertions)` — a release binary can
/// never be pointed at an attacker-controlled key via this env var, because
/// the branch that reads it does not exist in a release build.
const PUBKEY_ENV_OVERRIDE: &str = "USAGECHECK_LICENSE_PUBKEY";

fn decode_public_key(b64: &str) -> Option<VerifyingKey> {
    let bytes = STANDARD.decode(b64.trim()).ok()?;
    let array: [u8; 32] = bytes.try_into().ok()?;
    VerifyingKey::from_bytes(&array).ok()
}

/// Bytes of the placeholder public key, derived at runtime from an all-zero
/// signing key rather than hardcoded a second time — this is the single
/// source of truth `EMBEDDED_PUBLIC_KEY_B64` must decode to.
fn placeholder_key_bytes() -> [u8; 32] {
    ed25519_dalek::SigningKey::from_bytes(&[0u8; 32])
        .verifying_key()
        .to_bytes()
}

fn is_placeholder(b64: &str) -> bool {
    decode_public_key(b64)
        .map(|key| key.to_bytes() == placeholder_key_bytes())
        .unwrap_or(false)
}

/// Distinguishes "a genuinely valid, non-placeholder key" from "still the
/// placeholder" from "malformed/corrupt constant" (bad base64, wrong
/// length, or bytes that don't decode to a valid Ed25519 curve point).
///
/// (item 2 fix) `is_placeholder` alone cannot make this distinction: it
/// returns `false` for BOTH a good non-placeholder key AND an undecodable
/// one, which let a corrupt/garbage `EMBEDDED_PUBLIC_KEY_B64` silently PASS
/// the `embedded_public_key_is_not_placeholder` release guard even though
/// the app could never verify a token with it. That guard now asserts
/// exactly `ValidNonPlaceholder` instead of `!is_placeholder(..)`.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbeddedKeyCheck {
    /// Decodes to a valid Ed25519 point and differs from the placeholder —
    /// the only state a release build should ever ship with.
    ValidNonPlaceholder,
    /// Decodes fine, but is still the documented placeholder.
    Placeholder,
    /// Fails to decode as base64, is the wrong length, or does not decode
    /// to a valid Ed25519 curve point.
    Malformed,
}

#[cfg(test)]
fn check_embedded_key(b64: &str) -> EmbeddedKeyCheck {
    match decode_public_key(b64) {
        None => EmbeddedKeyCheck::Malformed,
        Some(key) if key.to_bytes() == placeholder_key_bytes() => EmbeddedKeyCheck::Placeholder,
        Some(_) => EmbeddedKeyCheck::ValidNonPlaceholder,
    }
}

/// Resolves the Ed25519 public key used to verify license tokens, or `None`
/// when no usable key is available (every token then fails verification —
/// see `decide_status`, which treats a missing key exactly like an
/// unverifiable token: `Free`).
///
/// - **Debug builds only:** `USAGECHECK_LICENSE_PUBKEY` (base64, 32 bytes),
///   if set and valid, overrides the embedded constant. This is the seam
///   integration tests use to point verification at a throwaway test key
///   signed by a matching test `SigningKey` (see `license/token.rs` tests
///   and the mock-server tests in `license/http_tests.rs`).
/// - **Otherwise:** the embedded [`EMBEDDED_PUBLIC_KEY_B64`] is used, UNLESS
///   it is still the placeholder AND this is a release build — in which case
///   this returns `None` so a release binary that forgot to embed the real
///   production key can never grant Pro to anyone.
pub(super) fn resolve_public_key() -> Option<VerifyingKey> {
    resolve_public_key_for(
        cfg!(debug_assertions),
        std::env::var(PUBKEY_ENV_OVERRIDE).ok().as_deref(),
    )
}

/// Pure core of [`resolve_public_key`], with the build kind and env value
/// injected explicitly so the release-build fail-closed behavior is
/// unit-testable without an actual release build (`cfg!` can't be toggled at
/// test time) and without mutating process-global env state.
fn resolve_public_key_for(debug_build: bool, env_override: Option<&str>) -> Option<VerifyingKey> {
    if debug_build {
        if let Some(raw) = env_override {
            if let Some(key) = decode_public_key(raw) {
                return Some(key);
            }
        }
    }
    if !debug_build && is_placeholder(EMBEDDED_PUBLIC_KEY_B64) {
        return None;
    }
    decode_public_key(EMBEDDED_PUBLIC_KEY_B64)
}

#[cfg(test)]
#[path = "pubkey_tests.rs"]
mod tests;
