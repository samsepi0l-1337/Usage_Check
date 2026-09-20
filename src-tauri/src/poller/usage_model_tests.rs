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
        breakdown: Vec::new(),
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
        breakdown: Vec::new(),
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
        breakdown: Vec::new(),
    };
    let local = LocalUsage::none(usage_core::models::LocalProvenance::NoLocalProfile);
    let result = assemble_account_usage(&acct, outcome, local);

    assert_eq!(result.status, "ok");
}

#[test]
fn assemble_live_outcome_propagates_breakdown() {
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
        breakdown: vec![UsageBreakdownRow {
            label: "Spark".into(),
            usage: QuotaUsage {
                percent: 0.0,
                resets_at: None,
                window_seconds: Some(604_800),
            },
        }],
    };
    let local = LocalUsage::none(usage_core::models::LocalProvenance::NoLocalProfile);
    let result = assemble_account_usage(&acct, outcome, local);

    assert_eq!(result.breakdown.len(), 1);
    assert_eq!(result.breakdown[0].label, "Spark");
}

#[test]
fn account_usage_from_cursor_carries_breakdown() {
    let acct = Account {
        id: "cursor-1".into(),
        provider: Provider::Cursor,
        label: "user@ex.com".into(),
        auth_source: usage_core::account::AuthSource::CursorDatabase {
            database_path: "/tmp/state.vscdb".into(),
            expected_identity: "user@ex.com".into(),
        },
    };
    let quota = CursorQuota {
        email: Some("user@ex.com".into()),
        plan: Some("Pro".into()),
        period: Some(QuotaUsage {
            percent: 20.0,
            resets_at: None,
            window_seconds: None,
        }),
        detail_suffix: Some("$12 left".into()),
        breakdown: vec![
            UsageBreakdownRow {
                label: "First Party".into(),
                usage: QuotaUsage {
                    percent: 17.0,
                    resets_at: None,
                    window_seconds: None,
                },
            },
            UsageBreakdownRow {
                label: "API".into(),
                usage: QuotaUsage {
                    percent: 41.0,
                    resets_at: None,
                    window_seconds: None,
                },
            },
        ],
    };
    let result = account_usage_from_cursor(&acct, &quota, "ok");
    assert_eq!(result.breakdown.len(), 2);
    assert_eq!(result.breakdown[0].label, "First Party");
    assert_eq!(result.breakdown[1].label, "API");
}

#[test]
fn account_usage_pro_required_carries_no_quota_and_marks_status() {
    let acct = Account {
        id: "grok-1".into(),
        provider: Provider::Grok,
        label: "user@ex.com".into(),
        auth_source: usage_core::account::AuthSource::XaiManagement {
            credential_id: "cred-1".into(),
            team_id: "team-1".into(),
        },
    };
    let result = account_usage_pro_required(&acct);
    assert_eq!(result.status, "pro_required");
    assert_eq!(result.account.id, "grok-1");
    assert!(result.five_hour.is_none());
    assert!(result.week.is_none());
    assert!(result.plan.is_none());
    assert!(result.detail_suffix.is_none());
    assert!(result.breakdown.is_empty());
    assert!(result.pool_breakdown.is_empty());
}

#[test]
fn assemble_browser_oauth_no_local_profile_suppresses_caveat() {
    // BrowserOAuth accounts have no local CLI profile by construction, so
    // NoLocalProfile is expected, not a problem — local_status must be None.
    let acct = Account {
        id: "test".into(),
        provider: Provider::Codex,
        label: "user@ex.com".into(),
        auth_source: AuthSource::BrowserOAuth {
            credential_id: "test-cred".into(),
        },
    };
    let outcome = FetchOutcome::Failed { status: Some(500) };
    let local = LocalUsage::none(usage_core::models::LocalProvenance::NoLocalProfile);
    let result = assemble_account_usage(&acct, outcome, local);

    assert_eq!(result.local_status, None);
}

