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

    #[test]
    fn auth_action_specs_with_omits_paid_providers_when_not_pro() {
        let specs = auth_action_specs_with(false);
        assert!(specs.iter().any(|s| s.provider == Provider::Codex));
        assert!(specs.iter().any(|s| s.provider == Provider::Claude));
        assert!(specs.iter().any(|s| s.provider == Provider::Agy));
        assert!(!specs.iter().any(|s| s.provider == Provider::Cursor));
        assert!(!specs.iter().any(|s| s.provider == Provider::Grok));
        assert!(!specs.iter().any(|s| s.provider == Provider::Higgsfield));
    }

    #[test]
    fn auth_action_specs_with_includes_paid_providers_when_pro() {
        let specs = auth_action_specs_with(true);
        assert!(specs.iter().any(|s| s.provider == Provider::Cursor));
        assert!(specs.iter().any(|s| s.provider == Provider::Grok));
        assert!(specs.iter().any(|s| s.provider == Provider::Higgsfield));
    }

    #[test]
    fn is_dispatch_allowed_refuses_paid_provider_without_pro() {
        let spec = spec_for_event("add-grok-clipboard").expect("Grok clipboard is registered");
        assert!(
            !is_dispatch_allowed(&spec, false),
            "a paid-provider spec must be refused at dispatch when not Pro, \
             regardless of whether the menu happened to render it"
        );
        assert!(
            is_dispatch_allowed(&spec, true),
            "a paid-provider spec is permitted at dispatch once Pro is active"
        );
    }

    #[test]
    fn is_dispatch_allowed_always_permits_free_providers() {
        let spec = spec_for_event("add-codex-oauth").expect("Codex OAuth is registered");
        assert!(is_dispatch_allowed(&spec, false));
        assert!(is_dispatch_allowed(&spec, true));
    }

    #[test]
    fn is_add_enabled_permits_a_free_provider_with_no_accounts() {
        let spec = spec_for_event("add-codex-cli").expect("Codex CLI is registered");

        assert!(is_add_enabled(&spec, false, &[]));
    }

    #[test]
    fn is_add_enabled_refuses_a_free_provider_at_the_cap() {
        let accounts = [account("codex-1", Provider::Codex)];
        let specs = auth_action_specs_with(false);

        for spec in specs
            .iter()
            .filter(|spec| spec.provider == Provider::Codex)
        {
            assert!(
                !is_add_enabled(spec, false, &accounts),
                "{} must be disabled once Codex reaches the Free cap",
                spec.event_id
            );
        }
        assert!(specs
            .iter()
            .filter(|spec| matches!(spec.provider, Provider::Claude | Provider::Agy))
            .all(|spec| is_add_enabled(spec, false, &accounts)));
    }

    #[test]
    fn is_add_enabled_permits_a_capped_free_provider_when_pro() {
        let accounts = [account("codex-1", Provider::Codex)];
        let spec = spec_for_event("add-codex-cli").expect("Codex CLI is registered");

        assert!(is_add_enabled(&spec, true, &accounts));
    }

    #[test]
    fn is_add_enabled_still_refuses_paid_providers_without_pro() {
        let spec = spec_for_event("add-cursor-local").expect("Cursor local import is registered");

        assert!(!is_add_enabled(&spec, false, &[]));
    }

    #[test]
    fn is_add_enabled_ignores_other_providers_accounts() {
        let accounts = [
            account("claude-1", Provider::Claude),
            account("claude-2", Provider::Claude),
            account("claude-3", Provider::Claude),
        ];
        let spec = spec_for_event("add-codex-cli").expect("Codex CLI is registered");

        assert!(is_add_enabled(&spec, false, &accounts));
    }

    #[test]
    fn add_entry_label_is_unchanged_when_enabled() {
        let spec = spec_for_event("add-codex-cli").expect("Codex CLI is registered");

        assert_eq!(add_entry_label(&spec, true), spec.label);
    }

    #[test]
    fn add_entry_label_explains_the_cap_when_disabled() {
        let spec = spec_for_event("add-codex-cli").expect("Codex CLI is registered");
        let enabled = add_entry_label(&spec, true);
        let disabled = add_entry_label(&spec, false);

        assert!(disabled.contains(spec.label));
        assert!(disabled.contains("Pro required"));
        assert_ne!(disabled, enabled);
    }

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

    fn account(id: &str, provider: Provider) -> Account {
        Account {
            id: id.into(),
            provider,
            label: format!("{id}@example.com"),
            auth_source: AuthSource::BrowserOAuth {
                credential_id: format!("credential-{id}"),
            },
        }
    }

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
    fn codex_five_hour_and_week_windows_render_separate_labelled_rows() {
        let mut u = usage(Provider::Codex, Some(12.0), Some(66.0));
        u.five_hour = Some(QuotaUsage {
            percent: 12.0,
            resets_at: Some(chrono::Utc::now() + chrono::Duration::hours(2)),
            window_seconds: Some(18_000),
        });
        u.week = Some(QuotaUsage {
            percent: 66.0,
            resets_at: Some(chrono::Utc::now() + chrono::Duration::days(6)),
            window_seconds: Some(604_800),
        });

        let lines = account_usage_lines(&u);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("     5h 12% · resets "), "row 1: {}", lines[0]);
        assert!(lines[1].starts_with("     7d 66% · resets "), "row 2: {}", lines[1]);
        assert_ne!(lines[0], lines[1], "each window needs its own reset row");
    }

    #[test]
    fn format_usage_detail_higgsfield_line_renders_used_percent_and_credits_with_reset() {
        // Ensure reset text is computed at render time from `resets_at` (not captured
        // inside `detail_suffix`), so a stale polled timestamp cannot freeze the
        // live countdown shown in the tray row.
        let mut u = usage(Provider::Higgsfield, None, Some(74.5));
        u.week = Some(QuotaUsage {
            percent: 74.5,
            resets_at: Some(chrono::Utc::now() + chrono::Duration::hours(8)),
            window_seconds: None,
        });
        u.detail_suffix = Some("12.75/50 credits".to_string());

        let line = format_usage_detail(&u);
        assert!(line.starts_with("74.5% · 12.75/50 credits"), "line: {line}");
        assert!(line.contains("· resets "), "line: {line}");
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

    // --- Stage C: license tray section (pure formatting/gating only —
    // building a real `Menu` requires the platform main thread, which
    // `cargo test` does not run on; see `menu_actions_tests.rs` for the
    // event-routing side of this section). ---------------------------

    use crate::license::{ActivationErrorClass, LicenseStatus};

    #[test]
    fn license_status_line_renders_every_variant() {
        assert_eq!(license_status_line(LicenseStatus::Free), "License: Free");
        assert_eq!(
            license_status_line(LicenseStatus::Pro { expires_at: None }),
            "License: Pro"
        );
        assert_eq!(
            license_status_line(LicenseStatus::ProDevOverride),
            "License: Pro (dev override)"
        );
        let expires_at = chrono::Utc::now() + chrono::Duration::days(3) + chrono::Duration::hours(2);
        let line = license_status_line(LicenseStatus::Pro {
            expires_at: Some(expires_at),
        });
        assert!(line.starts_with("License: Pro · expires "), "{line}");
        assert!(line.contains("3d"), "{line}");
        assert_eq!(
            license_status_line(LicenseStatus::Expired),
            "License: expired — reactivate"
        );
        assert_eq!(
            license_status_line(LicenseStatus::GracePeriodEnded),
            "License: verification needed"
        );
    }

    #[test]
    fn activation_result_line_uses_only_the_fixed_classification_vocabulary() {
        // `ActivationErrorClass` carries no string payload (see its
        // definition) and `activation_result_line` never reads
        // `ActivationError`'s `Display` — so by construction this can never
        // embed the license key, the token, or server-provided text (H4).
        // Pin the exact fixed vocabulary so a future change that threads a
        // `String` payload through gets caught immediately.
        let cases: &[(Result<(), ActivationErrorClass>, &str)] = &[
            (Ok(()), "Last attempt: activated"),
            (Err(ActivationErrorClass::Network), "Last attempt: no network"),
            (Err(ActivationErrorClass::ServerRejected), "Last attempt: invalid key"),
            (Err(ActivationErrorClass::InvalidToken), "Last attempt: invalid key"),
            (
                Err(ActivationErrorClass::DeviceMismatch),
                "Last attempt: bound to a different device",
            ),
            (
                Err(ActivationErrorClass::EndpointMissing),
                "Last attempt: server not available yet",
            ),
            (
                Err(ActivationErrorClass::Persist),
                "Last attempt: could not save license",
            ),
            (
                Err(ActivationErrorClass::NoStoredLicense),
                "Last attempt: no license to refresh",
            ),
            (Err(ActivationErrorClass::ReplayedToken), "Last attempt: try again"),
            (
                Err(ActivationErrorClass::NotEntitled),
                "Last attempt: key not currently valid",
            ),
            (
                Err(ActivationErrorClass::DeviceNotPersisted),
                "Last attempt: could not save device id",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(activation_result_line(input), *expected);
        }
    }

    #[test]
    fn add_account_result_line_prefixes_and_truncates() {
        let long = add_account_result_line(&"x".repeat(400));
        assert!(long.starts_with("Add account: "));
        assert!(
            long.chars().count() <= "Add account: ".chars().count() + 120,
            "line exceeded the prefix plus 120-character reason limit: {long}"
        );
        assert!(long.ends_with('…'));

        assert_eq!(add_account_result_line("short reason"), "Add account: short reason");
        assert_eq!(
            add_account_result_line("line one\nline two"),
            "Add account: line one line two"
        );
    }

    #[test]
    fn no_license_tray_string_can_contain_a_key_or_token_shaped_value() {
        // Structural proof, not a substring scan of one sample: every row
        // this section can render comes from `license_status_line` (which
        // only ever sees a `LicenseStatus` — no string field at all besides
        // an optional timestamp) or `activation_result_line` (which only
        // ever sees an `ActivationErrorClass` — a data-less enum). Exercise
        // every producible value of both and assert none contains a
        // plausible key/token marker.
        let forbidden = ["TEST-KEY", "sk-", "Bearer ", "eyJ"];
        let statuses = [
            LicenseStatus::Free,
            LicenseStatus::Pro { expires_at: None },
            LicenseStatus::Pro {
                expires_at: Some(chrono::Utc::now() + chrono::Duration::days(1)),
            },
            LicenseStatus::ProDevOverride,
            LicenseStatus::Expired,
            LicenseStatus::GracePeriodEnded,
        ];
        for status in statuses {
            let line = license_status_line(status);
            for marker in forbidden {
                assert!(!line.contains(marker), "status line leaked: {line}");
            }
        }
        let classes = [
            Ok(()),
            Err(ActivationErrorClass::Network),
            Err(ActivationErrorClass::ServerRejected),
            Err(ActivationErrorClass::InvalidToken),
            Err(ActivationErrorClass::DeviceMismatch),
            Err(ActivationErrorClass::EndpointMissing),
            Err(ActivationErrorClass::Persist),
            Err(ActivationErrorClass::NoStoredLicense),
            Err(ActivationErrorClass::ReplayedToken),
            Err(ActivationErrorClass::NotEntitled),
            Err(ActivationErrorClass::DeviceNotPersisted),
        ];
        for result in classes {
            let line = activation_result_line(&result);
            for marker in forbidden {
                assert!(!line.contains(marker), "result line leaked: {line}");
            }
        }
    }

    #[test]
    fn should_show_deactivate_requires_pro_or_a_stored_record() {
        assert!(!should_show_deactivate(false, false));
        assert!(should_show_deactivate(true, false));
        assert!(should_show_deactivate(false, true));
        assert!(should_show_deactivate(true, true));
    }

    // --- item 4 fix: `license_rows` — the ordered license-section row SET
    // (ids, labels, enabled flags, and which rows even appear) as a pure,
    // unit-testable decision. `build_menu` renders exactly this; a real
    // `tauri::menu::Menu` needs the platform main thread so it can't be
    // constructed or inspected here. -----------------------------------

    fn row_ids_and_enabled(
        status: LicenseStatus,
        last_attempt: Option<&Result<(), ActivationErrorClass>>,
        has_record: bool,
    ) -> Vec<(&'static str, bool)> {
        license_rows(status, last_attempt, has_record)
            .iter()
            .map(|r| (r.id, r.enabled))
            .collect()
    }

    #[test]
    fn license_rows_free_no_attempt_no_record() {
        assert_eq!(
            row_ids_and_enabled(LicenseStatus::Free, None, false),
            vec![
                ("license-status", false),
                ("license-activate-clipboard", true),
                ("license-get", true),
            ]
        );
    }

    #[test]
    fn license_rows_free_after_failed_attempt() {
        let attempt = Err(ActivationErrorClass::ServerRejected);
        assert_eq!(
            row_ids_and_enabled(LicenseStatus::Free, Some(&attempt), false),
            vec![
                ("license-status", false),
                ("license-result", false),
                ("license-activate-clipboard", true),
                ("license-get", true),
            ]
        );
    }

    #[test]
    fn license_rows_pro_with_expiry() {
        let attempt = Ok(());
        let expires_at = chrono::Utc::now() + chrono::Duration::days(10);
        assert_eq!(
            row_ids_and_enabled(
                LicenseStatus::Pro {
                    expires_at: Some(expires_at)
                },
                Some(&attempt),
                true
            ),
            vec![
                ("license-status", false),
                ("license-result", false),
                ("license-activate-clipboard", true),
                ("license-deactivate", true),
                ("license-get", true),
            ]
        );
    }

    #[test]
    fn license_rows_pro_without_expiry() {
        assert_eq!(
            row_ids_and_enabled(LicenseStatus::Pro { expires_at: None }, None, true),
            vec![
                ("license-status", false),
                ("license-activate-clipboard", true),
                ("license-deactivate", true),
                ("license-get", true),
            ]
        );
    }

    #[test]
    fn license_rows_dev_override_without_record_still_offers_deactivate() {
        assert_eq!(
            row_ids_and_enabled(LicenseStatus::ProDevOverride, None, false),
            vec![
                ("license-status", false),
                ("license-activate-clipboard", true),
                ("license-deactivate", true),
                ("license-get", true),
            ]
        );
    }

    #[test]
    fn license_rows_expired_with_record() {
        assert_eq!(
            row_ids_and_enabled(LicenseStatus::Expired, None, true),
            vec![
                ("license-status", false),
                ("license-activate-clipboard", true),
                ("license-deactivate", true),
                ("license-get", true),
            ]
        );
    }

    #[test]
    fn license_rows_grace_period_ended_with_record() {
        assert_eq!(
            row_ids_and_enabled(LicenseStatus::GracePeriodEnded, None, true),
            vec![
                ("license-status", false),
                ("license-activate-clipboard", true),
                ("license-deactivate", true),
                ("license-get", true),
            ]
        );
    }

    #[test]
    fn license_rows_record_present_but_not_pro_still_offers_deactivate() {
        // item 3's fix: a stored-but-malformed record must still surface a
        // way to remove it, even though `decide_status`/`status_in` reads
        // it as `Free` (never Pro) — `has_record: true` with `Free` status
        // is exactly that shape.
        assert_eq!(
            row_ids_and_enabled(LicenseStatus::Free, None, true),
            vec![
                ("license-status", false),
                ("license-activate-clipboard", true),
                ("license-deactivate", true),
                ("license-get", true),
            ]
        );
    }

    #[test]
    fn add_section_and_license_section_agree_on_one_sample() {
        let statuses = [
            LicenseStatus::Free,
            LicenseStatus::Pro { expires_at: None },
            LicenseStatus::Pro {
                expires_at: Some(chrono::Utc::now() + chrono::Duration::days(1)),
            },
            LicenseStatus::ProDevOverride,
            LicenseStatus::Expired,
            LicenseStatus::GracePeriodEnded,
        ];

        for status in statuses {
            let add_section_is_pro = matches!(
                status,
                LicenseStatus::Pro { .. } | LicenseStatus::ProDevOverride
            );
            let license_section_is_pro = license_rows(status, None, false)
                .iter()
                .any(|row| row.id == "license-deactivate");

            assert_eq!(
                add_section_is_pro, license_section_is_pro,
                "Add and license sections disagreed for {status:?}"
            );
        }
    }

    #[test]
    fn license_rows_result_row_absent_before_and_present_after_an_attempt() {
        assert!(
            !row_ids_and_enabled(LicenseStatus::Free, None, false)
                .iter()
                .any(|(id, _)| *id == "license-result"),
            "no attempt yet: the result row must not appear"
        );
        let attempt = Ok(());
        assert!(
            row_ids_and_enabled(LicenseStatus::Free, Some(&attempt), false)
                .iter()
                .any(|(id, _)| *id == "license-result"),
            "after an attempt: the result row must appear"
        );
    }

    #[test]
    fn license_rows_only_the_three_action_rows_are_enabled() {
        let attempt = Ok(());
        let rows = license_rows(LicenseStatus::Pro { expires_at: None }, Some(&attempt), true);
        let action_ids = ["license-activate-clipboard", "license-deactivate", "license-get"];
        for row in &rows {
            let expected_enabled = action_ids.contains(&row.id);
            assert_eq!(
                row.enabled, expected_enabled,
                "row {} enabled={} (expected {})",
                row.id, row.enabled, expected_enabled
            );
        }
        // Sanity: all three action rows are actually present in this fixture.
        for id in action_ids {
            assert!(rows.iter().any(|r| r.id == id), "expected row {id} to be present");
        }
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
