//! Provider edition helpers (Free vs Pro is now a runtime license gate).

use std::collections::BTreeSet;

use crate::account::{Account, Provider};

/// Providers included in every build (Codex, Claude, Gemini/agy).
pub fn free_providers() -> &'static [Provider] {
    &[Provider::Codex, Provider::Claude, Provider::Agy]
}

/// Providers that require a Pro license to use at runtime (Cursor, Grok, Higgsfield).
pub fn paid_providers() -> &'static [Provider] {
    &[Provider::Cursor, Provider::Grok, Provider::Higgsfield]
}

/// True when `provider` requires a Pro license at runtime.
pub fn requires_pro(provider: Provider) -> bool {
    paid_providers().contains(&provider)
}

/// How many accounts per FREE provider (Codex, Claude, Agy) a Free-state
/// installation may keep ACTIVE. Paid providers are deliberately not capped
/// here: in Free they are already gated wholesale by [`requires_pro`] plus the
/// `pro_required` poll path, so a per-provider count would be dead policy.
pub const FREE_ACCOUNTS_PER_PROVIDER: usize = 1;

/// The single user-facing sentence explaining the Free-state cap.
///
/// Lives here, rather than beside either enforcement point, because BOTH the
/// store's add-time rejection and the tray's disabled-entry / refusal rows must
/// show the identical text, and those live in modules that must not depend on
/// each other (D8). `usage_core` already owns user-facing provider strings
/// (`Provider::display_name`).
pub fn free_limit_reason(provider: Provider) -> String {
    format!(
        "{} already has {} account on the Free plan — activate a Pro license to add another. \
         Your existing accounts are kept.",
        provider.display_name(),
        FREE_ACCOUNTS_PER_PROVIDER,
    )
}

/// True when a Free-state install must REFUSE another `provider` account,
/// given the accounts already in the index.
///
/// Pure and license-free by construction: Free-ness is the caller's to decide
/// — call this only when the runtime is not Pro. Always false for a paid
/// provider, whose Free-state gate is [`requires_pro`], not a count.
pub fn free_limit_reached(accounts: &[Account], provider: Provider) -> bool {
    if requires_pro(provider) {
        return false;
    }
    accounts
        .iter()
        .filter(|account| account.provider == provider)
        .count()
        >= FREE_ACCOUNTS_PER_PROVIDER
}

/// The ids of accounts a Free-state install must report as `pro_required`:
/// every free-provider account BEYOND the first [`FREE_ACCOUNTS_PER_PROVIDER`]
/// for that provider, ranked by **index order**.
///
/// Index order is insertion order and is stable across restarts — the store
/// pushes new accounts onto the end and `Vec::remove` preserves the relative
/// order of the rest — so removing the retained account automatically promotes
/// the next one, with no id stored anywhere and nothing to migrate.
///
/// Never returns a paid-provider id: those are gated by [`requires_pro`] on a
/// different path, and returning them here would double-report one rule.
pub fn free_surplus_account_ids(accounts: &[Account]) -> BTreeSet<String> {
    let mut surplus = BTreeSet::new();
    for (index, account) in accounts.iter().enumerate() {
        if requires_pro(account.provider) {
            continue;
        }
        let rank = accounts[..index]
            .iter()
            .filter(|earlier| earlier.provider == account.provider)
            .count();
        if rank >= FREE_ACCOUNTS_PER_PROVIDER {
            surplus.insert(account.id.clone());
        }
    }
    surplus
}

