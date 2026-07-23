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
