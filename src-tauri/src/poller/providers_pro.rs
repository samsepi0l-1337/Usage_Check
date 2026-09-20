use super::http::{
    fetch_amp_balance, fetch_copilot_user, fetch_cursor_quota, fetch_deepseek_balance,
    fetch_fireworks_billing, fetch_grok_prepaid, fetch_higgsfield_account_json, fetch_kimi_usages,
    fetch_novita_balance, fetch_opencode_usage, fetch_openrouter_key, fetch_poe_balance,
    fetch_windsurf_status, fetch_zai_quota, refresh_cursor_access_token,
};
use super::providers::maybe_refresh;
use super::usage_model::{
    account_usage_from_amp, account_usage_from_augment, account_usage_from_bailian,
    account_usage_from_copilot, account_usage_from_cursor, account_usage_from_deepseek,
    account_usage_from_fireworks, account_usage_from_grok, account_usage_from_higgsfield,
    account_usage_from_kimi, account_usage_from_minimax, account_usage_from_novita,
    account_usage_from_opencode, account_usage_from_openrouter, account_usage_from_poe,
    account_usage_from_windsurf, account_usage_from_zai, status_for_failure, AccountUsage,
};
use crate::store::AccountStore;
use usage_core::account::{Account, Provider};
use usage_core::fetch::amp::AmpBalance;
use usage_core::fetch::augment::{parse_augment_account, AugmentCredits};
use usage_core::fetch::bailian::{parse_bailian_token_plan, BailianQuota};
use usage_core::fetch::copilot::CopilotQuota;
use usage_core::fetch::cursor::{cursor_quota_with_auth, CursorQuota};
use usage_core::fetch::deepseek::DeepSeekBalance;
use usage_core::fetch::fireworks::FireworksBilling;
use usage_core::fetch::grok::GrokPrepaid;
use usage_core::fetch::higgsfield::{parse_higgsfield_account, HiggsfieldCredits};
use usage_core::fetch::kimi::KimiUsage;
use usage_core::fetch::minimax::{parse_minimax_quota, MiniMaxQuota};
use usage_core::fetch::novita::NovitaBalance;
use usage_core::fetch::opencode::OpenCodeUsage;
use usage_core::fetch::openrouter::OpenRouterUsage;
use usage_core::fetch::poe::PoeBalance;
use usage_core::fetch::windsurf::WindsurfQuota;
use usage_core::fetch::zai::ZaiQuota;

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
                    breakdown: Vec::new(),
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
                    breakdown: Vec::new(),
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
                breakdown: Vec::new(),
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
                breakdown: Vec::new(),
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
    let Some(creds) = store.credentials(AccountStore::credential_key(account)) else {
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
    if store
        .credentials(AccountStore::credential_key(account))
        .is_none()
    {
        return account_usage_from_higgsfield(
            account,
            &HiggsfieldCredits {
                email: None,
                plan: None,
                credits_remaining: None,
                credits_total: None,
                renews_at: None,
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
                credits_total: None,
                renews_at: None,
            },
            "needs_setup",
        ),
    }
}

pub(super) async fn poll_minimax(_store: &AccountStore, account: &Account) -> AccountUsage {
    match crate::import::fetch_minimax_quota_json() {
        Ok(root) => {
            let quota = parse_minimax_quota(&root);
            let status = if quota.five_hour.is_some() || quota.week.is_some() {
                "ok"
            } else {
                "needs_setup"
            };
            account_usage_from_minimax(account, &quota, status)
        }
        Err(_) => account_usage_from_minimax(account, &MiniMaxQuota::default(), "needs_setup"),
    }
}

pub(super) async fn poll_augment(_store: &AccountStore, account: &Account) -> AccountUsage {
    match crate::import::fetch_augment_account_json() {
        Ok(root) => {
            let credits = parse_augment_account(&root);
            let status = if credits.credits_remaining.is_some() {
                "ok"
            } else {
                "needs_setup"
            };
            account_usage_from_augment(account, &credits, status)
        }
        Err(_) => account_usage_from_augment(account, &AugmentCredits::default(), "needs_setup"),
    }
}

fn kimi_status(status: Option<u16>) -> &'static str {
    match status {
        Some(401) => "needs_login",
        Some(404) => "needs_setup",
        Some(429) => "throttled",
        _ => "error",
    }
}

fn opencode_status(status: Option<u16>) -> &'static str {
    match status {
        Some(401) => "needs_login",
        Some(403) => "needs_setup",
        Some(429) => "throttled",
        _ => "error",
    }
}

