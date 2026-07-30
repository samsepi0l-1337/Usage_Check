//! Compact, JWS-like signed license token: `<payload>.<signature>`, both
//! segments base64url (no padding). See `docs/LICENSE_API.md` for the wire
//! contract this implements.
//!
//! SECURITY: the signature covers the RAW BYTES of the decoded payload
//! segment — never a re-serialized value — so there is no JSON
//! canonicalization ambiguity between what was signed and what is verified.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

/// The signed payload. Unknown extra fields are ignored (forward
/// compatibility with a site that adds fields later) — this is serde's
/// default behavior; nothing extra is needed to get it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenPayload {
    pub v: u8,
    pub key_id: String,
    pub plan: String,
    pub device: String,
    pub issued_at: DateTime<Utc>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

/// Why a token failed to verify. Every variant maps to [`super::LicenseStatus::Free`]
/// in `decide_status` — this type exists for logging/diagnostics only; never
/// surface it to a user in a way that helps a forger distinguish "wrong
/// signature" from "wrong device" etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenError {
    /// Not exactly two dot-separated segments, or a segment is empty.
    MalformedFormat,
    /// A segment was not valid base64url (no padding).
    Base64,
    /// The signature segment was not 64 bytes.
    MalformedSignature,
    /// The signature does not verify against the payload bytes.
    InvalidSignature,
    /// The signature verified, but the payload bytes are not valid
    /// `TokenPayload` JSON.
    MalformedPayload,
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            TokenError::MalformedFormat => "malformed token format",
            TokenError::Base64 => "invalid base64 segment",
            TokenError::MalformedSignature => "malformed signature segment",
            TokenError::InvalidSignature => "signature verification failed",
            TokenError::MalformedPayload => "malformed payload after verification",
        };
        f.write_str(msg)
    }
}

/// Verifies `token` against `public_key` and returns the decoded payload.
///
/// Never logs or returns the raw token/signature bytes. Verification uses
/// [`VerifyingKey::verify_strict`] (rejects malleable/non-canonical
/// signatures) over the EXACT decoded payload bytes — the payload is only
/// parsed as JSON after the signature has already been checked against those
/// same bytes, so nothing about the trusted result depends on a
/// re-serialization of the JSON.
pub fn verify_token(token: &str, public_key: &VerifyingKey) -> Result<TokenPayload, TokenError> {
    let mut parts = token.split('.');
    let payload_b64 = parts.next().ok_or(TokenError::MalformedFormat)?;
    let sig_b64 = parts.next().ok_or(TokenError::MalformedFormat)?;
    if parts.next().is_some() || payload_b64.is_empty() || sig_b64.is_empty() {
        return Err(TokenError::MalformedFormat);
    }

    let payload_bytes = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| TokenError::Base64)?;
    let sig_bytes = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|_| TokenError::Base64)?;
    let sig_array: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| TokenError::MalformedSignature)?;
    let signature = Signature::from_bytes(&sig_array);

    public_key
        .verify_strict(&payload_bytes, &signature)
        .map_err(|_| TokenError::InvalidSignature)?;

    serde_json::from_slice::<TokenPayload>(&payload_bytes).map_err(|_| TokenError::MalformedPayload)
}

#[cfg(test)]
pub(crate) fn encode_token(payload: &TokenPayload, signing_key: &ed25519_dalek::SigningKey) -> String {
    use ed25519_dalek::Signer;
    let payload_bytes = serde_json::to_vec(payload).expect("serialize test token payload");
    let signature = signing_key.sign(&payload_bytes);
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&payload_bytes),
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    )
}

/// Test helper: decodes a token's payload segment, applies `mutate` to the
/// parsed JSON, and re-encodes it WITHOUT re-signing — i.e. it keeps the
/// original signature segment unchanged. This is the "someone hand-edited
/// the cache file" shape the whole stage exists to defend against: the
/// resulting string is syntactically a valid token, but its signature no
/// longer matches the (mutated) payload bytes, so [`verify_token`] must
/// reject it.
#[cfg(test)]
pub(crate) fn tamper_payload(token: &str, mutate: impl FnOnce(&mut serde_json::Value)) -> String {
    let (payload_b64, sig_b64) = token.split_once('.').expect("well-formed test token");
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .expect("valid base64 test payload");
    let mut value: serde_json::Value =
        serde_json::from_slice(&payload_bytes).expect("valid json test payload");
    mutate(&mut value);
    let tampered_bytes = serde_json::to_vec(&value).expect("re-serialize tampered payload");
    format!("{}.{}", URL_SAFE_NO_PAD.encode(&tampered_bytes), sig_b64)
}

#[cfg(test)]
#[path = "token_tests.rs"]
mod tests;
