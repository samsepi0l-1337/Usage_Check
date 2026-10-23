use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{Duration, Utc};
use ed25519_dalek::SigningKey;
use std::ffi::OsString;
use tiny_http::{Header, Response, Server};

// ---------------------------------------------------------------------
// Env-var test harness. Shared lock with `status_tests.rs`,
// `menu_actions_tests.rs`, and `paths.rs` — see
// `super::LICENSE_ENV_LOCK` for why a single lock is required.
// ---------------------------------------------------------------------

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(previous) => std::env::set_var(self.key, previous),
            None => std::env::remove_var(self.key),
        }
    }
}

fn test_signing_key() -> SigningKey {
    SigningKey::from_bytes(&[11u8; 32])
}

/// Bound on how long the mock server thread waits for its one request
/// before giving up. Keeps a future test-isolation mistake (e.g. two tests
/// racing on `USAGECHECK_LICENSE_API`) a fast, loud FAILURE instead of a
/// hang — see `join_mock_server` below for the matching bounded join.
const MOCK_SERVER_RECV_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Bound on how long [`join_mock_server`] waits for the server thread to
/// finish before failing the test outright. Deliberately longer than
/// `MOCK_SERVER_RECV_TIMEOUT` so a server that legitimately times out
/// waiting for a request still has time to observably finish first.
const MOCK_SERVER_JOIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Spawns a `tiny_http` server on an OS-assigned localhost port that answers
/// exactly ONE request with the given status/body, then exits. Returns the
/// server's base URL and a join handle — join it with [`join_mock_server`]
/// (never a bare `.join()`) so a test that never receives its request fails
/// loudly and quickly instead of hanging the whole suite forever.
fn spawn_one_shot_server(status: u16, body: String) -> (String, std::thread::JoinHandle<()>) {
    let server = Server::http("127.0.0.1:0").expect("bind mock license server");
    let port = server
        .server_addr()
        .to_ip()
        .expect("mock server should bind an IPv4/IPv6 address")
        .port();
    let url = format!("http://127.0.0.1:{port}/");
    let handle = std::thread::spawn(
        move || match server.recv_timeout(MOCK_SERVER_RECV_TIMEOUT) {
            Ok(Some(mut request)) => {
                // Drain the request body so the client's write doesn't block on
                // a full socket buffer for a body we don't otherwise inspect.
                let mut discard = String::new();
                let _ = request.as_reader().read_to_string(&mut discard);
                let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .expect("static content-type header is valid");
                let response = Response::from_string(body)
                    .with_status_code(status)
                    .with_header(header);
                let _ = request.respond(response);
            }
            Ok(None) => {
                eprintln!(
                    "mock server received no request within {MOCK_SERVER_RECV_TIMEOUT:?} — did \
                 another test overwrite USAGECHECK_LICENSE_API?"
                );
            }
            Err(err) => eprintln!("mock server recv_timeout failed: {err}"),
        },
    );
    (url, handle)
}

