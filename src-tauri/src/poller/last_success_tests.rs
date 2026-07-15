use crate::poller::AccountUsage;
use usage_core::account::{Account, AuthSource, Provider};
use usage_core::models::QuotaUsage;
use usage_core::models::WindowTotals;

use super::*;

    // Serializes every test that mutates the process-global `last_success_cache`
    // (clear/evict/direct-lock). Without this, parallel `cargo test` threads clobber
    // each other's cached entries (e.g. one test's clear() wipes another's seeded "ok"
    // before its assertion), producing intermittent "error" vs "stale" failures.
    static LAST_SUCCESS_CACHE_TEST_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_last_success_cache_tests() -> std::sync::MutexGuard<'static, ()> {
        LAST_SUCCESS_CACHE_TEST_GUARD
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn auth_source_usage(id: &str, status: &str, five_hour: Option<QuotaUsage>) -> AccountUsage {
        let account = Account {
            id: id.to_string(),
            provider: Provider::Codex,
            label: "user@example.com".into(),
            auth_source: AuthSource::BrowserOAuth {
                credential_id: format!("credential-{id}"),
            },
        };
        AccountUsage {
            account,
            display_name: "user@example.com".into(),
            plan: Some("Pro".into()),
            five_hour,
            week: None,
            totals: WindowTotals::default(),
            pool_breakdown: Vec::new(),
            detail_suffix: None,
            status: status.to_string(),
            local_status: None,
        }
    }


    fn auth_source_quota(percent: f64) -> QuotaUsage {
        QuotaUsage {
            percent,
            resets_at: None,
            window_seconds: Some(18_000),
        }
    }

    #[test]
    fn auth_source_evict_last_success_drops_stale() {
        let _cache_guard = lock_last_success_cache_tests();
        clear_last_success_cache();
        let quota = auth_source_quota(25.0);
        {
            let mut cache = last_success_cache()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            apply_last_success(&mut cache, "x", auth_source_usage("x", "ok", Some(quota)));
        }

        evict_last_success("x");

        let result = {
            let mut cache = last_success_cache()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            apply_last_success(&mut cache, "x", auth_source_usage("x", "error", None))
        };

        assert_eq!(result.status, "error");
        assert_eq!(result.five_hour, None);
    }


    #[test]
    fn auth_source_evict_is_isolated() {
        let _cache_guard = lock_last_success_cache_tests();
        clear_last_success_cache();
        let x_quota = auth_source_quota(25.0);
        let y_quota = auth_source_quota(75.0);
        {
            let mut cache = last_success_cache()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            apply_last_success(&mut cache, "x", auth_source_usage("x", "ok", Some(x_quota)));
            apply_last_success(
                &mut cache,
                "y",
                auth_source_usage("y", "ok", Some(y_quota.clone())),
            );
        }

        evict_last_success("x");

        let (y_result, x_result) = {
            let mut cache = last_success_cache()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let y_result =
                apply_last_success(&mut cache, "y", auth_source_usage("y", "error", None));
            let x_result =
                apply_last_success(&mut cache, "x", auth_source_usage("x", "error", None));
            (y_result, x_result)
        };

        assert_eq!(y_result.status, "stale");
        assert_eq!(y_result.five_hour, Some(y_quota));
        assert_eq!(x_result.status, "error");
        assert_eq!(x_result.five_hour, None);
    }


    #[test]
    fn auth_source_ok_then_transient_error_serves_stale() {
        let _cache_guard = lock_last_success_cache_tests();
        clear_last_success_cache();
        let mut cache = HashMap::new();
        let quota = auth_source_quota(25.0);
        apply_last_success(
            &mut cache,
            "account-1",
            auth_source_usage("account-1", "ok", Some(quota.clone())),
        );

        let result = apply_last_success(
            &mut cache,
            "account-1",
            auth_source_usage("account-1", "error", None),
        );

        assert_eq!(result.status, "stale");
        assert_eq!(result.five_hour, Some(quota));
    }


    #[test]
    fn auth_source_transient_error_without_prior_success_stays_error() {
        let _cache_guard = lock_last_success_cache_tests();
        clear_last_success_cache();
        let mut cache = HashMap::new();

        let result = apply_last_success(
            &mut cache,
            "account-1",
            auth_source_usage("account-1", "error", None),
        );

        assert_eq!(result.status, "error");
        assert_eq!(result.five_hour, None);
    }


    #[test]
    fn auth_source_needs_login_never_stale() {
        let _cache_guard = lock_last_success_cache_tests();
        clear_last_success_cache();
        let mut cache = HashMap::new();
        apply_last_success(
            &mut cache,
            "account-1",
            auth_source_usage("account-1", "ok", Some(auth_source_quota(25.0))),
        );

        for status in ["needs_login", "rate_limited", "identity_changed"] {
            let result = apply_last_success(
                &mut cache,
                "account-1",
                auth_source_usage("account-1", status, None),
            );
            assert_eq!(result.status, status);
            assert_eq!(result.five_hour, None);
        }
    }


    #[test]
    fn auth_source_stale_restores_pool_breakdown_and_detail_suffix() {
        // Regression (M1): Agy/Pro accounts carry pool_breakdown/detail_suffix.
        // A transient error must restore them on stale, not blank them out.
        let _cache_guard = lock_last_success_cache_tests();
        clear_last_success_cache();
        let mut cache = HashMap::new();

        let mut good = auth_source_usage("agy-1", "ok", Some(auth_source_quota(40.0)));
        good.pool_breakdown = vec![usage_core::fetch::agy::AgyQuotaPool {
            name: "Gemini".into(),
            five_hour: Some(auth_source_quota(40.0)),
            week: Some(auth_source_quota(60.0)),
        }];
        good.detail_suffix = Some("$12 left".into());
        apply_last_success(&mut cache, "agy-1", good);

        // Error path produces an empty snapshot (no pools, no suffix).
        let mut failed = auth_source_usage("agy-1", "error", None);
        failed.pool_breakdown = Vec::new();
        failed.detail_suffix = None;
        let result = apply_last_success(&mut cache, "agy-1", failed);

        assert_eq!(result.status, "stale");
        assert_eq!(result.pool_breakdown.len(), 1);
        assert_eq!(result.pool_breakdown[0].name, "Gemini");
        assert_eq!(result.detail_suffix.as_deref(), Some("$12 left"));
    }

    #[test]
    fn auth_source_stale_uses_latest_success() {
        let _cache_guard = lock_last_success_cache_tests();
        clear_last_success_cache();
        let mut cache = HashMap::new();
        let first = auth_source_quota(25.0);
        let latest = auth_source_quota(75.0);
        apply_last_success(
            &mut cache,
            "account-1",
            auth_source_usage("account-1", "ok", Some(first)),
        );
        apply_last_success(
            &mut cache,
            "account-1",
            auth_source_usage("account-1", "ok", Some(latest.clone())),
        );

        let result = apply_last_success(
            &mut cache,
            "account-1",
            auth_source_usage("account-1", "error", None),
        );

        assert_eq!(result.status, "stale");
        assert_eq!(result.five_hour, Some(latest));
    }
