use super::*;
use std::io::Cursor;

#[test]
fn test_which_codex_checks_path() {
    let path_val = std::env::var("PATH").unwrap_or_default();
    if path_val.is_empty() {
        return;
    }
    let _: Vec<_> = std::env::split_paths(&path_val).collect();
}

/// Test 1: Real-shape JSONL exchange must fail now (bugs 1&2), pass after fix.
/// Feeds REAL app-server lines and verifies the probe parses correctly.
#[tokio::test]
async fn test_probe_codex_exchange_real_shape() {
    let account_line = r#"{"id":2,"result":{"id":"user-test","email":"test@example.com"}}"#;
    let rate_limits_line = r#"{"id":3,"result":{"primaryWindow":{"usedPercent":50.0,"windowDurationMins":60,"resetsAt":2000000000.0},"secondaryWindow":{"usedPercent":25.0,"windowDurationMins":10080,"resetsAt":2000700000.0}}}"#;
    let notification_line = r#"{"type":"sessionUpdate"}"#; // Interleaved notification (no id)

    let input = format!(
        "{}\n{}\n{}\n",
        account_line, rate_limits_line, notification_line
    );
    let reader = Cursor::new(input.as_bytes());
    let writer = Vec::new();

    let result = probe_codex_exchange(reader, writer).await;

    // After the fix, this MUST pass.
    assert!(result.is_ok(), "Probe failed: {:?}", result);
    let probe = result.unwrap();

    assert_eq!(probe.account.id, "user-test");
    assert_eq!(probe.account.email, Some("test@example.com".to_string()));
    assert!(probe.primary.is_some(), "Primary quota should be parsed");
    let prim = probe.primary.unwrap();
    assert_eq!(prim.percent, 50.0);
    assert_eq!(prim.window_seconds, Some(3600)); // 60 * 60

    assert!(
        probe.secondary.is_some(),
        "Secondary quota should be parsed"
    );
    let sec = probe.secondary.unwrap();
    assert_eq!(sec.percent, 25.0);
    assert_eq!(sec.window_seconds, Some(604800)); // 10080 * 60
}

#[tokio::test]
async fn test_probe_codex_exchange_missing_rate_limits_rejected() {
    let account_line = r#"{"id":2,"result":{"id":"user-test","email":"test@example.com"}}"#;
    let rate_limits_line = r#"{"id":3}"#;
    let input = format!("{}\n{}\n", account_line, rate_limits_line);

    let result = probe_codex_exchange(Cursor::new(input.as_bytes()), Vec::new()).await;

    let error = result.expect_err("missing rate limits should be rejected");
    assert_eq!(error, "rate limits parse error: no rate_limits in response");
}

/// Test 2: Hung-child timeout — reader never yields a line.
/// Wraps probe_codex_exchange in a short timeout; must return timeout error, not panic.
#[tokio::test]
async fn test_probe_codex_exchange_hung_child() {
    // A reader that never yields (stuck in read).
    struct HungReader;

    impl tokio::io::AsyncRead for HungReader {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            // Never complete, simulating a hung child.
            std::task::Poll::Pending
        }
    }

    impl tokio::io::AsyncBufRead for HungReader {
        fn poll_fill_buf(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<&[u8]>> {
            std::task::Poll::Pending
        }

        fn consume(self: std::pin::Pin<&mut Self>, _amt: usize) {}
    }

    let reader = HungReader;
    let writer = Vec::new();

    // Wrap in a SHORT timeout (200ms, not 10s) so test is fast.
    let future = probe_codex_exchange(reader, writer);
    let result = tokio::time::timeout(std::time::Duration::from_millis(200), future).await;

    assert!(result.is_err(), "Should timeout on hung reader, not panic");
}

/// Test 3: Identity rejection — null or API-key identity must return Err.
#[tokio::test]
async fn test_probe_codex_exchange_null_identity_rejected() {
    let account_line_null = r#"{"id":2,"result":null}"#;
    let rate_limits_line = r#"{"id":3,"result":{"primaryWindow":{"usedPercent":50.0,"windowDurationMins":60,"resetsAt":2000000000.0}}}"#;

    let input = format!("{}\n{}\n", account_line_null, rate_limits_line);
    let reader = Cursor::new(input.as_bytes());
    let writer = Vec::new();

    let result = probe_codex_exchange(reader, writer).await;

    // FIX BUG 4: Rejection must surface as Err, not Ok with empty identity.
    assert!(result.is_err(), "Null identity should be rejected as error");
}

/// Test 4: API-key identity rejection.
#[tokio::test]
async fn test_probe_codex_exchange_api_key_rejected() {
    let account_line_apikey =
        r#"{"id":2,"result":{"id":"sk-proj-123abc","email":"sk-proj@openai.com"}}"#;
    let rate_limits_line = r#"{"id":3,"result":{"primaryWindow":{"usedPercent":50.0,"windowDurationMins":60,"resetsAt":2000000000.0}}}"#;

    let input = format!("{}\n{}\n", account_line_apikey, rate_limits_line);
    let reader = Cursor::new(input.as_bytes());
    let writer = Vec::new();

    let result = probe_codex_exchange(reader, writer).await;

    // API-key accounts must be rejected.
    assert!(
        result.is_err(),
        "API-key identity should be rejected as error"
    );
}

#[test]
fn test_cli_adapter_login_command_clears_env_vars() {
    let adapter = CodexCliAdapter;
    let cmd = adapter.login_command(std::path::Path::new("/tmp/profile"));

    assert_eq!(cmd.executable.to_string_lossy(), "codex");
    assert!(cmd.args.contains(&OsString::from("login")));

    let env_remove_strs: Vec<String> = cmd
        .env_remove
        .iter()
        .map(|s| s.to_string_lossy().to_string())
        .collect();
    assert!(env_remove_strs.contains(&"CODEX_ACCESS_TOKEN".to_string()));
    assert!(env_remove_strs.contains(&"OPENAI_API_KEY".to_string()));
}

#[test]
fn test_cli_adapter_login_command_sets_codex_home() {
    let adapter = CodexCliAdapter;
    let profile = std::path::Path::new("/tmp/test_profile");
    let cmd = adapter.login_command(profile);

    let env_vars: Vec<(String, String)> = cmd
        .env
        .iter()
        .map(|(k, v)| {
            (
                k.to_string_lossy().to_string(),
                v.to_string_lossy().to_string(),
            )
        })
        .collect();

    assert!(env_vars
        .iter()
        .any(|(k, v)| k == "CODEX_HOME" && v.contains("test_profile")));
}
