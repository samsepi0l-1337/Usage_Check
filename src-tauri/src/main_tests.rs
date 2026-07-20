use super::*;

#[test]
fn browser_oauth_always_routes_to_oauth() {
    assert_eq!(
        classify_auth_action(Provider::Codex, AuthMethod::BrowserOAuth),
        AuthAction::Oauth
    );
    assert_eq!(
        classify_auth_action(Provider::Claude, AuthMethod::BrowserOAuth),
        AuthAction::Oauth
    );
    assert_eq!(
        classify_auth_action(Provider::Agy, AuthMethod::BrowserOAuth),
        AuthAction::Oauth
    );
}

#[test]
fn cli_routes_codex_and_claude_to_coordinator() {
    assert_eq!(
        classify_auth_action(Provider::Codex, AuthMethod::Cli),
        AuthAction::CliCoordinator
    );
    assert_eq!(
        classify_auth_action(Provider::Claude, AuthMethod::Cli),
        AuthAction::CliCoordinator
    );
}

#[test]
fn cli_routes_non_coordinator_providers_to_import() {
    // Agy has no CLI capability in the registry, but the classifier is total:
    // any non-Codex/Claude provider under Cli must fall through to Import.
    assert_eq!(
        classify_auth_action(Provider::Agy, AuthMethod::Cli),
        AuthAction::Import
    );
}

#[test]
fn local_database_and_env_route_to_import() {
    assert_eq!(
        classify_auth_action(Provider::Codex, AuthMethod::LocalDatabase),
        AuthAction::Import
    );
    assert_eq!(
        classify_auth_action(Provider::Codex, AuthMethod::ManagementKeyEnvironment),
        AuthAction::Import
    );
}

#[test]
fn clipboard_routes_to_grok_clipboard() {
    assert_eq!(
        classify_auth_action(Provider::Codex, AuthMethod::ManagementKeyClipboard),
        AuthAction::GrokClipboard
    );
}

#[cfg(feature = "edition-pro")]
#[test]
fn pro_providers_route_per_capability() {
    assert_eq!(
        classify_auth_action(Provider::Cursor, AuthMethod::LocalDatabase),
        AuthAction::Import
    );
    assert_eq!(
        classify_auth_action(Provider::Grok, AuthMethod::ManagementKeyClipboard),
        AuthAction::GrokClipboard
    );
    assert_eq!(
        classify_auth_action(Provider::Grok, AuthMethod::ManagementKeyEnvironment),
        AuthAction::Import
    );
    assert_eq!(
        classify_auth_action(Provider::Higgsfield, AuthMethod::Cli),
        AuthAction::Import
    );
}

// Registry-consistency: every (provider, method) actually wired into the tray registry
// classifies to the action the current dispatcher would have taken — guards against future
// registry drift. Edition-aware automatically because auth_action_specs() is cfg-gated.
#[test]
fn registry_specs_classify_as_expected() {
    for spec in crate::tray_menu::auth_action_specs() {
        let action = classify_auth_action(spec.provider, spec.method);
        let expected = match spec.method {
            AuthMethod::BrowserOAuth => AuthAction::Oauth,
            AuthMethod::Cli => match spec.provider {
                Provider::Codex | Provider::Claude => AuthAction::CliCoordinator,
                _ => AuthAction::Import,
            },
            AuthMethod::LocalDatabase | AuthMethod::ManagementKeyEnvironment => {
                AuthAction::Import
            }
            AuthMethod::ManagementKeyClipboard => AuthAction::GrokClipboard,
        };
        assert_eq!(
            action, expected,
            "spec {:?}/{:?} misrouted",
            spec.provider, spec.method
        );
    }
}
