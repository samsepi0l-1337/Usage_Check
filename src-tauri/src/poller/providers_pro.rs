#![cfg(feature = "edition-pro")]

use super::http::{
    fetch_cursor_quota,
    fetch_grok_prepaid,
    fetch_higgsfield_account_json,
    refresh_cursor_access_token,
};
use super::usage_model::{
    account_usage_from_cursor,
    account_usage_from_grok,
    account_usage_from_higgsfield,
    status_for_failure,
    AccountUsage,
};
use crate::store::AccountStore;
use usage_core::account::Account;
use usage_core::fetch::cursor::{cursor_quota_with_auth, CursorQuota};
use usage_core::fetch::grok::GrokPrepaid;
use usage_core::fetch::higgsfield::{parse_higgsfield_account, HiggsfieldCredits};

fn cursor_outcome_status(
    session_id: &str,
    expected_id: &str,
    fetch: Result<(), Option<u16>>,
) -> &'static str {
    if session_id != expected_id {
        "identity_changed"
    } else if fetch.is_err() {
        "experimental_error"
    } else {
        "ok"
    }
}

pub(super) async fn poll_cursor(
    _store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
    use usage_core::account::AuthSource;

    // Get database path and expected identity from auth_source
    let (database_path, expected_identity) = match &account.auth_source {
        AuthSource::CursorDatabase {
            database_path,
            expected_identity,
        } => (database_path.clone(), expected_identity.clone()),
        _ => {
            return account_usage_from_cursor(
                account,
                &CursorQuota {
                    email: None,
                    plan: None,
                    period: None,
                    detail_suffix: None,
                },
                "needs_login",
            );
        }
    };

    // Open DB read-only and read session (NO store.update_credentials)
    let session = match crate::cursor_local::read_cursor_session(&database_path) {
        Ok(s) => s,
        Err(_) => {
            return account_usage_from_cursor(
                account,
                &CursorQuota {
                    email: None,
                    plan: None,
                    period: None,
                    detail_suffix: None,
                },
                "needs_login",
            );
        }
    };

    // Validate identity
    let identity_status = cursor_outcome_status(&session.identity, &expected_identity, Ok(()));
    if identity_status != "ok" {
        return account_usage_from_cursor(
            account,
            &CursorQuota {
                email: None,
                plan: None,
                period: None,
                detail_suffix: None,
            },
            identity_status,
        );
    }

    // Refresh token in memory only
    let access_token = if let Some(refresh_token) = session.refresh_token.as_deref() {
        if let Ok(new_token) = refresh_cursor_access_token(client, refresh_token).await {
            new_token
        } else {
            session.access_token.clone()
        }
    } else {
        session.access_token.clone()
    };

    // Fetch quota with in-memory token
    let temp_creds = usage_core::account::Credentials {
        access_token,
        refresh_token: session.refresh_token.clone(),
        account_id: session.plan.clone(),
        expires_at: None,
    };
    match fetch_cursor_quota(client, &temp_creds).await {
        Ok(mut quota) => {
            quota = cursor_quota_with_auth(quota, session.email.clone(), session.plan.clone());
            account_usage_from_cursor(
                account,
                &quota,
                cursor_outcome_status(&session.identity, &expected_identity, Ok(())),
            )
        }
        Err(status) => account_usage_from_cursor(
            account,
            &CursorQuota {
                email: None,
                plan: session.plan.clone(),
                period: None,
                detail_suffix: None,
            },
            cursor_outcome_status(&session.identity, &expected_identity, Err(status)),
        ),
    }
}

pub(super) async fn poll_grok(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
    let Some(creds) = store.credentials(&account.id) else {
        return account_usage_from_grok(
            account,
            &GrokPrepaid {
                period: None,
                detail_suffix: Some("needs setup".into()),
            },
            "needs_login",
        );
    };
    match fetch_grok_prepaid(client, &creds).await {
        Ok(prepaid) => account_usage_from_grok(account, &prepaid, "ok"),
        Err(status) => account_usage_from_grok(
            account,
            &GrokPrepaid {
                period: None,
                detail_suffix: None,
            },
            status_for_failure(status),
        ),
    }
}

pub(super) async fn poll_higgsfield(store: &AccountStore, account: &Account) -> AccountUsage {
    if store.credentials(&account.id).is_none() {
        return account_usage_from_higgsfield(
            account,
            &HiggsfieldCredits {
                email: None,
                plan: None,
                credits_remaining: None,
            },
            "needs_login",
        );
    }

    match fetch_higgsfield_account_json() {
        Ok(root) => {
            let credits = parse_higgsfield_account(&root);
            let status = if credits.credits_remaining.is_some() {
                "ok"
            } else {
                "needs_setup"
            };
            account_usage_from_higgsfield(account, &credits, status)
        }
        Err(()) => account_usage_from_higgsfield(
            account,
            &HiggsfieldCredits {
                email: None,
                plan: None,
                credits_remaining: None,
            },
            "needs_setup",
        ),
    }
}

#[cfg(test)]
#[path = "providers_pro_tests.rs"]
mod tests;
