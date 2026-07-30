//! Activation HTTP transport: `POST <endpoint>` with `{key, device,
//! app_version}`, expecting `{"token": "<segment>.<segment>"}` on success.
//! See `docs/LICENSE_API.md` for the full wire contract.
//!
//! SECURITY: never log/print the license key, the device id, or the token.

use serde::{Deserialize, Serialize};

use super::LicenseStatus;

/// Default production endpoint. Overridable at runtime via
/// `USAGECHECK_LICENSE_API` — used by the mock-server tests, and so an
/// operator can point the app at a staging server.
const DEFAULT_ENDPOINT: &str = "https://autoworkit.com/api/license/verify";
const LICENSE_API_ENV: &str = "USAGECHECK_LICENSE_API";

/// Connect / total timeouts matching the existing poller client style
/// (`poller/mod.rs`).
const CONNECT_TIMEOUT_SECS: u64 = 10;
const TOTAL_TIMEOUT_SECS: u64 = 15;

/// Why an activation/refresh attempt did not produce a usable Pro status.
/// Distinguished so a future UI (Stage C) can say something useful.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationError {
    /// Could not reach the server at all (DNS/connect/timeout/transport).
    Network(String),
    /// The server responded with a non-2xx status and a structured
    /// `{"error": ..., "message": ...}` body (or an unparsable one, in
    /// which case `code` is a generic marker and `message` describes the
    /// raw HTTP status).
    Server { code: String, message: String },
    /// The response did not parse, or the returned token failed signature
    /// or payload verification.
    InvalidToken(String),
    /// The verified token's `device` does not match this machine.
    DeviceMismatch,
    /// A 404/405 from the DEFAULT host — the endpoint is not live yet.
    EndpointNotImplemented,
    /// Verification succeeded but persisting the record to disk failed.
    Persist(String),
    /// `refresh()` was called with no stored license to refresh.
    NoStoredLicense,
    /// H1 replay guard: the server returned a token whose signed `issued_at`
    /// is not STRICTLY newer than the currently-stored token's `issued_at`.
    /// Rejected before touching the stored record — the previous record is
    /// left exactly as it was.
    ReplayedToken,
    /// H3: the candidate token verified (signature, `v`, `plan`, device) but
    /// the FULL [`super::decide_status`] evaluation of it — the same
    /// temporal/watermark/rollback rules `status()` itself uses — does not
    /// resolve to `Pro`. Nothing is persisted; the previous record (if any)
    /// is left untouched.
    NotEntitled(LicenseStatus),
    /// F5: this machine's device id could not be durably persisted to disk.
    /// Refused BEFORE contacting the server and before writing anything else
    /// — a token must never be bound to a process-only id that a restart
    /// would replace with a different one, which would silently deny the
    /// user their own activation after that restart.
    DeviceNotPersisted,
}

impl std::fmt::Display for ActivationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActivationError::Network(detail) => write!(f, "network error: {detail}"),
            ActivationError::Server { code, message } => {
                write!(f, "server error ({code}): {message}")
            }
            ActivationError::InvalidToken(detail) => write!(f, "invalid token: {detail}"),
            ActivationError::DeviceMismatch => {
                write!(f, "license is bound to a different device")
            }
            ActivationError::EndpointNotImplemented => {
                write!(f, "license API endpoint is not implemented yet")
            }
            ActivationError::Persist(detail) => write!(f, "failed to save license: {detail}"),
            ActivationError::NoStoredLicense => write!(f, "no stored license to refresh"),
            ActivationError::ReplayedToken => {
                write!(f, "server returned a non-advancing token (possible replay)")
            }
            ActivationError::NotEntitled(status) => {
                write!(f, "verified token does not currently grant Pro ({status:?})")
            }
            ActivationError::DeviceNotPersisted => {
                write!(f, "device id could not be durably saved; try again")
            }
        }
    }
}

impl std::error::Error for ActivationError {}

/// Coarse, NEVER attacker-controlled classification of an [`ActivationError`]
/// (H4) — safe to log verbatim. Unlike `Display`/`{error}`, which for
/// `Server{..}` embeds server-provided `code`/`message` text, this carries no
/// text from the error at all: an attacker-controlled server response can
/// steer which VARIANT of `ActivationError` gets produced, but never what
/// text ends up in a log line built from this classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationErrorClass {
    Network,
    ServerRejected,
    InvalidToken,
    DeviceMismatch,
    EndpointMissing,
    Persist,
    NoStoredLicense,
    ReplayedToken,
    NotEntitled,
    DeviceNotPersisted,
}

