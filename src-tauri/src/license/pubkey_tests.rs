use super::*;
use ed25519_dalek::SigningKey;

fn test_key_b64() -> String {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    STANDARD.encode(signing_key.verifying_key().to_bytes())
}

#[test]
fn embedded_constant_is_the_documented_placeholder() {
    // Mechanical guard against a typo in the hardcoded constant: it must
    // decode to exactly the runtime-derived placeholder bytes.
    //
    // NOTE: this test and `embedded_public_key_is_not_placeholder` below
    // assert OPPOSITE things on purpose — this one pins today's state (still
    // the placeholder); that one is the `#[ignore]`d CI release guard that
    // must start PASSING once the real key lands. Replacing the placeholder
    // means deleting/rewriting THIS test, not the other one.
    let decoded = decode_public_key(EMBEDDED_PUBLIC_KEY_B64).expect("constant must be valid");
    assert_eq!(decoded.to_bytes(), placeholder_key_bytes());
    assert!(
        is_placeholder(EMBEDDED_PUBLIC_KEY_B64),
        "EMBEDDED_PUBLIC_KEY_B64 must stay the documented placeholder \
         until it is replaced with the real production key"
    );
}

/// RELEASE GUARD (Stage C): `#[ignore]`d so the normal `cargo test -p
/// usage-app` run stays green while the real production key has not been
/// embedded yet — this is EXPECTED to fail until then. CI's release
/// workflow (`.github/workflows/release.yml`) runs it explicitly with
/// `--ignored --exact` as a step that must pass before any release bundle
/// is built; see that file's "Guard: embedded license public key" step for
/// exactly how it's invoked and what to do when it fires.
///
/// (item 2 fix) Asserts `check_embedded_key(..) == ValidNonPlaceholder`
/// rather than `!is_placeholder(..)`: the old assertion PASSED for a
/// corrupt/garbage constant too, because `is_placeholder` returns `false`
/// for both "genuinely not the placeholder" and "doesn't decode at all" —
/// so a typo'd or truncated `EMBEDDED_PUBLIC_KEY_B64` could ship a release
/// build the app could never verify anything with, and this guard would
/// have said nothing. `check_embedded_key` now requires the constant to (a)
/// decode as valid base64, (b) be exactly 32 bytes, (c) construct a valid
/// Ed25519 `VerifyingKey`, AND (d) differ from the placeholder — any
/// failure on any of those fails this test.
#[test]
#[ignore = "release guard — run explicitly in CI before building release bundles"]
fn embedded_public_key_is_not_placeholder() {
    assert_eq!(
        check_embedded_key(EMBEDDED_PUBLIC_KEY_B64),
        EmbeddedKeyCheck::ValidNonPlaceholder,
        "EMBEDDED_PUBLIC_KEY_B64 (src-tauri/src/license/pubkey.rs) is not a valid, non-placeholder \
         Ed25519 public key — it is either still the placeholder, or malformed (not valid base64, \
         not 32 bytes, or not a valid Ed25519 point). Replace it with the real autoworkit.com \
         production Ed25519 public key before shipping a release build — see \
         docs/LICENSE_API.md for how the site generates and publishes it. Until it is replaced (or \
         fixed), every license verification in a release build fails closed to Free (see \
         `resolve_public_key`), so shipping like this means Pro can never be unlocked by any \
         customer."
    );
}

// --- item 2 fix: unit tests for the predicate behind the guard above. ---

#[test]
fn check_embedded_key_rejects_non_base64() {
    assert_eq!(
        check_embedded_key("not base64 at all!!"),
        EmbeddedKeyCheck::Malformed
    );
}

#[test]
fn check_embedded_key_rejects_wrong_length() {
    assert_eq!(
        check_embedded_key(&STANDARD.encode([0u8; 16])),
        EmbeddedKeyCheck::Malformed
    );
}

#[test]
fn check_embedded_key_flags_the_placeholder() {
    assert_eq!(
        check_embedded_key(EMBEDDED_PUBLIC_KEY_B64),
        EmbeddedKeyCheck::Placeholder
    );
}

#[test]
fn check_embedded_key_accepts_a_wellformed_non_placeholder_key() {
    assert_eq!(
        check_embedded_key(&test_key_b64()),
        EmbeddedKeyCheck::ValidNonPlaceholder
    );
}

#[test]
fn a_real_key_is_not_the_placeholder() {
    assert!(!is_placeholder(&test_key_b64()));
}

#[test]
fn release_build_with_placeholder_and_no_override_fails_closed() {
    assert_eq!(resolve_public_key_for(false, None), None);
}

#[test]
fn release_build_ignores_the_env_override_entirely() {
    // Even if an override value happens to be present in the environment, a
    // release build must never read it — the branch that would read it does
    // not exist for `debug_build: false`.
    let real_key = test_key_b64();
    assert_eq!(resolve_public_key_for(false, Some(&real_key)), None);
}

#[test]
fn debug_build_with_placeholder_and_no_override_uses_the_inert_placeholder() {
    // Not `None` (that's release-only fail-closed) — but it's the
    // placeholder key, which cannot verify any real token.
    let key = resolve_public_key_for(true, None).expect("debug build resolves a key");
    assert_eq!(key.to_bytes(), placeholder_key_bytes());
}

#[test]
fn debug_build_with_valid_override_uses_the_override() {
    let real_key_b64 = test_key_b64();
    let key = resolve_public_key_for(true, Some(&real_key_b64)).expect("override should resolve");
    assert_eq!(STANDARD.encode(key.to_bytes()), real_key_b64);
}

#[test]
fn debug_build_with_malformed_override_falls_back_to_embedded() {
    let key =
        resolve_public_key_for(true, Some("not valid base64!!")).expect("falls back to embedded");
    assert_eq!(key.to_bytes(), placeholder_key_bytes());
}

#[test]
fn decode_public_key_rejects_wrong_length() {
    assert!(decode_public_key(&STANDARD.encode([0u8; 16])).is_none());
    assert!(decode_public_key("not base64 at all!!").is_none());
}
