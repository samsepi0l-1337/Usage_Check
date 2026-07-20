use super::*;

#[test]
fn open_when_no_token_configured() {
    assert!(authorized(None, None));
    assert!(authorized(None, Some("Bearer whatever")));
}

#[test]
fn requires_matching_bearer_token_when_configured() {
    assert!(authorized(Some("secret"), Some("Bearer secret")));
    assert!(authorized(Some("secret"), Some("bearer secret"))); // case-insensitive scheme
    assert!(authorized(Some("secret"), Some("BEARER secret"))); // any casing
    assert!(authorized(Some("secret"), Some("Bearer  secret "))); // trims token
    assert!(!authorized(Some("secret"), Some("Bearer wrong")));
    assert!(!authorized(Some("secret"), Some("secret"))); // missing scheme
    assert!(!authorized(Some("secret"), None));
    assert!(!authorized(Some("secret"), Some("Bearer sekret"))); // same length, wrong value
}

#[test]
fn liveness_paths_never_require_auth() {
    assert!(!requires_auth("/health"));
    assert!(!requires_auth("/"));
    assert!(requires_auth("/v1/usage"));
    assert!(requires_auth("/metrics"));
    assert!(requires_auth("/v1/alerts"));
}

// Single test that mutates the process-global env var, so it can never race a
// sibling test also toggling USAGECHECK_API_TOKEN.
#[test]
fn check_reads_env_token() {
    std::env::remove_var("USAGECHECK_API_TOKEN");
    // No token configured: every path passes.
    assert!(check("/v1/usage", None));
    assert!(check("/health", None));

    std::env::set_var("USAGECHECK_API_TOKEN", "topsecret");
    assert!(!check("/v1/usage", None));
    assert!(!check("/v1/usage", Some("Bearer nope")));
    assert!(check("/v1/usage", Some("Bearer topsecret")));
    // Liveness endpoints stay open even with a token configured.
    assert!(check("/health", None));
    assert!(check("/", None));

    std::env::remove_var("USAGECHECK_API_TOKEN");
}