pub(super) async fn poll_kimi(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
    let credential_id = AccountStore::credential_key(account);
    let Some(creds) = store.credentials(credential_id) else {
        return account_usage_from_kimi(account, &KimiUsage::default(), "needs_login");
    };
    let creds = maybe_refresh(store, credential_id, Provider::Kimi, creds).await;
    match fetch_kimi_usages(client, &creds).await {
        Ok(quota) if quota.is_empty() => account_usage_from_kimi(account, &quota, "needs_setup"),
        Ok(quota) => account_usage_from_kimi(account, &quota, "ok"),
        Err(status) => account_usage_from_kimi(account, &KimiUsage::default(), kimi_status(status)),
    }
}

pub(super) async fn poll_opencode(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
    let Some(creds) = store.credentials(AccountStore::credential_key(account)) else {
        return account_usage_from_opencode(account, &OpenCodeUsage::default(), "needs_login");
    };
    match fetch_opencode_usage(client, &creds).await {
        Ok(quota) => account_usage_from_opencode(account, &quota, "ok"),
        Err(status) => {
            account_usage_from_opencode(account, &OpenCodeUsage::default(), opencode_status(status))
        }
    }
}

pub(super) async fn poll_deepseek(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
    let Some(creds) = store.credentials(AccountStore::credential_key(account)) else {
        return account_usage_from_deepseek(account, &DeepSeekBalance::default(), "needs_login");
    };
    match fetch_deepseek_balance(client, &creds).await {
        Ok(balance) => account_usage_from_deepseek(account, &balance, "ok"),
        Err(status) => account_usage_from_deepseek(
            account,
            &DeepSeekBalance::default(),
            status_for_failure(status),
        ),
    }
}

pub(super) async fn poll_openrouter(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
    let Some(creds) = store.credentials(AccountStore::credential_key(account)) else {
        return account_usage_from_openrouter(account, &OpenRouterUsage::default(), "needs_login");
    };
    match fetch_openrouter_key(client, &creds).await {
        Ok(usage) => account_usage_from_openrouter(account, &usage, "ok"),
        Err(status) => account_usage_from_openrouter(
            account,
            &OpenRouterUsage::default(),
            status_for_failure(status),
        ),
    }
}

pub(super) async fn poll_poe(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
    let Some(creds) = store.credentials(AccountStore::credential_key(account)) else {
        return account_usage_from_poe(account, &PoeBalance::default(), "needs_login");
    };
    match fetch_poe_balance(client, &creds).await {
        Ok(balance) => {
            let status = if balance.detail_suffix.is_none() && balance.period.is_none() {
                "needs_setup"
            } else {
                "ok"
            };
            account_usage_from_poe(account, &balance, status)
        }
        Err(status) => {
            account_usage_from_poe(account, &PoeBalance::default(), status_for_failure(status))
        }
    }
}

pub(super) async fn poll_fireworks(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
    let Some(creds) = store.credentials(AccountStore::credential_key(account)) else {
        return account_usage_from_fireworks(account, &FireworksBilling::default(), "needs_login");
    };
    match fetch_fireworks_billing(client, &creds).await {
        Ok(billing) => {
            let status = if billing.detail_suffix.is_none() && billing.period.is_none() {
                "needs_setup"
            } else {
                "ok"
            };
            account_usage_from_fireworks(account, &billing, status)
        }
        Err(Some(404)) => {
            account_usage_from_fireworks(account, &FireworksBilling::default(), "needs_setup")
        }
        Err(status) => account_usage_from_fireworks(
            account,
            &FireworksBilling::default(),
            status_for_failure(status),
        ),
    }
}

pub(super) async fn poll_novita(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
    let Some(creds) = store.credentials(AccountStore::credential_key(account)) else {
        return account_usage_from_novita(account, &NovitaBalance::default(), "needs_login");
    };
    match fetch_novita_balance(client, &creds).await {
        Ok(balance) => {
            let status = if balance.detail_suffix.is_none() {
                "needs_setup"
            } else {
                "ok"
            };
            account_usage_from_novita(account, &balance, status)
        }
        Err(status) => account_usage_from_novita(
            account,
            &NovitaBalance::default(),
            status_for_failure(status),
        ),
    }
}

fn amp_status(status: Option<u16>) -> &'static str {
    match status {
        Some(401) | Some(403) => "needs_login",
        Some(429) => "throttled",
        _ => "experimental_error",
    }
}

fn zai_status(status: Option<u16>) -> &'static str {
    match status {
        Some(401) => "needs_login",
        Some(403) | Some(404) => "needs_setup",
        Some(429) => "throttled",
        _ => "error",
    }
}

