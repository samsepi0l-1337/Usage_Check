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
fn amp_and_zai_status_maps() {
    assert_eq!(amp_status(Some(401)), "needs_login");
    assert_eq!(amp_status(Some(403)), "needs_login");
    assert_eq!(amp_status(Some(429)), "throttled");
    assert_eq!(amp_status(Some(500)), "experimental_error");
    assert_eq!(zai_status(Some(401)), "needs_login");
    assert_eq!(zai_status(Some(403)), "needs_setup");
    assert_eq!(zai_status(Some(404)), "needs_setup");
    assert_eq!(zai_status(Some(429)), "throttled");
    assert_eq!(zai_status(Some(500)), "error");
}

#[test]
fn copilot_and_windsurf_status_maps() {
    assert_eq!(copilot_status(Some(401)), "needs_login");
    assert_eq!(copilot_status(Some(403)), "needs_login");
    assert_eq!(copilot_status(Some(404)), "needs_setup");
    assert_eq!(copilot_status(Some(429)), "throttled");
    assert_eq!(copilot_status(Some(500)), "experimental_error");
    assert_eq!(
        windsurf_status("ws-user", "ws-user", Err(Some(401))),
        "needs_login"
    );
    assert_eq!(
        windsurf_status("ws-user", "ws-user", Err(Some(500))),
        "experimental_error"
    );
    assert_eq!(windsurf_status("ws-a", "ws-b", Ok(())), "identity_changed");
    assert_eq!(
        windsurf_quota_status(&WindsurfQuota::default()),
        "experimental_error"
    );
    let filled = WindsurfQuota {
        week: Some(usage_core::models::QuotaUsage {
            percent: 10.0,
            resets_at: None,
            window_seconds: None,
        }),
        ..WindsurfQuota::default()
    };
    assert_eq!(windsurf_quota_status(&filled), "ok");
}

#[test]
fn trae_and_kiro_and_factory_status_maps() {
    assert_eq!(
        trae_status("trae-user", "trae-user", Err(Some(401))),
        "needs_login"
    );
    assert_eq!(
        trae_status("trae-user", "trae-user", Err(Some(500))),
        "experimental_error"
    );
    assert_eq!(trae_status("a", "b", Ok(())), "identity_changed");
    assert_eq!(
        trae_quota_status(&TraeQuota::default()),
        "experimental_error"
    );
    assert_eq!(kiro_http_status(Some(401), false), "needs_login");
    assert_eq!(kiro_http_status(Some(404), true), "needs_setup");
    assert_eq!(kiro_http_status(Some(500), false), "experimental_error");
    assert_eq!(factory_status(Some(401)), "needs_login");
    assert_eq!(factory_status(Some(500)), "experimental_error");
}

#[test]
fn cursor_success_maps_ok() {
    assert_eq!(
        cursor_outcome_status("cursor-user", "cursor-user", Ok(())),
        "ok"
    );
}
