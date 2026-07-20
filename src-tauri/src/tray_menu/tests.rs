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
