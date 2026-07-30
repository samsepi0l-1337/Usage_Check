use super::*;
use chrono::Duration;
use ed25519_dalek::SigningKey;

fn test_signing_key() -> SigningKey {
    // Fixed seed: deterministic across runs, obviously not a real key.
    SigningKey::from_bytes(&[7u8; 32])
}

fn other_signing_key() -> SigningKey {
    SigningKey::from_bytes(&[9u8; 32])
}

fn payload() -> TokenPayload {
    TokenPayload {
        v: 1,
        key_id: "key-123".into(),
        plan: "pro".into(),
        device: "device-abc".into(),
        issued_at: Utc::now() - Duration::days(1),
        expires_at: None,
    }
}

#[test]
fn a_token_signed_by_the_expected_key_verifies() {
    let signing_key = test_signing_key();
    let public_key = signing_key.verifying_key();
    let expected = payload();
    let token = encode_token(&expected, &signing_key);

    let verified = verify_token(&token, &public_key).expect("token should verify");
    assert_eq!(verified, expected);
}

#[test]
fn flipped_byte_in_payload_fails() {
    let signing_key = test_signing_key();
    let public_key = signing_key.verifying_key();
    let token = encode_token(&payload(), &signing_key);
    let (payload_b64, sig_b64) = token.split_once('.').unwrap();

    let mut bytes = URL_SAFE_NO_PAD.decode(payload_b64).unwrap();
    bytes[0] ^= 0xFF;
    let tampered = format!("{}.{}", URL_SAFE_NO_PAD.encode(&bytes), sig_b64);

    assert_eq!(
        verify_token(&tampered, &public_key),
        Err(TokenError::InvalidSignature)
    );
}

#[test]
fn flipped_byte_in_signature_fails() {
    let signing_key = test_signing_key();
    let public_key = signing_key.verifying_key();
    let token = encode_token(&payload(), &signing_key);
    let (payload_b64, sig_b64) = token.split_once('.').unwrap();

    let mut bytes = URL_SAFE_NO_PAD.decode(sig_b64).unwrap();
    bytes[0] ^= 0xFF;
    let tampered = format!("{payload_b64}.{}", URL_SAFE_NO_PAD.encode(&bytes));

    assert_eq!(
        verify_token(&tampered, &public_key),
        Err(TokenError::InvalidSignature)
    );
}

#[test]
fn token_signed_by_a_different_key_fails() {
    let signing_key = test_signing_key();
    let wrong_public_key = other_signing_key().verifying_key();
    let token = encode_token(&payload(), &signing_key);

    assert_eq!(
        verify_token(&token, &wrong_public_key),
        Err(TokenError::InvalidSignature)
    );
}

#[test]
fn malformed_token_missing_dot_fails() {
    let signing_key = test_signing_key();
    let public_key = signing_key.verifying_key();
    assert_eq!(
        verify_token("not-a-token", &public_key),
        Err(TokenError::MalformedFormat)
    );
}

#[test]
fn malformed_token_too_many_segments_fails() {
    let signing_key = test_signing_key();
    let public_key = signing_key.verifying_key();
    assert_eq!(
        verify_token("a.b.c", &public_key),
        Err(TokenError::MalformedFormat)
    );
}

#[test]
fn malformed_token_empty_segment_fails() {
    let signing_key = test_signing_key();
    let public_key = signing_key.verifying_key();
    assert_eq!(
        verify_token(".sig", &public_key),
        Err(TokenError::MalformedFormat)
    );
    assert_eq!(
        verify_token("payload.", &public_key),
        Err(TokenError::MalformedFormat)
    );
}

#[test]
fn malformed_token_bad_base64_fails() {
    let signing_key = test_signing_key();
    let public_key = signing_key.verifying_key();
    assert_eq!(
        verify_token("not!valid!base64.also!not!valid", &public_key),
        Err(TokenError::Base64)
    );
}

#[test]
fn tamper_payload_helper_keeps_signature_but_breaks_it() {
    let signing_key = test_signing_key();
    let public_key = signing_key.verifying_key();
    let token = encode_token(&payload(), &signing_key);

    let tampered = tamper_payload(&token, |v| {
        v["plan"] = serde_json::Value::String("pro".into());
        v["device"] = serde_json::Value::String("someone-elses-device".into());
    });

    // Signature segment is byte-identical; only the payload changed.
    assert_eq!(
        token.split_once('.').unwrap().1,
        tampered.split_once('.').unwrap().1
    );
    assert_eq!(
        verify_token(&tampered, &public_key),
        Err(TokenError::InvalidSignature)
    );
}

#[test]
fn unknown_extra_fields_are_ignored() {
    let signing_key = test_signing_key();
    let public_key = signing_key.verifying_key();
    let mut value = serde_json::to_value(payload()).unwrap();
    value["future_field"] = serde_json::Value::String("something new".into());
    let bytes = serde_json::to_vec(&value).unwrap();
    let signature = {
        use ed25519_dalek::Signer;
        signing_key.sign(&bytes)
    };
    let token = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&bytes),
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    );

    let verified = verify_token(&token, &public_key).expect("unknown fields must not break parsing");
    assert_eq!(verified.plan, "pro");
}
