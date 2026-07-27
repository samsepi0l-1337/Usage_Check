    use super::*;

    #[test]
    fn spec_for_event_resolves_registry_actions() {
        let spec = spec_for_event("add-codex-oauth").expect("Codex OAuth is registered");
        assert_eq!(spec.provider, Provider::Codex);
        assert_eq!(spec.method, AuthMethod::BrowserOAuth);

        let spec = spec_for_event("add-claude-cli").expect("Claude CLI is registered");
        assert_eq!(spec.provider, Provider::Claude);
        assert_eq!(spec.method, AuthMethod::Cli);
    }

    #[test]
    fn spec_for_event_rejects_dead_and_unknown() {
        assert!(spec_for_event("add-higgsfield-login").is_none());
        assert!(spec_for_event("add-unknown-provider").is_none());
        assert!(spec_for_event("refresh").is_none());
    }

    #[cfg(feature = "edition-pro")]
    #[test]
    fn spec_for_event_resolves_pro_registry_actions() {
        let spec = spec_for_event("add-grok-clipboard").expect("Grok clipboard is registered");
        assert_eq!(spec.provider, Provider::Grok);
        assert_eq!(spec.method, AuthMethod::ManagementKeyClipboard);

        let spec = spec_for_event("add-higgsfield-cli").expect("Higgsfield CLI is registered");
        assert_eq!(spec.provider, Provider::Higgsfield);
        assert_eq!(spec.method, AuthMethod::Cli);
    }

    use crate::poller::AccountUsage;
    use usage_core::account::{Account, AuthSource};
    use usage_core::fetch::agy::AgyQuotaPool;
    use usage_core::models::{QuotaUsage, UsageBreakdownRow, WindowTotals};

    fn quota(percent: f64) -> QuotaUsage {
        QuotaUsage {
            percent,
            resets_at: None,
            window_seconds: Some(18_000),
        }
    }

    fn usage(provider: Provider, five: Option<f64>, week: Option<f64>) -> AccountUsage {
        AccountUsage {
            account: Account {
                id: "id".into(),
                provider,
                label: "label".into(),
                auth_source: AuthSource::BrowserOAuth {
                    credential_id: "c".into(),
                },
            },
            display_name: "acct".into(),
            plan: None,
            five_hour: five.map(quota),
            week: week.map(quota),
            totals: WindowTotals::default(),
            pool_breakdown: Vec::new(),
            breakdown: Vec::new(),
            detail_suffix: None,
            status: "ok".into(),
            local_status: None,
        }
    }

    #[test]
    fn account_max_percent_takes_highest_finite_window() {
        assert_eq!(
            account_max_percent(&usage(Provider::Codex, Some(40.0), Some(80.0))),
            Some(80.0)
        );
        assert_eq!(account_max_percent(&usage(Provider::Codex, None, None)), None);
        // NaN windows are ignored.
        assert_eq!(
            account_max_percent(&usage(Provider::Codex, Some(f64::NAN), Some(12.0))),
            Some(12.0)
        );
    }

    #[test]
    fn account_max_percent_includes_pool_windows() {
        let mut u = usage(Provider::Agy, None, None);
        u.pool_breakdown = vec![AgyQuotaPool {
            name: "Gemini".into(),
            five_hour: None,
            week: Some(quota(97.0)),
        }];
        assert_eq!(account_max_percent(&u), Some(97.0));
    }

    #[test]
    fn account_max_percent_includes_breakdown_rows() {
        let mut u = usage(Provider::Claude, Some(35.0), Some(30.0));
        u.breakdown = vec![UsageBreakdownRow {
            label: "Fable".into(),
            usage: quota(92.0),
        }];
        assert_eq!(account_max_percent(&u), Some(92.0));
    }

    #[test]
    fn format_breakdown_row_renders_label_and_percent() {
        let row = UsageBreakdownRow {
            label: "Fable".into(),
            usage: quota(28.0),
        };
        assert_eq!(format_breakdown_row(&row), "Fable 28%");
    }

    #[test]
    fn format_breakdown_row_renders_present_and_zero() {
        let row = UsageBreakdownRow {
            label: "Spark".into(),
            usage: quota(0.0),
        };
        assert_eq!(format_breakdown_row(&row), "Spark 0%");
    }

    #[test]
    fn updated_label_formats_local_hms() {
        use chrono::TimeZone;
        let ts = chrono::Utc.timestamp_opt(1_700_000_000, 0).single().unwrap();
        let label = updated_label(ts);
        assert!(label.starts_with("Updated "));
        // HH:MM:SS after the prefix (8 chars, colon-separated).
        let time = label.trim_start_matches("Updated ");
        let parts: Vec<&str> = time.split(':').collect();
        assert_eq!(parts.len(), 3, "expected HH:MM:SS, got {time}");
        assert!(parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_digit())));
    }

    #[test]
    fn near_limit_count_counts_accounts_at_or_above_threshold() {
        let usages = vec![
            usage(Provider::Codex, Some(95.0), None),  // near
            usage(Provider::Claude, Some(90.0), None), // near (inclusive)
            usage(Provider::Codex, Some(50.0), None),  // not near
            usage(Provider::Codex, None, None),        // no data -> not near
        ];
        assert_eq!(near_limit_count(&usages, 90.0), 2);
        assert_eq!(near_limit_count(&usages, 96.0), 0);
    }

    #[test]
    fn format_usage_detail_omits_token_totals_when_live_quota_present() {
        let mut u = usage(Provider::Codex, Some(1.0), None);
        u.five_hour = Some(QuotaUsage {
            percent: 1.0,
            resets_at: Some(chrono::Utc::now() + chrono::Duration::seconds(513_000)),
            window_seconds: Some(18_000),
        });
        u.totals = WindowTotals {
            five_hours: 0,
            week: 3_022_300_000,
            month: 0,
        };
        let line = format_usage_detail(&u);
        assert_eq!(line, "1% · resets 5d 22h");
    }

    #[test]
    fn format_usage_detail_falls_back_to_token_totals_without_live_quota() {
        let mut u = usage(Provider::Codex, None, None);
        u.totals = WindowTotals {
            five_hours: 12_000,
            week: 3_022_300_000,
            month: 0,
        };
        let line = format_usage_detail(&u);
        assert_eq!(line, "5h 12.0k · 7d 3022.3M");
    }

    #[test]
    fn test_auth_specs_no_forbidden_substrings() {
        let specs = auth_action_specs();
        let forbidden = [
            "Gemini (CLI)",
            "Antigravity (CLI)",
            "Cursor (CLI)",
            "Grok (CLI)",
            "SuperGrok",
            "Higgsfield (browser)",
        ];
        for spec in specs {
            for forbidden_str in &forbidden {
                assert!(
                    !spec.label.contains(forbidden_str),
                    "Forbidden substring '{}' in '{}'",
                    forbidden_str,
                    spec.label
                );
            }
        }
    }

    #[test]
    fn account_usage_lines_splits_both_windows_with_own_resets() {
        let mut u = usage(Provider::Claude, Some(12.0), Some(66.0));
        u.five_hour = Some(QuotaUsage {
            percent: 12.0,
            resets_at: Some(chrono::Utc::now() + chrono::Duration::seconds(8_160)), // ~2h16m
            window_seconds: None,
        });
        u.week = Some(QuotaUsage {
            percent: 66.0,
            resets_at: Some(chrono::Utc::now() + chrono::Duration::seconds(526_800)), // ~6d3h
            window_seconds: None,
        });
        let lines = account_usage_lines(&u);
        assert_eq!(lines.len(), 2);
        assert!(
            lines[0].starts_with("     5h 12% · resets "),
            "row 1: {}",
            lines[0]
        );
        assert!(
            lines[1].starts_with("     7d 66% · resets "),
            "row 2: {}",
            lines[1]
        );
    }

    #[test]
    fn account_usage_lines_only_five_hour_has_reset() {
        let mut u = usage(Provider::Claude, Some(12.0), Some(66.0));
        u.five_hour = Some(QuotaUsage {
            percent: 12.0,
            resets_at: Some(chrono::Utc::now() + chrono::Duration::seconds(8_160)),
            window_seconds: None,
        });
        u.week = Some(QuotaUsage {
            percent: 66.0,
            resets_at: None,
            window_seconds: None,
        });
        let lines = account_usage_lines(&u);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("· resets"), "row 1: {}", lines[0]);
        assert_eq!(lines[1], "     7d 66%", "row 2 should have no reset suffix");
    }

    #[test]
    fn account_usage_lines_only_week_has_reset() {
        let mut u = usage(Provider::Claude, Some(12.0), Some(66.0));
        u.five_hour = Some(QuotaUsage {
            percent: 12.0,
            resets_at: None,
            window_seconds: None,
        });
        u.week = Some(QuotaUsage {
            percent: 66.0,
            resets_at: Some(chrono::Utc::now() + chrono::Duration::seconds(526_800)),
            window_seconds: None,
        });
        let lines = account_usage_lines(&u);
        assert_eq!(lines.len(), 2);
        assert!(!lines[0].contains("· resets"), "row 1: {}", lines[0]);
        assert!(lines[1].contains("· resets"), "row 2: {}", lines[1]);
    }

    #[test]
    fn account_usage_lines_reset_values_are_source_distinct() {
        let mut u = usage(Provider::Claude, Some(12.0), Some(66.0));
        u.five_hour = Some(QuotaUsage {
            percent: 12.0,
            resets_at: Some(
                chrono::Utc::now() + chrono::Duration::days(3) + chrono::Duration::hours(5),
            ),
            window_seconds: None,
        });
        u.week = Some(QuotaUsage {
            percent: 66.0,
            resets_at: Some(
                chrono::Utc::now() + chrono::Duration::days(20) + chrono::Duration::hours(5),
            ),
            window_seconds: None,
        });
        let lines = account_usage_lines(&u);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("3d"), "row 1: {}", lines[0]);
        assert!(!lines[0].contains("20d"), "row 1: {}", lines[0]);
        assert!(lines[1].contains("20d"), "row 2: {}", lines[1]);
        assert!(!lines[1].contains("3d"), "row 2: {}", lines[1]);

        let reset_5h = lines[0]
            .split("· resets ")
            .nth(1)
            .expect("row 1 has reset suffix");
        let reset_7d = lines[1]
            .split("· resets ")
            .nth(1)
            .expect("row 2 has reset suffix");
        assert_ne!(reset_5h, reset_7d, "reset text should differ between windows");
    }

    #[test]
    fn account_usage_lines_single_window_matches_today() {
        let u = usage(Provider::Codex, None, Some(66.0));
        let lines = account_usage_lines(&u);
        assert_eq!(lines, vec![format!("     {}", format_usage_detail(&u))]);
    }

    #[test]
    fn account_usage_lines_status_and_local_suffix_on_last_row_only() {
        let mut u = usage(Provider::Claude, Some(12.0), Some(66.0));
        u.status = "error".into();
        u.local_status = Some("unavailable".into());
        let lines = account_usage_lines(&u);
        assert_eq!(lines.len(), 2);
        assert!(!lines[0].contains("(error)"));
        assert!(!lines[0].contains("(local: unavailable)"));
        assert!(lines[1].contains("(error)"));
        assert!(lines[1].contains("(local: unavailable)"));
        let joined = lines.join(" ");
        assert_eq!(joined.matches("(error)").count(), 1);
        assert_eq!(joined.matches("(local: unavailable)").count(), 1);
    }

    #[test]
    fn account_usage_lines_agy_pool_breakdown_stays_single_row() {
        let mut u = usage(Provider::Agy, Some(12.0), Some(66.0));
        u.pool_breakdown = vec![AgyQuotaPool {
            name: "Gemini".into(),
            five_hour: None,
            week: Some(quota(97.0)),
        }];
        let lines = account_usage_lines(&u);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], format!("     {}", format_usage_detail(&u)));
    }
