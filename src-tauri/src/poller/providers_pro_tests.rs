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
fn cursor_success_maps_ok() {
    assert_eq!(
        cursor_outcome_status("cursor-user", "cursor-user", Ok(())),
        "ok"
    );
}
