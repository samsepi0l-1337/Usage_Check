use super::*;

#[test]
fn cursor_identity_mismatch_maps_identity_changed() {
    assert_eq!(
        cursor_outcome_status("cursor-user-a", "cursor-user-b", Ok(())),
        "identity_changed"
    );
}

#[test]
fn cursor_rpc_failure_maps_experimental_error() {
    assert_eq!(
        cursor_outcome_status("cursor-user", "cursor-user", Err(Some(500))),
        "experimental_error"
    );
    assert_eq!(
        cursor_outcome_status("cursor-user", "cursor-user", Err(None)),
        "experimental_error"
    );
}

#[test]
fn kimi_and_opencode_status_maps() {
    assert_eq!(kimi_status(Some(401)), "needs_login");
    assert_eq!(kimi_status(Some(404)), "needs_setup");
    assert_eq!(kimi_status(Some(429)), "throttled");
    assert_eq!(kimi_status(Some(500)), "error");
    assert_eq!(opencode_status(Some(401)), "needs_login");
    assert_eq!(opencode_status(Some(403)), "needs_setup");
    assert_eq!(opencode_status(Some(429)), "throttled");
}

#[test]
fn cursor_success_maps_ok() {
    assert_eq!(
        cursor_outcome_status("cursor-user", "cursor-user", Ok(())),
        "ok"
    );
}