/// Joins a mock-server thread with a bounded wait, FAILING the test with a
/// clear message rather than blocking forever if the thread has not
/// finished in time. A defensive backstop layered on top of
/// `spawn_one_shot_server`'s own bounded `recv_timeout` above, so neither a
/// stuck server nor a stuck client under test can hang the suite.
fn join_mock_server<T>(handle: std::thread::JoinHandle<T>) -> T {
    let deadline = std::time::Instant::now() + MOCK_SERVER_JOIN_TIMEOUT;
    while !handle.is_finished() {
        if std::time::Instant::now() >= deadline {
            panic!(
                "mock server did not finish within {MOCK_SERVER_JOIN_TIMEOUT:?} — did another \
                 test overwrite USAGECHECK_LICENSE_API? (or the mock server / client under test \
                 is genuinely stuck)"
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    handle.join().expect("mock server thread should not panic")
}

fn signed_success_body(signing_key: &SigningKey, payload: &super::token::TokenPayload) -> String {
    let tok = super::token::encode_token(payload, signing_key);
    serde_json::json!({ "token": tok }).to_string()
}

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::await_holding_lock)]
async fn auto_activate_from_key_in_persists_pro_on_signed_200() {
    let _env_lock = super::LICENSE_ENV_LOCK
        .lock()
        .expect("license environment lock should not be poisoned");
    let tmp = tempfile::tempdir().expect("create temporary app-data directory");
    let signing_key = test_signing_key();
    let device_id = super::device::device_id_in(Some(tmp.path()));
    let payload = super::token::TokenPayload {
        v: 1,
        key_id: "debug-startup-http-test".to_string(),
        plan: "pro".to_string(),
        device: device_id,
        issued_at: Utc::now() - Duration::hours(1),
        expires_at: None,
    };
    let (server_url, server_handle) =
        spawn_one_shot_server(200, signed_success_body(&signing_key, &payload));
    let public_key = STANDARD.encode(signing_key.verifying_key().to_bytes());
    let _pubkey_env = EnvVarGuard::set("USAGECHECK_LICENSE_PUBKEY", &public_key);
    let _api_env = EnvVarGuard::set("USAGECHECK_LICENSE_API", &server_url);

    let result = super::auto_activate_from_key_in(Some(tmp.path()), true, Some("dev")).await;

    join_mock_server(server_handle);
    assert_eq!(
        result.expect("signed activation response should succeed"),
        Some(LicenseStatus::Pro { expires_at: None })
    );
    assert!(
        tmp.path().join("license.json").is_file(),
        "successful activation should persist license.json"
    );
    assert_eq!(
        super::status_in(Some(tmp.path())),
        LicenseStatus::Pro { expires_at: None }
    );
}

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::await_holding_lock)]
async fn auto_activate_from_key_in_server_rejected_writes_nothing() {
    let _env_lock = super::LICENSE_ENV_LOCK
        .lock()
        .expect("license environment lock should not be poisoned");
    let tmp = tempfile::tempdir().expect("create temporary app-data directory");
    let (server_url, server_handle) =
        spawn_one_shot_server(400, r#"{"error":"invalid_key","message":"no"}"#.to_string());
    let _api_env = EnvVarGuard::set("USAGECHECK_LICENSE_API", &server_url);

    let result =
        super::auto_activate_from_key_in(Some(tmp.path()), true, Some("rejected-key")).await;

    join_mock_server(server_handle);
    let error = result.expect_err("server rejection should fail activation");
    assert_eq!(
        error.classify(),
        super::ActivationErrorClass::ServerRejected
    );
    assert!(
        !tmp.path().join("license.json").exists(),
        "a rejected activation must not persist a license record"
    );
}

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::await_holding_lock)]
async fn auto_activate_from_key_in_release_is_noop() {
    let _env_lock = super::LICENSE_ENV_LOCK
        .lock()
        .expect("license environment lock should not be poisoned");
    let tmp = tempfile::tempdir().expect("create temporary app-data directory");

    let result = super::auto_activate_from_key_in(Some(tmp.path()), false, Some("dev")).await;

    assert_eq!(
        result.expect("release-mode auto-activation should be a no-op"),
        None
    );
    assert!(
        !tmp.path().join("license.json").exists(),
        "release-mode auto-activation must not persist a license record"
    );
}

#[test]
fn auto_activate_failure_log_is_fixed_and_contains_no_secrets() {
    let line = super::auto_activate_failure_log(super::ActivationErrorClass::ServerRejected);

    assert_eq!(line, "license: auto-activate failed; class=server-rejected");
    assert!(!line.contains("invalid"));
    assert!(!line.contains("rejected-key"));
    assert!(!line.contains("payload.signature"));
}

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::await_holding_lock)]
async fn apply_debug_startup_license_in_failed_http_does_not_seed() {
    let _env_lock = super::LICENSE_ENV_LOCK
        .lock()
        .expect("license environment lock should not be poisoned");
    let tmp = tempfile::tempdir().expect("create temporary app-data directory");
    let (server_url, server_handle) =
        spawn_one_shot_server(400, r#"{"error":"invalid_key","message":"no"}"#.to_string());
    let _api_env = EnvVarGuard::set("USAGECHECK_LICENSE_API", &server_url);

    super::apply_debug_startup_license_in(Some(tmp.path()), true, None, Some("dev"), None).await;

    join_mock_server(server_handle);
    assert!(
        !tmp.path().join("license.json").exists(),
        "failed HTTP activation must not fall through to the local seed"
    );
}

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::await_holding_lock)]
async fn apply_debug_startup_license_in_release_writes_nothing() {
    let _env_lock = super::LICENSE_ENV_LOCK
        .lock()
        .expect("license environment lock should not be poisoned");
    let tmp = tempfile::tempdir().expect("create temporary app-data directory");

    super::apply_debug_startup_license_in(Some(tmp.path()), false, None, Some("dev"), None).await;

    assert!(
        !tmp.path().join("license.json").exists(),
        "release-mode startup handling must neither activate nor seed"
    );
}
