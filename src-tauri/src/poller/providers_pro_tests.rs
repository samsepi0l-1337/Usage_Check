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

mod paid_identity_regressions {
    use super::*;
    use usage_core::account::{AuthSource, Credentials};

    fn reference(provider: Provider, identity: &str) -> Account {
        let expected_identity = identity.to_string();
        let auth_source = match provider {
            Provider::Higgsfield => AuthSource::HiggsfieldCli { expected_identity },
            Provider::MiniMax => AuthSource::MiniMaxCli { expected_identity },
            Provider::Augment => AuthSource::AugmentCli { expected_identity },
            _ => unreachable!(),
        };
        Account {
            id: "reference".into(),
            provider,
            label: identity.into(),
            auth_source,
        }
    }

    fn check_cli_reference(provider: Provider) {
        for (email, status) in [
            (Some("a@example.test"), "ok"),
            (Some("b@example.test"), "identity_changed"),
            (None, "needs_setup"),
        ] {
            let root = serde_json::json!({"email": email,
                    "credits": 25, "credits_remaining": 25, "credits_total": 100, "credits_included": 100,
                    "current_interval_remaining_percent": 75});
            let account = reference(provider, "a@example.test");
            let usage = match provider {
                Provider::Higgsfield => higgsfield_usage(&account, Ok(root)),
                Provider::MiniMax => minimax_usage(&account, Ok(root)),
                _ => augment_usage(&account, Ok(root)),
            };
            assert_eq!(usage.status, status, "{provider:?}, {email:?}");
            if status != "ok" {
                assert!(usage.five_hour.is_none() && usage.week.is_none());
                assert!(usage.detail_suffix.is_none());
                assert!(!usage.display_name.contains("b@example.test"));
            } else {
                assert!(usage.five_hour.is_some() || usage.week.is_some());
                assert!(usage.display_name.contains("a@example.test"));
            }
        }
    }

    #[test]
    fn higgsfield_reference_without_secret_validates_cli_identity() {
        check_cli_reference(Provider::Higgsfield);
    }
    #[test]
    fn minimax_reference_validates_cli_identity() {
        check_cli_reference(Provider::MiniMax);
    }
    #[test]
    fn augment_reference_validates_cli_identity() {
        check_cli_reference(Provider::Augment);
    }

    #[tokio::test]
    async fn kiro_requires_app_owned_credentials_and_profile_arn() {
        let dir = tempfile::tempdir().unwrap();
        let store = AccountStore::new_at(dir.path().join("store"));
        let account = Account {
            id: "a".into(),
            provider: Provider::Kiro,
            label: "a@example.test".into(),
            auth_source: AuthSource::BrowserOAuth {
                credential_id: "a".into(),
            },
        };
        let usage = poll_kiro(&store, &reqwest::Client::new(), &account).await;
        assert_eq!(usage.status, "needs_login");
        assert!(usage.week.is_none());
        let account = store
            .add_with(
                Provider::Kiro,
                "a@example.test".into(),
                Credentials {
                    access_token: "stored-a".into(),
                    refresh_token: None,
                    account_id: None,
                    expires_at: None,
                },
                || true,
            )
            .unwrap();
        let usage = poll_kiro(&store, &reqwest::Client::new(), &account).await;
        assert_eq!(usage.status, "needs_setup");
    }
}