/// All providers compiled into this build.
pub fn all_providers() -> Vec<Provider> {
    let mut providers = free_providers().to_vec();
    providers.extend_from_slice(paid_providers());
    providers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::AuthSource;

    fn account(id: &str, provider: Provider) -> Account {
        Account {
            id: id.into(),
            provider,
            label: format!("{id}@example.com"),
            auth_source: AuthSource::BrowserOAuth {
                credential_id: format!("cred-{id}"),
            },
        }
    }

    #[test]
    fn free_providers_are_codex_claude_agy() {
        assert_eq!(free_providers().len(), 3);
    }

    #[test]
    fn all_providers_includes_free_and_paid() {
        assert_eq!(paid_providers().len(), 3);
        assert_eq!(all_providers().len(), 6);
    }

    #[test]
    fn requires_pro_matches_paid_providers() {
        assert!(requires_pro(Provider::Cursor));
        assert!(requires_pro(Provider::Grok));
        assert!(requires_pro(Provider::Higgsfield));
        assert!(!requires_pro(Provider::Codex));
        assert!(!requires_pro(Provider::Claude));
        assert!(!requires_pro(Provider::Agy));
    }

    #[test]
    fn free_limit_reached_is_false_for_an_empty_index() {
        for provider in [Provider::Codex, Provider::Claude, Provider::Agy] {
            assert!(!free_limit_reached(&[], provider));
        }
    }

    #[test]
    fn free_limit_reached_is_true_at_exactly_one_existing_account() {
        let accounts = [account("codex-1", Provider::Codex)];

        assert!(free_limit_reached(&accounts, Provider::Codex));
        assert!(!free_limit_reached(&accounts, Provider::Claude));
        assert!(!free_limit_reached(&accounts, Provider::Agy));
    }

    #[test]
    fn free_limit_reached_is_always_false_for_paid_providers() {
        for provider in [Provider::Cursor, Provider::Grok, Provider::Higgsfield] {
            let accounts = [
                account("paid-1", provider),
                account("paid-2", provider),
                account("paid-3", provider),
            ];
            assert!(!free_limit_reached(&accounts, provider));
        }
    }

    #[test]
    fn free_surplus_is_empty_for_one_account_per_provider() {
        let accounts = [
            account("codex-1", Provider::Codex),
            account("claude-1", Provider::Claude),
            account("agy-1", Provider::Agy),
        ];

        assert!(free_surplus_account_ids(&accounts).is_empty());
    }

    #[test]
    fn free_surplus_keeps_the_first_and_gates_the_rest_per_provider() {
        let accounts = [
            account("codex-1", Provider::Codex),
            account("codex-2", Provider::Codex),
            account("claude-1", Provider::Claude),
            account("codex-3", Provider::Codex),
        ];

        assert_eq!(
            free_surplus_account_ids(&accounts),
            BTreeSet::from(["codex-2".into(), "codex-3".into()])
        );
    }

    #[test]
    fn free_surplus_gates_extra_agy_accounts() {
        let accounts = [
            account("agy-1", Provider::Agy),
            account("agy-2", Provider::Agy),
            account("agy-3", Provider::Agy),
        ];

        assert_eq!(
            free_surplus_account_ids(&accounts),
            BTreeSet::from(["agy-2".into(), "agy-3".into()])
        );
    }

    #[test]
    fn free_surplus_never_contains_a_paid_provider_id() {
        let accounts = [
            account("grok-1", Provider::Grok),
            account("grok-2", Provider::Grok),
            account("grok-3", Provider::Grok),
            account("cursor-1", Provider::Cursor),
            account("cursor-2", Provider::Cursor),
            account("cursor-3", Provider::Cursor),
        ];

        assert!(free_surplus_account_ids(&accounts).is_empty());
    }

    #[test]
    fn free_surplus_promotes_the_next_account_when_the_first_is_removed() {
        let accounts = [
            account("codex-1", Provider::Codex),
            account("codex-2", Provider::Codex),
        ];

        assert_eq!(
            free_surplus_account_ids(&accounts),
            BTreeSet::from(["codex-2".into()])
        );
        assert!(free_surplus_account_ids(&accounts[1..]).is_empty());
    }

    #[test]
    fn free_limit_reason_names_the_provider_and_leaves_no_placeholder() {
        let reason = free_limit_reason(Provider::Codex);

        assert!(reason.contains(Provider::Codex.display_name()));
        assert!(reason.contains("Pro license"));
        assert!(!reason.contains('{'));
    }
}
