//! Provider edition helpers (Free vs Pro is now a runtime license gate).

use crate::account::Provider;

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

/// All providers compiled into this build.
pub fn all_providers() -> Vec<Provider> {
    let mut providers = free_providers().to_vec();
    providers.extend_from_slice(paid_providers());
    providers
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
