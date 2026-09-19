use super::*;

use usage_core::account::Credentials;

use crate::store::SecretSource;

fn secret_creds() -> Credentials {
    Credentials {
        access_token: "unused-token".to_string(),
        refresh_token: None,
        account_id: None,
        expires_at: None,
    }
}

/// Registers one account per paid provider in an isolated tempdir-backed store,
/// using auth sources that would need real network/DB I/O if actually polled —
/// so a `"pro_required"` result for every paid account is proof that
/// `poll_all_with(.., is_pro: false)` never reached the real poll bodies.
fn store_with_all_paid_providers() -> (tempfile::TempDir, AccountStore) {
    let tmp = tempfile::tempdir().unwrap();
    let store = AccountStore::new_at(tmp.path().to_path_buf());

    store
        .add_reference_with(
            Provider::Cursor,
            "cursor-test".to_string(),
            AuthSource::CursorDatabase {
                database_path: tmp.path().join("nonexistent-cursor.vscdb"),
                expected_identity: "cursor-identity".to_string(),
            },
            || true,
        )
        .expect("register Cursor account");

    store
        .add_secret_with(
            Provider::Grok,
            "grok-test".to_string(),
            SecretSource::XaiManagement {
                team_id: "team-test".to_string(),
            },
            Credentials {
                access_token: "unused-token".to_string(),
                refresh_token: None,
                account_id: Some("team-test".to_string()),
                expires_at: None,
            },
            || true,
        )
        .expect("register Grok account");

    store
        .add_reference_with(
            Provider::Higgsfield,
            "higgsfield-test".to_string(),
            AuthSource::HiggsfieldCli {
                expected_identity: "higgsfield-test".to_string(),
            },
            || true,
        )
        .expect("register Higgsfield account");

    for (provider, label) in [
        (Provider::Kimi, "kimi-test"),
        (Provider::OpenCode, "opencode-test"),
        (Provider::DeepSeek, "deepseek-test"),
        (Provider::OpenRouter, "openrouter-test"),
    ] {
        store
            .add_secret_with(
                provider,
                label.to_string(),
                SecretSource::BrowserOAuth,
                secret_creds(),
                || true,
            )
            .unwrap_or_else(|_| panic!("register {label} account"));
    }

    (tmp, store)
}

#[tokio::test]
async fn unlicensed_poll_skips_io_for_every_paid_provider() {
    let (_tmp, store) = store_with_all_paid_providers();

    let results = poll_all_with(&store, false).await;

    assert_eq!(results.len(), 7, "expected one result per paid account");
    for usage in &results {
        assert_eq!(
            usage.status, "pro_required",
            "provider {:?} should short-circuit to pro_required without any I/O",
            usage.account.provider
        );
    }
}