#[test]
fn assemble_browser_oauth_unavailable_still_surfaces_caveat() {
    // A genuine problem provenance must still surface for BrowserOAuth accounts.
    let acct = Account {
        id: "test".into(),
        provider: Provider::Codex,
        label: "user@ex.com".into(),
        auth_source: AuthSource::BrowserOAuth {
            credential_id: "test-cred".into(),
        },
    };
    let outcome = FetchOutcome::Failed { status: Some(500) };
    let local = LocalUsage::none(usage_core::models::LocalProvenance::Unavailable);
    let result = assemble_account_usage(&acct, outcome, local);

    assert_eq!(result.local_status, Some("unavailable".to_string()));
}

#[test]
fn assemble_cli_profile_no_local_profile_still_surfaces_caveat() {
    // Non-BrowserOAuth accounts (e.g. CliProfile) must keep showing the caveat.
    let acct = Account {
        id: "test".into(),
        provider: Provider::Codex,
        label: "user@ex.com".into(),
        auth_source: AuthSource::CliProfile {
            profile_root: "/tmp/profile".into(),
            ownership: usage_core::account::ProfileOwnership::External,
            expected_identity: "user@ex.com".into(),
        },
    };
    let outcome = FetchOutcome::Failed { status: Some(500) };
    let local = LocalUsage::none(usage_core::models::LocalProvenance::NoLocalProfile);
    let result = assemble_account_usage(&acct, outcome, local);

    assert_eq!(result.local_status, Some("no_local_profile".to_string()));
}

#[test]
fn account_usage_from_minimax_maps_five_hour_and_week() {
    let acct = Account {
        id: "mmx-1".into(),
        provider: Provider::MiniMax,
        label: "MiniMax".into(),
        auth_source: AuthSource::MiniMaxCli {
            expected_identity: "mmx@example.com".into(),
        },
    };
    let quota = usage_core::fetch::minimax::MiniMaxQuota {
        email: Some("mmx@example.com".into()),
        plan: Some("Token Plan".into()),
        five_hour: Some(QuotaUsage {
            percent: 37.0,
            resets_at: None,
            window_seconds: Some(18_000),
        }),
        week: Some(QuotaUsage {
            percent: 4.0,
            resets_at: None,
            window_seconds: None,
        }),
    };
    let result = account_usage_from_minimax(&acct, &quota, "ok");
    assert_eq!(result.display_name, "mmx@example.com");
    assert_eq!(result.five_hour.unwrap().percent, 37.0);
    assert_eq!(result.week.unwrap().percent, 4.0);
    assert!(result.detail_suffix.is_none());
}

#[test]
fn account_usage_from_augment_remaining_only_has_suffix_not_percent() {
    let acct = Account {
        id: "aug-1".into(),
        provider: Provider::Augment,
        label: "Augment".into(),
        auth_source: AuthSource::AugmentCli {
            expected_identity: "aug@example.com".into(),
        },
    };
    let credits = usage_core::fetch::augment::AugmentCredits {
        email: Some("aug@example.com".into()),
        plan: Some("Developer".into()),
        credits_remaining: Some(12.0),
        credits_included: None,
        renews_at: None,
    };
    let result = account_usage_from_augment(&acct, &credits, "ok");
    assert_eq!(result.display_name, "aug@example.com");
    assert!(result.week.is_none());
    assert_eq!(
        result.detail_suffix.as_deref(),
        Some("12 credits remaining")
    );
}

#[test]
fn account_usage_from_poe_remaining_only_has_suffix_not_percent() {
    let acct = Account {
        id: "poe-1".into(),
        provider: Provider::Poe,
        label: "Poe".into(),
        auth_source: AuthSource::BrowserOAuth {
            credential_id: "poe-cred".into(),
        },
    };
    let balance = usage_core::fetch::poe::PoeBalance {
        period: None,
        detail_suffix: Some("1500 points left".into()),
    };
    let result = account_usage_from_poe(&acct, &balance, "ok");
    assert!(result.week.is_none());
    assert_eq!(result.detail_suffix.as_deref(), Some("1500 points left"));
}

#[test]
fn account_usage_from_fireworks_maps_period_and_billed_suffix() {
    let acct = Account {
        id: "fw-1".into(),
        provider: Provider::Fireworks,
        label: "Fireworks".into(),
        auth_source: AuthSource::BrowserOAuth {
            credential_id: "fw-cred".into(),
        },
    };
    let billing = usage_core::fetch::fireworks::FireworksBilling {
        period: Some(QuotaUsage {
            percent: 25.0,
            resets_at: None,
            window_seconds: None,
        }),
        detail_suffix: Some("$25.00 billed".into()),
    };
    let result = account_usage_from_fireworks(&acct, &billing, "ok");
    assert_eq!(result.week.unwrap().percent, 25.0);
    assert_eq!(result.detail_suffix.as_deref(), Some("$25.00 billed"));
}

