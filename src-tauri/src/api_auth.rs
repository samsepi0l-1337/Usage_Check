//! Optional bearer-token auth for the local API (`USAGECHECK_API_TOKEN`).
//!
//! When the env var is unset/empty the API stays open (its localhost-only bind
//! is the default protection). When set, every data endpoint requires an
//! `Authorization: Bearer <token>` header; the liveness/discovery paths
//! (`/health`, `/`) stay open so monitoring works without the token.

/// Configured bearer token from `USAGECHECK_API_TOKEN`, trimmed. `None` when
/// unset or empty — the API then stays open.
pub(crate) fn configured_token() -> Option<String> {
    std::env::var("USAGECHECK_API_TOKEN")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Liveness/discovery paths that never require auth.
pub(crate) fn requires_auth(path: &str) -> bool {
    !matches!(path, "/health" | "/")
}

/// Whether a request to `path` bearing `auth_header` (the raw `Authorization`
/// value, if present) is allowed, reading the configured token from env.
pub(crate) fn check(path: &str, auth_header: Option<&str>) -> bool {
    !requires_auth(path) || authorized(configured_token().as_deref(), auth_header)
}

/// Whether a request is authorized given the optionally-configured `token` and
/// the raw `Authorization` header value. Open (`token` is `None`) => allowed.
pub(crate) fn authorized(token: Option<&str>, auth_header: Option<&str>) -> bool {
    let Some(expected) = token else {
        return true;
    };
    auth_header
        .and_then(bearer_value)
        .map(|provided| constant_time_eq(provided.as_bytes(), expected.as_bytes()))
        .unwrap_or(false)
}

/// Extracts the token from a `Bearer <token>` header value. The scheme is
/// matched case-insensitively (`Bearer`/`bearer`/`BEARER`/...); the token is
/// trimmed of surrounding whitespace.
fn bearer_value(header: &str) -> Option<&str> {
    let (scheme, rest) = header.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| rest.trim())
}

/// Length-checked constant-time byte comparison. Not perfectly constant-time
/// (it reveals length inequality early), but adequate for a localhost opt-in
/// token and free of the trivial short-circuit-on-first-byte timing leak.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
#[path = "api_auth_tests.rs"]
mod tests;