impl std::fmt::Display for ActivationErrorClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            ActivationErrorClass::Network => "network",
            ActivationErrorClass::ServerRejected => "server-rejected",
            ActivationErrorClass::InvalidToken => "invalid-token",
            ActivationErrorClass::DeviceMismatch => "device-mismatch",
            ActivationErrorClass::EndpointMissing => "endpoint-missing",
            ActivationErrorClass::Persist => "persist-failed",
            ActivationErrorClass::NoStoredLicense => "no-stored-license",
            ActivationErrorClass::ReplayedToken => "replayed-token",
            ActivationErrorClass::NotEntitled => "not-entitled",
            ActivationErrorClass::DeviceNotPersisted => "device-not-persisted",
        };
        write!(f, "{label}")
    }
}

impl ActivationError {
    /// H4: log THIS, never `self`/`{error}` — see [`ActivationErrorClass`].
    pub fn classify(&self) -> ActivationErrorClass {
        match self {
            ActivationError::Network(_) => ActivationErrorClass::Network,
            ActivationError::Server { .. } => ActivationErrorClass::ServerRejected,
            ActivationError::InvalidToken(_) => ActivationErrorClass::InvalidToken,
            ActivationError::DeviceMismatch => ActivationErrorClass::DeviceMismatch,
            ActivationError::EndpointNotImplemented => ActivationErrorClass::EndpointMissing,
            ActivationError::Persist(_) => ActivationErrorClass::Persist,
            ActivationError::NoStoredLicense => ActivationErrorClass::NoStoredLicense,
            ActivationError::ReplayedToken => ActivationErrorClass::ReplayedToken,
            ActivationError::NotEntitled(_) => ActivationErrorClass::NotEntitled,
            ActivationError::DeviceNotPersisted => ActivationErrorClass::DeviceNotPersisted,
        }
    }
}

/// Resolves the activation endpoint: `USAGECHECK_LICENSE_API` if set and
/// non-empty, else [`DEFAULT_ENDPOINT`].
pub(super) fn resolve_endpoint() -> String {
    std::env::var(LICENSE_API_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string())
}

#[derive(Serialize)]
struct ActivateRequest<'a> {
    key: &'a str,
    device: &'a str,
    app_version: &'a str,
}

#[derive(Deserialize)]
struct ActivateSuccessBody {
    token: String,
}

#[derive(Deserialize)]
struct ActivateErrorBody {
    error: String,
    message: String,
}

/// Posts the activation request and returns the raw token string on success
/// (unverified — the caller verifies the signature and payload before
/// trusting or persisting anything). Never retries automatically.
pub(super) async fn request_token(
    endpoint: &str,
    key: &str,
    device: &str,
) -> Result<String, ActivationError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(TOTAL_TIMEOUT_SECS))
        .connect_timeout(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .build()
        .map_err(|e| ActivationError::Network(e.to_string()))?;

    let body = ActivateRequest {
        key,
        device,
        app_version: env!("CARGO_PKG_VERSION"),
    };

    let resp = client
        .post(endpoint)
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| ActivationError::Network(e.to_string()))?;

    let status = resp.status();
    if status.is_success() {
        let parsed: ActivateSuccessBody = resp
            .json()
            .await
            .map_err(|_| ActivationError::InvalidToken("malformed success response body".into()))?;
        return Ok(parsed.token);
    }

    let body = resp.json::<ActivateErrorBody>().await.ok();
    Err(classify_error(status.as_u16(), endpoint, body))
}

/// Pure mapping from a non-2xx response to an [`ActivationError`] — split out
/// of [`request_token`] so it is unit-testable without a real HTTP round
/// trip (the 404/405-from-the-DEFAULT-host case can never be reproduced by a
/// test mock server, since [`DEFAULT_ENDPOINT`] is a fixed production URL).
fn classify_error(status: u16, endpoint: &str, body: Option<ActivateErrorBody>) -> ActivationError {
    if (status == 404 || status == 405) && endpoint == DEFAULT_ENDPOINT {
        return ActivationError::EndpointNotImplemented;
    }
    match body {
        Some(body) => ActivationError::Server {
            code: body.error,
            message: body.message,
        },
        None => ActivationError::Server {
            code: "http_error".to_string(),
            message: format!("HTTP {status}"),
        },
    }
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