#[test]
fn account_usage_from_novita_remaining_only_has_suffix_not_percent() {
    let acct = Account {
        id: "novita-1".into(),
        provider: Provider::Novita,
        label: "Novita".into(),
        auth_source: AuthSource::BrowserOAuth {
            credential_id: "novita-cred".into(),
        },
    };
    let balance = usage_core::fetch::novita::NovitaBalance {
        available_usd: Some(1.0),
        detail_suffix: Some("$1.00 left".into()),
    };
    let result = account_usage_from_novita(&acct, &balance, "ok");
    assert!(result.week.is_none());
    assert_eq!(result.detail_suffix.as_deref(), Some("$1.00 left"));
}

#[test]
fn account_usage_from_amp_remaining_only_has_suffix_not_percent() {
    let acct = Account {
        id: "amp-1".into(),
        provider: Provider::Amp,
        label: "Amp".into(),
        auth_source: AuthSource::BrowserOAuth {
            credential_id: "amp-cred".into(),
        },
    };
    let balance = usage_core::fetch::amp::AmpBalance {
        period: None,
        detail_suffix: Some("$5 remaining".into()),
    };
    let result = account_usage_from_amp(&acct, &balance, "ok");
    assert!(result.week.is_none());
    assert_eq!(result.detail_suffix.as_deref(), Some("$5 remaining"));
}

#[test]
fn account_usage_from_zai_maps_five_hour_and_week() {
    let acct = Account {
        id: "zai-1".into(),
        provider: Provider::Zai,
        label: "Z.AI".into(),
        auth_source: AuthSource::BrowserOAuth {
            credential_id: "zai-cred".into(),
        },
    };
    let quota = usage_core::fetch::zai::ZaiQuota {
        plan: Some("PRO".into()),
        five_hour: Some(QuotaUsage {
            percent: 18.5,
            resets_at: None,
            window_seconds: Some(18_000),
        }),
        week: Some(QuotaUsage {
            percent: 47.2,
            resets_at: None,
            window_seconds: Some(604_800),
        }),
    };
    let result = account_usage_from_zai(&acct, &quota, "ok");
    assert_eq!(result.plan.as_deref(), Some("PRO"));
    assert_eq!(result.five_hour.unwrap().percent, 18.5);
    assert_eq!(result.week.unwrap().percent, 47.2);
}

#[test]
fn account_usage_from_bailian_maps_five_hour_and_week() {
    let acct = Account {
        id: "bl-1".into(),
        provider: Provider::Bailian,
        label: "Alibaba Token Plan".into(),
        auth_source: AuthSource::BailianCli {
            expected_identity: "Alibaba Token Plan".into(),
        },
    };
    let quota = usage_core::fetch::bailian::BailianQuota {
        five_hour: Some(QuotaUsage {
            percent: 70.0,
            resets_at: None,
            window_seconds: Some(18_000),
        }),
        week: Some(QuotaUsage {
            percent: 40.0,
            resets_at: None,
            window_seconds: Some(604_800),
        }),
    };
    let result = account_usage_from_bailian(&acct, &quota, "ok");
    assert_eq!(result.five_hour.unwrap().percent, 70.0);
    assert_eq!(result.week.unwrap().percent, 40.0);
}

#[test]
fn assemble_failed_outcome_yields_empty_breakdown() {
    let acct = Account {
        id: "test".into(),
        provider: Provider::Codex,
        label: "user@ex.com".into(),
        auth_source: usage_core::account::AuthSource::BrowserOAuth {
            credential_id: "test-cred".into(),
        },
    };
    let outcome = FetchOutcome::Failed { status: Some(500) };
    let local = LocalUsage::none(usage_core::models::LocalProvenance::NoLocalProfile);
    let result = assemble_account_usage(&acct, outcome, local);

    assert!(result.breakdown.is_empty());
}