pub(super) async fn poll_amp(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
    let Some(creds) = store.credentials(AccountStore::credential_key(account)) else {
        return account_usage_from_amp(account, &AmpBalance::default(), "needs_login");
    };
    match fetch_amp_balance(client, &creds).await {
        Ok(balance) => {
            let status = if balance.period.is_none() && balance.detail_suffix.is_none() {
                "needs_setup"
            } else {
                "ok"
            };
            account_usage_from_amp(account, &balance, status)
        }
        Err(status) => account_usage_from_amp(account, &AmpBalance::default(), amp_status(status)),
    }
}

pub(super) async fn poll_zai(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
    let Some(creds) = store.credentials(AccountStore::credential_key(account)) else {
        return account_usage_from_zai(account, &ZaiQuota::default(), "needs_login");
    };
    match fetch_zai_quota(client, &creds).await {
        Ok(quota) if quota.is_empty() => account_usage_from_zai(account, &quota, "needs_setup"),
        Ok(quota) => account_usage_from_zai(account, &quota, "ok"),
        Err(status) => account_usage_from_zai(account, &ZaiQuota::default(), zai_status(status)),
    }
}

pub(super) async fn poll_bailian(_store: &AccountStore, account: &Account) -> AccountUsage {
    match crate::import::fetch_bailian_token_plan_json() {
        Ok(root) => {
            let quota = parse_bailian_token_plan(&root);
            let status = if quota.five_hour.is_some() || quota.week.is_some() {
                "ok"
            } else {
                "needs_setup"
            };
            account_usage_from_bailian(account, &quota, status)
        }
        Err(_) => account_usage_from_bailian(account, &BailianQuota::default(), "needs_setup"),
    }
}

fn copilot_status(status: Option<u16>) -> &'static str {
    match status {
        Some(401) | Some(403) => "needs_login",
        Some(404) => "needs_setup",
        Some(429) => "throttled",
        _ => "experimental_error",
    }
}

fn empty_windsurf() -> WindsurfQuota {
    WindsurfQuota::default()
}

fn windsurf_status(
    session_id: &str,
    expected_id: &str,
    fetch: Result<(), Option<u16>>,
) -> &'static str {
    if session_id != expected_id {
        "identity_changed"
    } else {
        match fetch {
            Ok(()) => "ok",
            Err(Some(401) | Some(403)) => "needs_login",
            Err(_) => "experimental_error",
        }
    }
}

fn windsurf_quota_status(quota: &WindsurfQuota) -> &'static str {
    if quota.five_hour.is_none() && quota.week.is_none() {
        "experimental_error"
    } else {
        "ok"
    }
}

pub(super) async fn poll_copilot(
    store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
    let Some(creds) = store.credentials(AccountStore::credential_key(account)) else {
        return account_usage_from_copilot(account, &CopilotQuota::default(), "needs_login");
    };
    match fetch_copilot_user(client, &creds).await {
        Ok(quota) => {
            let status = if quota.period.is_none() && quota.detail_suffix.is_none() {
                "needs_setup"
            } else {
                "ok"
            };
            account_usage_from_copilot(account, &quota, status)
        }
        Err(status) => {
            account_usage_from_copilot(account, &CopilotQuota::default(), copilot_status(status))
        }
    }
}

pub(super) async fn poll_windsurf(
    _store: &AccountStore,
    client: &reqwest::Client,
    account: &Account,
) -> AccountUsage {
    use usage_core::account::AuthSource;

    let (database_path, expected_identity) = match &account.auth_source {
        AuthSource::WindsurfDatabase {
            database_path,
            expected_identity,
        } => (database_path.clone(), expected_identity.clone()),
        _ => {
            return account_usage_from_windsurf(account, &empty_windsurf(), "needs_login");
        }
    };

    let session = match crate::windsurf_local::read_windsurf_session(&database_path) {
        Ok(s) => s,
        Err(_) => {
            return account_usage_from_windsurf(account, &empty_windsurf(), "needs_login");
        }
    };

    let identity_status = windsurf_status(&session.identity, &expected_identity, Ok(()));
    if identity_status != "ok" {
        return account_usage_from_windsurf(account, &empty_windsurf(), identity_status);
    }

    match fetch_windsurf_status(client, &session.api_key).await {
        Ok(mut quota) => {
            if quota.email.is_none() {
                quota.email = session.email.clone();
            }
            if quota.plan.is_none() {
                quota.plan = session.plan.clone();
            }
            account_usage_from_windsurf(account, &quota, windsurf_quota_status(&quota))
        }
        Err(status) => account_usage_from_windsurf(
            account,
            &empty_windsurf(),
            windsurf_status(&session.identity, &expected_identity, Err(status)),
        ),
    }
}

#[cfg(test)]
#[path = "providers_pro_tests.rs"]
mod tests;
