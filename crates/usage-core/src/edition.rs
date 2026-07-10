//! Compile-time edition helpers (Free vs Pro).

use crate::account::Provider;

#[cfg(all(feature = "edition-free", feature = "edition-pro"))]
compile_error!("edition-free and edition-pro are mutually exclusive; build with --no-default-features");

/// `"free"` or `"pro"` depending on the enabled edition feature.
pub fn edition_id() -> &'static str {
    if cfg!(feature = "edition-pro") {
        "pro"
    } else {
        "free"
    }
}

/// Human-readable edition label for UI strings.
pub fn edition_label() -> &'static str {
    if cfg!(feature = "edition-pro") {
        "Pro"
    } else {
        "Free"
    }
}

/// Providers included in every build (Codex, Claude, Gemini/agy).
pub fn free_providers() -> &'static [Provider] {
    &[Provider::Codex, Provider::Claude, Provider::Agy]
}

/// Pro-only providers (Cursor, Grok, Higgsfield).
#[cfg(feature = "edition-pro")]
pub fn paid_providers() -> &'static [Provider] {
    &[
        Provider::Cursor,
        Provider::Grok,
        Provider::Higgsfield,
    ]
}

/// All providers compiled into this edition.
pub fn all_providers() -> Vec<Provider> {
    #[cfg(feature = "edition-pro")]
    {
        let mut providers = free_providers().to_vec();
        providers.extend_from_slice(paid_providers());
        providers
    }
    #[cfg(not(feature = "edition-pro"))]
    {
        free_providers().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edition_id_matches_feature() {
        if cfg!(feature = "edition-pro") {
            assert_eq!(edition_id(), "pro");
            assert_eq!(edition_label(), "Pro");
        } else {
            assert_eq!(edition_id(), "free");
            assert_eq!(edition_label(), "Free");
        }
    }

    #[test]
    fn free_providers_are_codex_claude_agy() {
        assert_eq!(free_providers().len(), 3);
    }

    #[cfg(feature = "edition-pro")]
    #[test]
    fn pro_build_includes_paid_providers() {
        assert_eq!(paid_providers().len(), 3);
        assert_eq!(all_providers().len(), 6);
    }

    #[cfg(feature = "edition-free")]
    #[test]
    fn free_build_excludes_paid_providers() {
        assert_eq!(all_providers().len(), 3);
    }
}
