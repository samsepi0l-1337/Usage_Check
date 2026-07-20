use super::*;

#[test]
fn pkce_challenge_is_url_safe_no_padding() {
    let (verifier, challenge) = make_pkce();
    assert!(verifier.len() >= 43);
    assert!(!challenge.contains('=') && !challenge.contains('+') && !challenge.contains('/'));
}

#[test]
fn authorize_url_contains_params() {
    let cfg = ProviderOAuth {
        client_id: "cid".into(),
        client_secret: None,
        auth_url: "https://auth.example/authorize".into(),
        token_url: "https://auth.example/token".into(),
        scopes: "openid".into(),
        fixed_redirect: None,
        extra_authorize_params: vec![("id_token_add_organizations".into(), "true".into())],
        use_pkce: true,
    };
    let url = build_authorize_url(&cfg, "chal", "http://localhost:1455/auth/callback", "st8");
    assert!(url.contains("client_id=cid"));
    assert!(url.contains("code_challenge=chal"));
    assert!(url.contains("code_challenge_method=S256"));
    assert!(url.contains("state=st8"));
    assert!(url.contains("id_token_add_organizations=true"));
    assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"));
}

#[test]
fn authorize_url_skips_pkce_when_disabled() {
    let cfg = ProviderOAuth {
        client_id: "cid".into(),
        client_secret: Some("sec".into()),
        auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
        token_url: "https://oauth2.googleapis.com/token".into(),
        scopes: "openid".into(),
        fixed_redirect: None,
        extra_authorize_params: vec![("access_type".into(), "offline".into())],
        use_pkce: false,
    };
    let url = build_authorize_url(&cfg, "chal", "http://127.0.0.1:9/callback", "st8");
    assert!(!url.contains("code_challenge"));
    assert!(url.contains("access_type=offline"));
}

#[test]
fn codex_config_matches_cli_contract() {
    let cfg = config(Provider::Codex).unwrap();
    assert_eq!(cfg.client_id, "app_EMoamEEZ73f0CkXaXp7hrann");
    assert_eq!(cfg.fixed_redirect, Some((1455, "/auth/callback")));
    assert!(cfg.scopes.contains("api.connectors.read"));
    assert!(cfg.use_pkce);
    assert!(cfg
        .extra_authorize_params
        .iter()
        .any(|(k, v)| k == "codex_cli_simplified_flow" && v == "true"));
}

#[test]
fn agy_config_resolves_from_env() {
    std::env::set_var(
        "ANTIGRAVITY_OAUTH_CLIENT_ID",
        "1071006060591-test.apps.googleusercontent.com",
    );
    std::env::set_var("ANTIGRAVITY_OAUTH_CLIENT_SECRET", "GOCSPX-test-secret-value");
    let cfg = config(Provider::Agy).unwrap();
    assert!(cfg.client_id.contains("apps.googleusercontent.com"));
    assert!(cfg.client_secret.is_some());
    assert!(!cfg.use_pkce);
    assert!(cfg.scopes.contains("cloud-platform"));
    std::env::remove_var("ANTIGRAVITY_OAUTH_CLIENT_ID");
    std::env::remove_var("ANTIGRAVITY_OAUTH_CLIENT_SECRET");
}

#[test]
fn extract_google_oauth_pair_prefers_enterprise_prefix() {
    // Mimic Antigravity packing: two GOCSPX secrets concatenated, then a URL.
    let blob = concat!(
        "\0",
        "884354919052-otherclientid000000000000000.apps.googleusercontent.com",
        "\0",
        "GOCSPX-AAAA1111BBBB2222CCCC3333",
        "GOCSPX-EEEE5555FFFF6666GGGG7777",
        "https://cloudcode-pa.googleapis.com",
        "\0",
        "1071006060591-exampleclientid000000000000000.apps.googleusercontent.com",
        "\0",
    )
    .as_bytes();
    let (id, secret) = extract_google_oauth_pair(blob).unwrap();
    assert!(id.starts_with("1071006060591-"), "{id}");
    assert!(id.ends_with(".apps.googleusercontent.com"), "{id}");
    // First of the concatenated pair (enterprise secret), not the second,
    // and never with a trailing `https` glue.
    assert_eq!(secret, "GOCSPX-AAAA1111BBBB2222CCCC3333");
    assert!(!secret.contains("http"));
}

#[test]
fn extract_splits_concatenated_gocspx_secrets() {
    let blob = b"1071006060591-exampleclientid000000000000000.apps.googleusercontent.com\0\
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PX-SECONDSECRETVALUEHERE00https://x";
    let (_id, secret) = extract_google_oauth_pair(blob).unwrap();
    assert_eq!(secret, "GOCSPX-FIRSTSECRETVALUEHERE0000");
    assert!(!secret[7..].contains("GOCSPX-"));
    assert!(!secret.contains("http"));
}

