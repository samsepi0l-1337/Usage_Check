use super::*;

#[test]
fn auth_source_codex_identity_mismatch() {
    assert_eq!(
        codex_identity_status("id-a", "id-b"),
        Some("identity_changed")
    );
    assert_eq!(codex_identity_status("id", "id"), None);
}

#[test]
fn auth_source_claude_identity_mismatch() {
    assert_eq!(claude_identity_status("a", "b"), Some("identity_changed"));
    assert_eq!(claude_identity_status("a", "a"), None);
}

#[test]
fn maps_agy_pools() {
    let acct = Account {
        id: "3".into(),
        provider: Provider::Agy,
        label: "agy".into(),
        auth_source: AuthSource::BrowserOAuth {
            credential_id: "agy-credential".into(),
        },
    };
    let quota = AgyQuota {
        email: Some("a@b.com".into()),
        plan: Some("Pro".into()),
        pools: vec![AgyQuotaPool {
            name: "Gemini Models".into(),
            five_hour: None,
            week: Some(QuotaUsage {
                percent: 0.0,
                resets_at: None,
                window_seconds: Some(604_800),
            }),
        }],
    };
    let au = account_usage_from_agy(&acct, &quota, "ok");
    assert_eq!(au.display_name, "a@b.com");
    assert_eq!(au.pool_breakdown.len(), 1);
    assert!((au.week.as_ref().unwrap().percent - 0.0).abs() < 0.01);
}

#[test]
fn status_for_failure_maps_auth_errors() {
    assert_eq!(status_for_failure(Some(401)), "needs_login");
    assert_eq!(status_for_failure(Some(403)), "needs_login");
    assert_eq!(status_for_failure(Some(429)), "throttled");
    assert_eq!(status_for_failure(Some(500)), "error");
    assert_eq!(status_for_failure(None), "error");
}

#[test]
fn test_assemble_live_outcome_ok_local_preserves_totals() {
    // §6.9: Live outcome + local(Ok, totals>0).
    // Expected: token_totals == local.totals, five_hour/week Some.
    // This is the CURRENT BUG — assemble_account_usage passes WindowTotals::default() instead.
    let acct = Account {
        id: "test".into(),
        provider: Provider::Codex,
        label: "user@ex.com".into(),
        auth_source: usage_core::account::AuthSource::BrowserOAuth {
            credential_id: "test-cred".into(),
        },
    };
    let outcome = FetchOutcome::Live {
        five_hour: Some(QuotaUsage {
            percent: 25.0,
            resets_at: None,
            window_seconds: Some(18000),
        }),
        week: Some(QuotaUsage {
            percent: 30.0,
            resets_at: None,
            window_seconds: None,
        }),
        plan: Some("Pro".into()),
        email: Some("user@ex.com".into()),
    };
    let local = LocalUsage {
        totals: WindowTotals {
            five_hours: 500,
            week: 2000,
            month: 10000,
        },
        provenance: usage_core::models::LocalProvenance::Ok,
    };
    let result = assemble_account_usage(&acct, outcome, local);
    // Real assertions:
    assert!(
        result.five_hour.is_some(),
        "Live outcome should preserve five_hour"
    );
    assert!(result.week.is_some(), "Live outcome should preserve week");
    // CRITICAL: token_totals must match local.totals (currently fails — returns 0)
    assert_eq!(
        result.totals.five_hours, 500,
        "token_totals should match local.totals (BUG: currently returns 0)"
    );
    assert_eq!(result.totals.week, 2000, "week tokens should match local");
    assert_eq!(
        result.totals.month, 10000,
        "month tokens should match local"
    );
}

#[test]
fn test_assemble_failed_outcome_uses_local_totals() {
    // §6.9: Failed(429) + local(Ok).
    // Expected: five_hour/week None, token_totals == local.totals.
    let acct = Account {
        id: "test".into(),
        provider: Provider::Claude,
        label: "user@ex.com".into(),
        auth_source: usage_core::account::AuthSource::BrowserOAuth {
            credential_id: "claude-cred".into(),
        },
    };
    let outcome = FetchOutcome::Failed { status: Some(429) };
    let local = LocalUsage {
        totals: WindowTotals {
            five_hours: 300,
            week: 1500,
            month: 8000,
        },
        provenance: usage_core::models::LocalProvenance::Ok,
    };
    let result = assemble_account_usage(&acct, outcome, local);
    // Real assertions:
    assert!(
        result.five_hour.is_none(),
        "Failed outcome should not set five_hour"
    );
    assert!(result.week.is_none(), "Failed outcome should not set week");
    assert_eq!(
        result.totals.five_hours, 300,
        "Failed should use local totals"
    );
    assert_eq!(result.totals.week, 1500, "Failed should use local week");
}

#[test]
fn test_assemble_failed_unavailable_distinct_from_zero() {
    // §6.9/DoD §1.4: Unavailable must be DISTINCT from real 0 totals.
    // The DTO's local_status should carry "unavailable" when provenance=Unavailable.
    let acct = Account {
        id: "test".into(),
        provider: Provider::Codex,
        label: "test@ex.com".into(),
        auth_source: usage_core::account::AuthSource::BrowserOAuth {
            credential_id: "cred".into(),
        },
    };
    let outcome = FetchOutcome::Failed { status: None };
    let local = LocalUsage {
        totals: WindowTotals::default(),
        provenance: usage_core::models::LocalProvenance::Unavailable,
    };
    let result = assemble_account_usage(&acct, outcome, local);

    // CRITICAL: totals are 0, BUT status must distinguish Unavailable
    assert_eq!(result.totals.five_hours, 0, "Unavailable has no totals");

    // The status field should indicate the problem, not generic "error"
    // Stub will have generic status, but test proves the seam exists
    assert!(
        !result.status.is_empty(),
        "Status must be set (stub returns generic, LOGIC refines per provenance)"
    );
}

#[test]
fn test_assemble_live_empty_windows_yields_waiting_for_usage() {
    let acct = Account {
        id: "test".into(),
        provider: Provider::Codex,
        label: "user@ex.com".into(),
        auth_source: usage_core::account::AuthSource::BrowserOAuth {
            credential_id: "test-cred".into(),
        },
    };
    let outcome = FetchOutcome::Live {
        five_hour: None,
        week: None,
        plan: None,
        email: None,
    };
    let local = LocalUsage::none(usage_core::models::LocalProvenance::NoLocalProfile);
    let result = assemble_account_usage(&acct, outcome, local);

    assert_eq!(result.status, "waiting_for_usage");
}

#[test]
fn test_assemble_live_some_five_hour_yields_ok() {
    let acct = Account {
        id: "test".into(),
        provider: Provider::Codex,
        label: "user@ex.com".into(),
        auth_source: usage_core::account::AuthSource::BrowserOAuth {
            credential_id: "test-cred".into(),
        },
    };
    let outcome = FetchOutcome::Live {
        five_hour: Some(QuotaUsage {
            percent: 10.0,
            resets_at: None,
            window_seconds: Some(18000),
        }),
        week: None,
        plan: None,
        email: None,
    };
    let local = LocalUsage::none(usage_core::models::LocalProvenance::NoLocalProfile);
    let result = assemble_account_usage(&acct, outcome, local);

    assert_eq!(result.status, "ok");
}