#[test]
fn agy_config_uses_registered_localhost_callback() {
    std::env::set_var(
        "ANTIGRAVITY_OAUTH_CLIENT_ID",
        "1071006060591-test.apps.googleusercontent.com",
    );
    std::env::set_var("ANTIGRAVITY_OAUTH_CLIENT_SECRET", "GOCSPX-test-secret-value000");
    let cfg = config(Provider::Agy).unwrap();
    assert_eq!(cfg.fixed_redirect, Some((8080, "/callback")));
    assert!(!cfg.use_pkce);
    std::env::remove_var("ANTIGRAVITY_OAUTH_CLIENT_ID");
    std::env::remove_var("ANTIGRAVITY_OAUTH_CLIENT_SECRET");
}

#[test]
fn resolve_agy_oauth_client_from_local_install_when_present() {
    // GUI tray apps often have no env vars; resolution must succeed from
    // a local Antigravity/agy install when available.
    std::env::remove_var("ANTIGRAVITY_OAUTH_CLIENT_ID");
    std::env::remove_var("ANTIGRAVITY_OAUTH_CLIENT_SECRET");
    match resolve_agy_oauth_client() {
        Ok((id, secret)) => {
            assert!(id.contains("apps.googleusercontent.com"), "{id}");
            assert!(secret.starts_with("GOCSPX-"), "secret shape");
            assert!(!secret[7..].contains("GOCSPX-"), "must not concatenate secrets");
        }
        Err(e) => {
            // CI / machines without Antigravity installed.
            assert!(
                e.contains("Antigravity OAuth credentials not found"),
                "{e}"
            );
        }
    }
}

#[test]
fn codex_and_claude_config_present() {
    assert!(config(Provider::Codex).is_ok());
    assert!(config(Provider::Claude).is_ok());
}

#[test]
fn claude_scopes_match_cli_keychain_contract() {
    let cfg = config(Provider::Claude).unwrap();
    assert!(cfg.scopes.contains("user:sessions:claude_code"));
    assert!(cfg.scopes.contains("user:inference"));
    assert!(!cfg.scopes.contains("org:create_api_key"));
    assert!(cfg.use_pkce);
}

#[test]
fn parse_callback_query_extracts_code_and_state() {
    let params = parse_callback_query("/auth/callback?code=abc123&state=xyz").unwrap();
    assert_eq!(params.code, "abc123");
    assert_eq!(params.state, "xyz");
}

#[test]
fn parse_callback_query_none_without_query() {
    assert!(parse_callback_query("/auth/callback").is_none());
}

#[test]
fn chatgpt_account_id_from_synthetic_jwt() {
    // header.payload.sig — only payload matters; unsigned test fixture.
    let payload = URL_SAFE_NO_PAD.encode(
        br#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct-test-9"}}"#,
    );
    let jwt = format!("e30.{payload}.sig");
    assert_eq!(
        chatgpt_account_id_from_id_token(&jwt).as_deref(),
        Some("acct-test-9")
    );
}

#[test]
fn google_sub_and_chatgpt_account_id_from_id_token() {
    let payload = URL_SAFE_NO_PAD.encode(
        br#"{"sub":"116950757786684882215"}"#,
    );
    let jwt = format!("e30.{payload}.sig");
    assert_eq!(
        google_sub_from_id_token(&jwt).as_deref(),
        Some("116950757786684882215")
    );
    // ChatGPT claim must not be invented from a Google id_token.
    assert!(chatgpt_account_id_from_id_token(&jwt).is_none());
}

#[test]
fn account_id_prefers_chatgpt_then_google_sub() {
    let google = TokenResponse {
        access_token: "ya29.x".into(),
        refresh_token: None,
        id_token: {
            let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"google-sub-1"}"#);
            Some(format!("e30.{payload}.sig"))
        },
        expires_in: None,
        account_id: None,
    };
    assert_eq!(
        account_id_from_token_response(&google).as_deref(),
        Some("google-sub-1")
    );

    let chatgpt = TokenResponse {
        access_token: "at".into(),
        refresh_token: None,
        id_token: {
            let payload = URL_SAFE_NO_PAD.encode(
                br#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct-1"},"sub":"ignored"}"#,
            );
            Some(format!("e30.{payload}.sig"))
        },
        expires_in: None,
        account_id: None,
    };
    assert_eq!(
        account_id_from_token_response(&chatgpt).as_deref(),
        Some("acct-1")
    );
}

#[test]
fn should_refresh_true_when_already_expired() {
    let now = Utc::now();
    let expires_at = Some(now - chrono::Duration::seconds(5));
    assert!(should_refresh(expires_at, now, Duration::from_secs(60)));
}

#[test]
fn should_refresh_true_when_within_threshold() {
    let now = Utc::now();
    let expires_at = Some(now + chrono::Duration::seconds(30));
    assert!(should_refresh(expires_at, now, Duration::from_secs(60)));
}

#[test]
fn should_refresh_false_when_comfortably_in_future() {
    let now = Utc::now();
    let expires_at = Some(now + chrono::Duration::seconds(300));
    assert!(!should_refresh(expires_at, now, Duration::from_secs(60)));
}

#[test]
fn should_refresh_false_when_no_expiry_known() {
    let now = Utc::now();
    assert!(!should_refresh(None, now, Duration::from_secs(60)));
}

#[test]
fn should_refresh_true_at_exact_threshold_boundary() {
    let now = Utc::now();
    let expires_at = Some(now + chrono::Duration::seconds(60));
    assert!(should_refresh(expires_at, now, Duration::from_secs(60)));
}
