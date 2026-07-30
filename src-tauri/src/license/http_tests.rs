use super::*;
use crate::license::{self, device, token, watermark, LicenseRecord, LicenseStatus};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, Duration, Utc};
use ed25519_dalek::SigningKey;
use std::ffi::OsString;
use std::sync::Arc;
use tiny_http::{Header, Response, Server};

// ---------------------------------------------------------------------
// Env-var test harness. Shared lock with `status_tests.rs`,
// `menu_actions_tests.rs`, and `paths.rs` — see
// `super::super::LICENSE_ENV_LOCK` for why a single lock is required.
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
    let handle = std::thread::spawn(move || match server.recv_timeout(MOCK_SERVER_RECV_TIMEOUT) {
        Ok(Some(mut request)) => {
            // Drain the request body so the client's write doesn't block on
            // a full socket buffer for a body we don't otherwise inspect.
            let mut discard = String::new();
            let _ = request.as_reader().read_to_string(&mut discard);
            let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .expect("static content-type header is valid");
            let response = Response::from_string(body).with_status_code(status).with_header(header);
            let _ = request.respond(response);
        }
        Ok(None) => {
            eprintln!(
                "mock server received no request within {MOCK_SERVER_RECV_TIMEOUT:?} — did \
                 another test overwrite USAGECHECK_LICENSE_API?"
            );
        }
        Err(err) => eprintln!("mock server recv_timeout failed: {err}"),
    });
    (url, handle)
}

/// Joins a mock-server thread with a bounded wait, FAILING the test with a
/// clear message rather than blocking forever if the thread has not
/// finished in time. A defensive backstop layered on top of
/// `spawn_one_shot_server`'s own bounded `recv_timeout` above, so neither a
/// stuck server nor a stuck client under test can hang the suite. Generic
/// over the handle's return type so it also serves
/// [`spawn_two_shot_server`], whose handle carries the two requests'
/// arrival instants rather than `()`.
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

/// Like [`spawn_one_shot_server`], but answers exactly TWO requests and
/// reports whether the SECOND one reached the server WHILE the first was
/// still being held open (unresponded). Used by the (D) concurrency test
/// below, which needs more than "which request arrived first" to actually
/// PROVE serialization — see that test's own comment for the full
/// reasoning; the short version is that a server which simply processes
/// requests in a sequential accept loop can never observe a genuine overlap
/// regardless of whether the CLIENT actually raced two requests, because
/// the server's own code cannot get around to accepting the second
/// connection until it has finished handling the first either way. This
/// function avoids that trap by polling tiny_http's NON-BLOCKING
/// `try_recv()` — which pulls from a queue tiny_http's own background
/// listener populates independently of what this thread is doing — during
/// the window before it responds to the first request, so "the second
/// request had already reached the server" is observed directly rather
/// than inferred from when this thread's code happened to ask for it.
///
/// Returns the server's URL and a join handle carrying `overlap_detected`:
/// `true` if the second request reached tiny_http's queue before this
/// function responded to the first one, `false` otherwise.
fn spawn_two_shot_server(
    bodies: [String; 2],
    first_response_delay: std::time::Duration,
) -> (String, std::thread::JoinHandle<bool>) {
    let server = Server::http("127.0.0.1:0").expect("bind mock license server");
    let port = server
        .server_addr()
        .to_ip()
        .expect("mock server should bind an IPv4/IPv6 address")
        .port();
    let url = format!("http://127.0.0.1:{port}/");
    let [first_body, second_body] = bodies;
    let handle = std::thread::spawn(move || {
        let content_type_header = || {
            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .expect("static content-type header is valid")
        };
        let respond = |mut request: tiny_http::Request, body: String| {
            let mut discard = String::new();
            let _ = request.as_reader().read_to_string(&mut discard);
            let response = Response::from_string(body).with_status_code(200).with_header(content_type_header());
            let _ = request.respond(response);
        };

        let first_request = server
            .recv_timeout(MOCK_SERVER_RECV_TIMEOUT)
            .expect("mock server recv_timeout failed")
            .expect(
                "mock server received no first request within timeout — did another test \
                 overwrite USAGECHECK_LICENSE_API?",
            );

        // Poll (non-blocking) for the SECOND request arriving WHILE the
        // first is still held open, for up to `first_response_delay`. This
        // is the actual overlap check: `try_recv` reflects tiny_http's own
        // background acceptance, not this thread's control flow.
        let poll_deadline = std::time::Instant::now() + first_response_delay;
        let mut overlapping_second_request = None;
        while std::time::Instant::now() < poll_deadline {
            match server.try_recv() {
                Ok(Some(req)) => {
                    overlapping_second_request = Some(req);
                    break;
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(2)),
                Err(err) => {
                    eprintln!("mock server try_recv failed: {err}");
                    break;
                }
            }
        }
        let overlap_detected = overlapping_second_request.is_some();

        // NOW respond to the first request — after the polling window, so a
        // genuinely unlocked second call has had the full delay to reach
        // the server before this closes the window.
        respond(first_request, first_body);

        let second_request = match overlapping_second_request {
            Some(req) => req,
            None => server
                .recv_timeout(MOCK_SERVER_RECV_TIMEOUT)
                .expect("mock server recv_timeout failed")
                .expect(
                    "mock server received no second request within timeout — did another test \
                     overwrite USAGECHECK_LICENSE_API?",
                ),
        };
        respond(second_request, second_body);

        overlap_detected
    });
    (url, handle)
}

/// Like [`spawn_one_shot_server`], but delays its response by `delay` and
/// notifies the returned [`tokio::sync::Notify`] the INSTANT the request has
/// reached the server — i.e. the instant a client `.await`ing the response
/// is genuinely in flight (past the point where `activate_lock` is already
/// held). Used by the deactivate-vs-refresh race test below to synchronize
/// "only call deactivate once refresh is truly in flight" deterministically,
/// without guessing at a fixed sleep.
fn spawn_delayed_one_shot_server(
    status: u16,
    body: String,
    delay: std::time::Duration,
) -> (String, Arc<tokio::sync::Notify>, std::thread::JoinHandle<()>) {
    let server = Server::http("127.0.0.1:0").expect("bind mock license server");
    let port = server
        .server_addr()
        .to_ip()
        .expect("mock server should bind an IPv4/IPv6 address")
        .port();
    let url = format!("http://127.0.0.1:{port}/");
    let request_received = Arc::new(tokio::sync::Notify::new());
    let notify = request_received.clone();
    let handle = std::thread::spawn(move || match server.recv_timeout(MOCK_SERVER_RECV_TIMEOUT) {
        Ok(Some(mut request)) => {
            // The request has genuinely reached tiny_http at this point —
            // signal BEFORE the artificial delay, so the waiting side
            // observes "in flight", never "about to respond".
            notify.notify_one();
            std::thread::sleep(delay);
            let mut discard = String::new();
            let _ = request.as_reader().read_to_string(&mut discard);
            let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .expect("static content-type header is valid");
            let response = Response::from_string(body).with_status_code(status).with_header(header);
            let _ = request.respond(response);
        }
        Ok(None) => {
            eprintln!(
                "mock server received no request within {MOCK_SERVER_RECV_TIMEOUT:?} — did \
                 another test overwrite USAGECHECK_LICENSE_API?"
            );
        }
        Err(err) => eprintln!("mock server recv_timeout failed: {err}"),
    });
    (url, request_received, handle)
}

/// (A) Snapshot of both files a successful `activate`/`refresh` commits
/// together — `license.json` and `clock-watermark` — as raw bytes (`None`
/// when the file does not exist). Used to prove a REJECTED candidate leaves
/// BOTH completely untouched, not just the license record.
fn snapshot_license_files(dir: &std::path::Path) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    (
        std::fs::read(dir.join("license.json")).ok(),
        std::fs::read(dir.join("clock-watermark")).ok(),
    )
}

fn signed_success_body(signing_key: &SigningKey, payload: &token::TokenPayload) -> String {
    let tok = token::encode_token(payload, signing_key);
    serde_json::json!({ "token": tok }).to_string()
}

// ---------------------------------------------------------------------
// THE test: activate against a mock server, persist, then tamper with the
// persisted token WITHOUT re-signing, and prove `status()` fails closed.
// ---------------------------------------------------------------------

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn activate_persists_pro_and_tampering_the_persisted_token_forces_free() {
    let _lock = license::LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _pubkey_env = EnvVarGuard::set(
        "USAGECHECK_LICENSE_PUBKEY",
        &STANDARD.encode(signing_key.verifying_key().to_bytes()),
    );

    let tmp = tempfile::tempdir().unwrap();
    let this_device = device::device_id_in(Some(tmp.path()));

    let payload = token::TokenPayload {
        v: 1,
        key_id: "key-e2e".into(),
        plan: "pro".into(),
        device: this_device.clone(),
        issued_at: Utc::now() - Duration::hours(1),
        expires_at: None,
    };
    let (url, server_handle) = spawn_one_shot_server(200, signed_success_body(&signing_key, &payload));
    let _api_env = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url);

    let status = license::activate_in(Some(tmp.path()), "TEST-KEY")
        .await
        .expect("activate against the mock server should succeed");
    join_mock_server(server_handle);
    assert_eq!(status, LicenseStatus::Pro { expires_at: None });

    // The record is genuinely persisted to disk.
    let license_path = tmp.path().join("license.json");
    let on_disk: LicenseRecord =
        serde_json::from_str(&std::fs::read_to_string(&license_path).unwrap()).unwrap();
    assert_eq!(on_disk.key, "TEST-KEY");
    assert!(on_disk.token.contains('.'), "persisted token should be the dot-separated wire format");

    // A LATER status() call (no network — verifies signature from disk)
    // still returns Pro.
    assert_eq!(
        license::status_in(Some(tmp.path())),
        LicenseStatus::Pro { expires_at: None }
    );

    // THE tamper test. Rewrite the persisted token's payload — plan, device,
    // AND expires_at — WITHOUT re-signing, and confirm status() fails closed.
    let tampered_token = token::tamper_payload(&on_disk.token, |v| {
        v["plan"] = serde_json::Value::String("pro".into());
        v["device"] = serde_json::Value::String(this_device.clone());
        v["expires_at"] = serde_json::Value::Null;
    });
    assert_ne!(
        tampered_token, on_disk.token,
        "sanity: the tamper helper must actually change the payload bytes"
    );
    let tampered_record = LicenseRecord {
        token: tampered_token,
        ..on_disk.clone()
    };
    std::fs::write(
        &license_path,
        serde_json::to_string_pretty(&tampered_record).unwrap(),
    )
    .unwrap();

    assert_eq!(
        license::status_in(Some(tmp.path())),
        LicenseStatus::Free,
        "a hand-edited token payload with a stale signature must fail closed, never grant Pro"
    );
}

// ---------------------------------------------------------------------
// Error mapping.
// ---------------------------------------------------------------------

#[tokio::test]
async fn server_error_code_is_surfaced_verbatim() {
    let (url, handle) = spawn_one_shot_server(
        400,
        serde_json::json!({"error": "invalid_key", "message": "the key is not recognized"})
            .to_string(),
    );

    let result = request_token(&url, "BAD-KEY", "device-x").await;
    join_mock_server(handle);

    assert_eq!(
        result,
        Err(ActivationError::Server {
            code: "invalid_key".into(),
            message: "the key is not recognized".into(),
        })
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn transport_failure_is_a_network_error_and_nothing_is_persisted() {
    let _lock = license::LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    // Port 1 is privileged and unbound in a sandboxed test environment — the
    // connection is refused, exercising a genuine transport failure with no
    // server involved at all.
    let _api_env = EnvVarGuard::set("USAGECHECK_LICENSE_API", "http://127.0.0.1:1/");

    let tmp = tempfile::tempdir().unwrap();
    let result = license::activate_in(Some(tmp.path()), "ANY-KEY").await;

    assert!(
        matches!(result, Err(ActivationError::Network(_))),
        "expected a Network error, got: {result:?}"
    );
    assert!(
        !tmp.path().join("license.json").exists(),
        "a failed activation must never persist anything"
    );
}

#[tokio::test]
async fn malformed_success_body_is_an_invalid_token_error() {
    let (url, handle) = spawn_one_shot_server(200, "{\"not_a_token_field\": true}".to_string());
    let result = request_token(&url, "KEY", "device-x").await;
    join_mock_server(handle);
    assert!(matches!(result, Err(ActivationError::InvalidToken(_))));
}

// ---------------------------------------------------------------------
// The PUBLIC zero-arg wrappers (`activate`/`refresh`/`deactivate`/
// `device_id`/`status`/`is_pro`) — exercised end to end via
// `USAGECHECK_APP_DATA_DIR` (`paths.rs`'s debug-only test seam), not just
// their injectable `*_in` cores, so this is genuine coverage of what
// `main.rs`/Stage C actually call.
// ---------------------------------------------------------------------

struct AppDataDirGuard(Option<OsString>);

impl AppDataDirGuard {
    fn set(path: &std::path::Path) -> Self {
        let previous = std::env::var_os("USAGECHECK_APP_DATA_DIR");
        std::env::set_var("USAGECHECK_APP_DATA_DIR", path);
        Self(previous)
    }
}

impl Drop for AppDataDirGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(previous) => std::env::set_var("USAGECHECK_APP_DATA_DIR", previous),
            None => std::env::remove_var("USAGECHECK_APP_DATA_DIR"),
        }
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn public_activate_refresh_deactivate_wrappers_work_end_to_end() {
    let _lock = license::LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let tmp = tempfile::tempdir().unwrap();
    let _app_data_env = AppDataDirGuard::set(tmp.path());

    let signing_key = test_signing_key();
    let _pubkey_env = EnvVarGuard::set(
        "USAGECHECK_LICENSE_PUBKEY",
        &STANDARD.encode(signing_key.verifying_key().to_bytes()),
    );

    // device_id() is stable and derived from the (now-overridden) app data
    // dir, same as the injectable core.
    let this_device = license::device_id();
    assert_eq!(this_device.len(), 64);
    assert_eq!(this_device, license::device_id());

    assert!(!license::is_pro(), "no license activated yet");

    let payload = token::TokenPayload {
        v: 1,
        key_id: "public-wrapper-test".into(),
        plan: "pro".into(),
        device: this_device,
        issued_at: Utc::now() - Duration::hours(1),
        expires_at: None,
    };
    let (url, handle) = spawn_one_shot_server(200, signed_success_body(&signing_key, &payload));
    let _api_env = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url);

    let status = license::activate("TEST-KEY").await.expect("public activate() should succeed");
    join_mock_server(handle);
    assert_eq!(status, LicenseStatus::Pro { expires_at: None });
    assert!(license::is_pro());
    assert_eq!(license::status(), LicenseStatus::Pro { expires_at: None });

    // refresh() re-runs activation with the STORED key — no `USAGECHECK_LICENSE_API`
    // request body is needed here since the mock server doesn't inspect it,
    // but a second one-shot server is required (the first was consumed).
    // H1: the refresh response's `issued_at` must strictly ADVANCE past the
    // stored token's own `issued_at`, or the new replay guard rejects it —
    // reusing `payload` unchanged here would now fail with `ReplayedToken`.
    let payload2 = token::TokenPayload {
        issued_at: payload.issued_at + Duration::minutes(1),
        ..payload.clone()
    };
    let (url2, handle2) = spawn_one_shot_server(200, signed_success_body(&signing_key, &payload2));
    let _api_env2 = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url2);
    let refreshed = license::refresh().await.expect("public refresh() should succeed");
    join_mock_server(handle2);
    assert_eq!(refreshed, LicenseStatus::Pro { expires_at: None });

    license::deactivate().await.expect("deactivate should succeed");
    assert!(!license::is_pro(), "deactivate must remove the license");
    assert_eq!(license::status(), LicenseStatus::Free);

    // deactivate() is idempotent when nothing is stored.
    license::deactivate()
        .await
        .expect("deactivate on an already-empty record is a no-op");
}

// item 3 fix: `deactivate_in` never depended on the record parsing in the
// first place (it removes the file by path, never reading its contents),
// but this pins that behavior explicitly now that `has_stored_license`
// advertises a malformed record as removable too.
#[tokio::test]
async fn deactivate_removes_a_malformed_license_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("license.json"), "not json").unwrap();
    license::deactivate_in(Some(tmp.path()))
        .await
        .expect("deactivate must remove a malformed record too, not just a well-formed one");
    assert!(!tmp.path().join("license.json").exists());
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn public_refresh_with_no_stored_license_is_no_stored_license_error() {
    let _lock = license::LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let _app_data_env = AppDataDirGuard::set(tmp.path());

    assert_eq!(license::refresh().await, Err(ActivationError::NoStoredLicense));
}

// ---------------------------------------------------------------------
// H1 replay guard + H3 validate-before-persist: `activate_or_refresh_in`
// (exercised through the private `activate_in`/`refresh_in` cores, same as
// the tests above).
// ---------------------------------------------------------------------

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn refresh_rejects_a_non_advancing_issued_at_as_replayed_and_leaves_the_record_untouched() {
    let _lock = license::LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _pubkey_env = EnvVarGuard::set(
        "USAGECHECK_LICENSE_PUBKEY",
        &STANDARD.encode(signing_key.verifying_key().to_bytes()),
    );

    let tmp = tempfile::tempdir().unwrap();
    let this_device = device::device_id_in(Some(tmp.path()));

    let payload = token::TokenPayload {
        v: 1,
        key_id: "replay-test".into(),
        plan: "pro".into(),
        device: this_device,
        issued_at: Utc::now() - Duration::hours(1),
        expires_at: None,
    };
    let (url, handle) = spawn_one_shot_server(200, signed_success_body(&signing_key, &payload));
    let _api_env = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url);

    let status = license::activate_in(Some(tmp.path()), "TEST-KEY")
        .await
        .expect("initial activation should succeed");
    join_mock_server(handle);
    assert_eq!(status, LicenseStatus::Pro { expires_at: None });

    let before: LicenseRecord =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join("license.json")).unwrap()).unwrap();
    let before_files = snapshot_license_files(tmp.path());

    // A "refresh" server response reusing the SAME issued_at (a replay, or a
    // caching/proxy bug serving a stale response) must be rejected outright.
    let (url2, handle2) = spawn_one_shot_server(200, signed_success_body(&signing_key, &payload));
    let _api_env2 = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url2);
    let result = license::refresh_in(Some(tmp.path())).await;
    join_mock_server(handle2);
    assert_eq!(result, Err(ActivationError::ReplayedToken));

    let after: LicenseRecord =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join("license.json")).unwrap()).unwrap();
    assert_eq!(
        before.token, after.token,
        "a replayed refresh response must leave the stored record completely untouched"
    );
    // (A) Neither file may be mutated for a rejected candidate — the replay
    // guard runs before the watermark is ever touched.
    assert_eq!(
        snapshot_license_files(tmp.path()),
        before_files,
        "a replayed refresh response must leave BOTH license.json AND clock-watermark byte-identical"
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn refresh_with_an_expired_candidate_is_not_entitled_and_leaves_the_record_intact() {
    let _lock = license::LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _pubkey_env = EnvVarGuard::set(
        "USAGECHECK_LICENSE_PUBKEY",
        &STANDARD.encode(signing_key.verifying_key().to_bytes()),
    );

    let tmp = tempfile::tempdir().unwrap();
    let this_device = device::device_id_in(Some(tmp.path()));

    let good_payload = token::TokenPayload {
        v: 1,
        key_id: "not-entitled-test".into(),
        plan: "pro".into(),
        device: this_device,
        issued_at: Utc::now() - Duration::hours(1),
        expires_at: None,
    };
    let (url, handle) = spawn_one_shot_server(200, signed_success_body(&signing_key, &good_payload));
    let _api_env = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url);
    let status = license::activate_in(Some(tmp.path()), "TEST-KEY")
        .await
        .expect("initial activation should succeed");
    join_mock_server(handle);
    assert_eq!(status, LicenseStatus::Pro { expires_at: None });

    let before: LicenseRecord =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join("license.json")).unwrap()).unwrap();
    let before_files = snapshot_license_files(tmp.path());

    // A newer, VALIDLY-SIGNED, but ALREADY-EXPIRED candidate (H3: full
    // decide_status evaluation runs BEFORE persisting) must not overwrite
    // the previously-good record.
    let expired_payload = token::TokenPayload {
        issued_at: good_payload.issued_at + Duration::minutes(1),
        expires_at: Some(Utc::now() - Duration::minutes(1)),
        ..good_payload.clone()
    };
    let (url2, handle2) = spawn_one_shot_server(200, signed_success_body(&signing_key, &expired_payload));
    let _api_env2 = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url2);
    let result = license::refresh_in(Some(tmp.path())).await;
    join_mock_server(handle2);
    assert_eq!(result, Err(ActivationError::NotEntitled(LicenseStatus::Expired)));

    let after: LicenseRecord =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join("license.json")).unwrap()).unwrap();
    assert_eq!(
        before.token, after.token,
        "a NotEntitled candidate must leave the previously-stored good record untouched"
    );
    // (A) THE regression test: before the fix, the watermark was repaired
    // UNCONDITIONALLY before the candidate's full temporal validity was
    // evaluated, so an expired candidate still advanced `clock-watermark`
    // even though `license.json` was correctly left alone. Both files must
    // now be byte-identical to before the call.
    assert_eq!(
        snapshot_license_files(tmp.path()),
        before_files,
        "a NotEntitled candidate must leave BOTH license.json AND clock-watermark byte-identical"
    );
}

/// (A) Same regression as above, for a FUTURE-issued candidate: a validly
/// signed token whose `issued_at` is ahead of `now` (`decide_status` rejects
/// this as `Free`, one of the EARLIEST checks, before the watermark/rollback
/// branch is even reached) must still leave both files completely untouched.
#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn refresh_with_a_future_issued_candidate_is_not_entitled_and_leaves_both_files_intact() {
    let _lock = license::LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _pubkey_env = EnvVarGuard::set(
        "USAGECHECK_LICENSE_PUBKEY",
        &STANDARD.encode(signing_key.verifying_key().to_bytes()),
    );

    let tmp = tempfile::tempdir().unwrap();
    let this_device = device::device_id_in(Some(tmp.path()));

    let good_payload = token::TokenPayload {
        v: 1,
        key_id: "future-issued-test".into(),
        plan: "pro".into(),
        device: this_device,
        issued_at: Utc::now() - Duration::hours(1),
        expires_at: None,
    };
    let (url, handle) = spawn_one_shot_server(200, signed_success_body(&signing_key, &good_payload));
    let _api_env = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url);
    let status = license::activate_in(Some(tmp.path()), "TEST-KEY")
        .await
        .expect("initial activation should succeed");
    join_mock_server(handle);
    assert_eq!(status, LicenseStatus::Pro { expires_at: None });

    let before: LicenseRecord =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join("license.json")).unwrap()).unwrap();
    let before_files = snapshot_license_files(tmp.path());

    // A newer, validly-signed candidate whose issued_at is in the FUTURE —
    // an untrustworthy record regardless of anything else about it.
    let future_payload = token::TokenPayload {
        issued_at: Utc::now() + Duration::hours(1),
        ..good_payload.clone()
    };
    let (url2, handle2) = spawn_one_shot_server(200, signed_success_body(&signing_key, &future_payload));
    let _api_env2 = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url2);
    let result = license::refresh_in(Some(tmp.path())).await;
    join_mock_server(handle2);
    assert_eq!(result, Err(ActivationError::NotEntitled(LicenseStatus::Free)));

    let after: LicenseRecord =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join("license.json")).unwrap()).unwrap();
    assert_eq!(before.token, after.token);
    assert_eq!(
        snapshot_license_files(tmp.path()),
        before_files,
        "a future-issued candidate must leave BOTH license.json AND clock-watermark byte-identical"
    );
}

// ---------------------------------------------------------------------
// F3: a genuine online verification must be able to RECOVER from a
// far-future or deleted watermark — server-authenticated time (the
// candidate's own signed `issued_at`) repairs the watermark unconditionally.
// ---------------------------------------------------------------------

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn refresh_recovers_from_a_far_future_watermark_and_returns_pro() {
    let _lock = license::LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _pubkey_env = EnvVarGuard::set(
        "USAGECHECK_LICENSE_PUBKEY",
        &STANDARD.encode(signing_key.verifying_key().to_bytes()),
    );

    let tmp = tempfile::tempdir().unwrap();
    let this_device = device::device_id_in(Some(tmp.path()));

    let payload = token::TokenPayload {
        v: 1,
        key_id: "f3-far-future".into(),
        plan: "pro".into(),
        device: this_device,
        issued_at: Utc::now() - Duration::hours(1),
        expires_at: None,
    };
    let (url, handle) = spawn_one_shot_server(200, signed_success_body(&signing_key, &payload));
    let _api_env = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url);
    let status = license::activate_in(Some(tmp.path()), "TEST-KEY")
        .await
        .expect("initial activation should succeed");
    join_mock_server(handle);
    assert_eq!(status, LicenseStatus::Pro { expires_at: None });

    // Corrupt the watermark to a bogus far-future value, as if a rollback
    // had previously been (mis-)detected or the file was hand-edited.
    let bogus = watermark::WatermarkRecord {
        max_seen: Utc::now() + Duration::days(400),
        source_issued_at: Utc::now() + Duration::days(400),
    };
    std::fs::write(
        tmp.path().join("clock-watermark"),
        serde_json::to_string(&bogus).unwrap(),
    )
    .unwrap();
    assert_eq!(
        license::status_in(Some(tmp.path())),
        LicenseStatus::GracePeriodEnded,
        "sanity: the far-future watermark must actually look broken before repair"
    );

    // A genuine, freshly-signed refresh response repairs it unconditionally.
    let payload2 = token::TokenPayload {
        issued_at: payload.issued_at + Duration::minutes(1),
        ..payload.clone()
    };
    let (url2, handle2) = spawn_one_shot_server(200, signed_success_body(&signing_key, &payload2));
    let _api_env2 = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url2);
    let refreshed = license::refresh_in(Some(tmp.path())).await;
    join_mock_server(handle2);
    assert_eq!(refreshed, Ok(LicenseStatus::Pro { expires_at: None }));

    // And the repair is durable — a later, purely-offline status() read
    // (no network) also sees Pro, not just the refresh call's own return.
    assert_eq!(
        license::status_in(Some(tmp.path())),
        LicenseStatus::Pro { expires_at: None }
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn activation_recovers_from_a_deleted_watermark_and_returns_pro() {
    let _lock = license::LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _pubkey_env = EnvVarGuard::set(
        "USAGECHECK_LICENSE_PUBKEY",
        &STANDARD.encode(signing_key.verifying_key().to_bytes()),
    );

    let tmp = tempfile::tempdir().unwrap();
    let this_device = device::device_id_in(Some(tmp.path()));
    // No watermark file exists at all yet (cold start / deleted).
    assert!(!tmp.path().join("clock-watermark").exists());

    let payload = token::TokenPayload {
        v: 1,
        key_id: "f3-deleted".into(),
        plan: "pro".into(),
        device: this_device,
        issued_at: Utc::now() - Duration::hours(1),
        expires_at: None,
    };
    let (url, handle) = spawn_one_shot_server(200, signed_success_body(&signing_key, &payload));
    let _api_env = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url);

    let status = license::activate_in(Some(tmp.path()), "TEST-KEY").await;
    join_mock_server(handle);
    assert_eq!(status, Ok(LicenseStatus::Pro { expires_at: None }));
    assert!(
        tmp.path().join("clock-watermark").exists(),
        "a successful activation must repair (create) the watermark"
    );
    assert_eq!(
        license::status_in(Some(tmp.path())),
        LicenseStatus::Pro { expires_at: None }
    );
}

// ---------------------------------------------------------------------
// item 1 fix: `activate_or_refresh_in` writes the watermark FIRST, then the
// record — and a watermark-write failure must propagate as an error rather
// than being silently swallowed, with the record write never attempted.
// ---------------------------------------------------------------------

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn activation_propagates_a_watermark_write_failure_and_never_publishes_a_record() {
    let _lock = license::LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _pubkey_env = EnvVarGuard::set(
        "USAGECHECK_LICENSE_PUBKEY",
        &STANDARD.encode(signing_key.verifying_key().to_bytes()),
    );

    let tmp = tempfile::tempdir().unwrap();
    let this_device = device::device_id_in(Some(tmp.path()));

    // Fault injection: force the WATERMARK write specifically to fail.
    // `write_private_file` stages into a temp sibling and then
    // `fs::rename`s it onto the final target path — renaming a regular
    // file onto an EXISTING DIRECTORY always fails (ENOTDIR/EISDIR on Unix;
    // the equivalent on Windows), regardless of whether that directory is
    // empty. Pre-creating a real directory at the watermark's exact path
    // therefore makes ONLY the watermark write fail: the license record
    // write targets a different filename (`license.json`) and would
    // succeed if it were ever attempted.
    std::fs::create_dir(tmp.path().join("clock-watermark")).unwrap();

    let payload = token::TokenPayload {
        v: 1,
        key_id: "watermark-write-failure".into(),
        plan: "pro".into(),
        device: this_device,
        issued_at: Utc::now() - Duration::hours(1),
        expires_at: None,
    };
    let (url, handle) = spawn_one_shot_server(200, signed_success_body(&signing_key, &payload));
    let _api_env = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url);

    let result = license::activate_in(Some(tmp.path()), "TEST-KEY").await;
    join_mock_server(handle);

    // (a) the call returns an error.
    assert!(
        matches!(result, Err(ActivationError::Persist(_))),
        "a watermark-write failure must surface as ActivationError::Persist, got: {result:?}"
    );

    // (b) NO license record was published — with the watermark written
    // FIRST, the record write is never even attempted once the watermark
    // write has already failed.
    assert!(
        !tmp.path().join("license.json").exists(),
        "activation must never publish a record when the watermark write that precedes it failed"
    );

    // (c) a subsequent status_in() is non-Pro.
    assert!(
        !matches!(license::status_in(Some(tmp.path())), LicenseStatus::Pro { .. }),
        "with no record on disk, status must never read as Pro"
    );
}

// ---------------------------------------------------------------------
// F4: the replay guard applies uniformly to `activate`, not just `refresh`.
// ---------------------------------------------------------------------

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn activate_rejects_a_token_identical_to_a_verifiable_stored_one_as_replayed() {
    let _lock = license::LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _pubkey_env = EnvVarGuard::set(
        "USAGECHECK_LICENSE_PUBKEY",
        &STANDARD.encode(signing_key.verifying_key().to_bytes()),
    );

    let tmp = tempfile::tempdir().unwrap();
    let this_device = device::device_id_in(Some(tmp.path()));
    let payload = token::TokenPayload {
        v: 1,
        key_id: "f4-activate-replay".into(),
        plan: "pro".into(),
        device: this_device,
        issued_at: Utc::now() - Duration::hours(1),
        expires_at: None,
    };
    let (url, handle) = spawn_one_shot_server(200, signed_success_body(&signing_key, &payload));
    let _api_env = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url);
    let status = license::activate_in(Some(tmp.path()), "TEST-KEY")
        .await
        .expect("initial activation should succeed");
    join_mock_server(handle);
    assert_eq!(status, LicenseStatus::Pro { expires_at: None });

    let before: LicenseRecord =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join("license.json")).unwrap()).unwrap();

    // A second `activate` call whose server response reuses the SAME
    // issued_at as the currently-stored, still-verifiable token.
    let (url2, handle2) = spawn_one_shot_server(200, signed_success_body(&signing_key, &payload));
    let _api_env2 = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url2);
    let result = license::activate_in(Some(tmp.path()), "TEST-KEY").await;
    join_mock_server(handle2);
    assert_eq!(result, Err(ActivationError::ReplayedToken));

    let after: LicenseRecord =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join("license.json")).unwrap()).unwrap();
    assert_eq!(
        before.token, after.token,
        "a replayed activate response must leave the stored record completely untouched"
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn activate_with_a_corrupted_stored_token_accepts_a_valid_fresh_candidate() {
    let _lock = license::LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _pubkey_env = EnvVarGuard::set(
        "USAGECHECK_LICENSE_PUBKEY",
        &STANDARD.encode(signing_key.verifying_key().to_bytes()),
    );

    let tmp = tempfile::tempdir().unwrap();
    let this_device = device::device_id_in(Some(tmp.path()));

    // A stored record whose token is NOT a token this build can verify at
    // all (the exact forgery shape H3/Stage B exists to defeat) — there is
    // no trustworthy prior `issued_at` to compare against.
    let corrupted = LicenseRecord {
        token: "not-a-real-token".into(),
        key: "OLD-KEY".into(),
        verified_at: Utc::now(),
    };
    std::fs::write(
        tmp.path().join("license.json"),
        serde_json::to_string_pretty(&corrupted).unwrap(),
    )
    .unwrap();

    let payload = token::TokenPayload {
        v: 1,
        key_id: "f4-corrupted-prior".into(),
        plan: "pro".into(),
        device: this_device,
        issued_at: Utc::now() - Duration::hours(1),
        expires_at: None,
    };
    let (url, handle) = spawn_one_shot_server(200, signed_success_body(&signing_key, &payload));
    let _api_env = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url);
    let result = license::activate_in(Some(tmp.path()), "TEST-KEY").await;
    join_mock_server(handle);
    assert_eq!(
        result,
        Ok(LicenseStatus::Pro { expires_at: None }),
        "an unverifiable stored token has no trustworthy prior issued_at, so the guard must be skipped"
    );

    let after: LicenseRecord =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join("license.json")).unwrap()).unwrap();
    assert_ne!(
        after.token, corrupted.token,
        "the record must be REPLACED once the fresh candidate resolves to Pro"
    );
}

// ---------------------------------------------------------------------
// F5: never bind a license to a non-durable device id.
// ---------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn activation_refuses_before_any_network_or_write_when_device_id_cannot_be_persisted() {
    use std::os::unix::fs::symlink;

    // Mutates the shared `USAGECHECK_LICENSE_API` env var below (even though
    // `activate_in` never reads it here — it fails closed on the
    // unpersistable device id first) — still needs the crate-wide lock, or
    // the set/restore below can race a concurrent test's own value.
    let _lock = license::LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let tmp = tempfile::tempdir().unwrap();
    let device_id_path = tmp.path().join("device-id");
    // Force device-id persistence to fail: the path itself is a symlink, so
    // `reject_symlink` refuses every write attempt.
    symlink("/nonexistent-target", &device_id_path).unwrap();
    let first = device::device_id_in(Some(tmp.path()));

    // Deliberately point at an unroutable address rather than a real mock
    // server — if `activate_in` ever DID attempt a network call here, this
    // test would hang/fail on a connection attempt instead of silently
    // passing, making a regression to "contacts the server anyway" loud.
    let _api_env = EnvVarGuard::set("USAGECHECK_LICENSE_API", "http://127.0.0.1:1/");

    let result = license::activate_in(Some(tmp.path()), "TEST-KEY").await;
    assert_eq!(result, Err(ActivationError::DeviceNotPersisted));
    assert!(
        !tmp.path().join("license.json").exists(),
        "nothing may be written when the device id itself is not durable"
    );
    assert!(
        !tmp.path().join("clock-watermark").exists(),
        "the watermark must not be repaired either when activation refuses this early"
    );

    // A later successful persistence (the symlink is cleared, mimicking the
    // underlying condition resolving) yields the SAME id that was already
    // cached and handed to the refused activation attempt — never a fresh
    // one, and never one that would silently orphan a subsequent real
    // activation from the one this refusal reported.
    std::fs::remove_file(&device_id_path).unwrap();
    let second = device::device_id_in(Some(tmp.path()));
    assert_eq!(first, second, "recovery must yield the same id, not a freshly minted one");
}

// (C) The in-process cache must not be trusted blindly once `persisted` was
// observed `true` — the durable state can be deleted/replaced out from under
// the process after a successful activation, and `refresh` must re-verify it
// BEFORE contacting the server, not just once at the very first call.
#[cfg(unix)]
#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn refresh_re_establishes_device_durability_before_any_network_contact() {
    use std::os::unix::fs::symlink;

    let _lock = license::LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _pubkey_env = EnvVarGuard::set(
        "USAGECHECK_LICENSE_PUBKEY",
        &STANDARD.encode(signing_key.verifying_key().to_bytes()),
    );

    let tmp = tempfile::tempdir().unwrap();
    let device_id_path = tmp.path().join("device-id");
    let this_device = device::device_id_in(Some(tmp.path()));
    assert!(device_id_path.exists(), "sanity: device id should be persisted after minting");

    let payload = token::TokenPayload {
        v: 1,
        key_id: "device-durability-test".into(),
        plan: "pro".into(),
        device: this_device,
        issued_at: Utc::now() - Duration::hours(1),
        expires_at: None,
    };
    let (url, handle) = spawn_one_shot_server(200, signed_success_body(&signing_key, &payload));
    let _api_env = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url);
    let status = license::activate_in(Some(tmp.path()), "TEST-KEY")
        .await
        .expect("initial activation should succeed");
    join_mock_server(handle);
    assert_eq!(status, LicenseStatus::Pro { expires_at: None });

    let before: LicenseRecord =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join("license.json")).unwrap()).unwrap();

    // Simulate the device-id file being lost after activation (deleted, or a
    // dropped network volume) AND being unable to re-persist it (path
    // replaced with a symlink `reject_symlink` always refuses) — the
    // deterministic FAILURE shape: durability cannot be re-established.
    std::fs::remove_file(&device_id_path).unwrap();
    symlink("/nonexistent-target", &device_id_path).unwrap();

    // Point at an unroutable address: if `refresh_in` ever DID attempt a
    // network call despite the lost durability, this would surface as a
    // `Network` error (connection refused on a privileged, unbound port) —
    // clearly distinguishable from `DeviceNotPersisted` — making a
    // regression to "contacts the server anyway" loud and immediate rather
    // than silently passing.
    let _api_env2 = EnvVarGuard::set("USAGECHECK_LICENSE_API", "http://127.0.0.1:1/");

    let result = license::refresh_in(Some(tmp.path())).await;
    assert_eq!(
        result,
        Err(ActivationError::DeviceNotPersisted),
        "a refresh must re-verify durable device state and refuse BEFORE any network contact \
         when it cannot be re-established — not silently proceed, and not surface as a Network error"
    );

    let after: LicenseRecord =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join("license.json")).unwrap()).unwrap();
    assert_eq!(
        before.token, after.token,
        "a refused refresh must leave the previously-stored good record untouched"
    );
}

// ---------------------------------------------------------------------
// D: two concurrent activate/refresh calls must never lose the newer
// committed token to an older one racing in behind it.
// ---------------------------------------------------------------------

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn concurrent_refresh_calls_never_lose_the_newer_committed_token() {
    let _lock = license::LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _pubkey_env = EnvVarGuard::set(
        "USAGECHECK_LICENSE_PUBKEY",
        &STANDARD.encode(signing_key.verifying_key().to_bytes()),
    );

    let tmp = tempfile::tempdir().unwrap();
    let this_device = device::device_id_in(Some(tmp.path()));

    let baseline_payload = token::TokenPayload {
        v: 1,
        key_id: "concurrency-test".into(),
        plan: "pro".into(),
        device: this_device,
        issued_at: Utc::now() - Duration::hours(2),
        expires_at: None,
    };
    let (url, handle) = spawn_one_shot_server(200, signed_success_body(&signing_key, &baseline_payload));
    let _api_env = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url);
    let status = license::activate_in(Some(tmp.path()), "TEST-KEY")
        .await
        .expect("baseline activation should succeed");
    join_mock_server(handle);
    assert_eq!(status, LicenseStatus::Pro { expires_at: None });

    // Two responses with STRICTLY increasing `issued_at`: whichever HTTP
    // request reaches the server FIRST gets the older one, whichever
    // reaches it SECOND gets the newer one.
    //
    // What this test actually proves, and why the previous version didn't:
    // serving older-then-newer purely in ARRIVAL order, with the server
    // processing requests in a plain sequential accept loop, would ALSO
    // pass with `activate_lock` deleted from `activate_or_refresh_in` —
    // nothing in that version forced the two requests to be non-overlapping
    // (a sequential accept loop can't even observe an overlap: it can't ask
    // for request #2 until it's done handling #1, regardless of whether the
    // CLIENT actually sent #2 concurrently), so "whichever request happens
    // to arrive first gets the older payload" held regardless of whether
    // the two calls were actually serialized. The property that IS specific
    // to the lock is that the second call cannot even SEND its HTTP request
    // until the first call has entirely finished (network round trip AND
    // both file writes) and released the lock. `spawn_two_shot_server` now
    // makes that observable directly: it holds the first request open and
    // polls tiny_http's non-blocking `try_recv()` (fed by tiny_http's own
    // background listener, independent of this thread's control flow) for
    // up to `first_response_delay`, reporting whether the second request
    // reached the server DURING that window — i.e. a genuine overlap. With
    // the lock held, `refresh_in` #2 cannot send its request until strictly
    // after `refresh_in` #1 has released the lock, which is after the
    // server has already been sent request #1's response — so no overlap
    // should ever be observed. Without the lock, `tokio::join!` runs both
    // `refresh_in` futures concurrently: call #2 reaches the point of
    // sending its own HTTP request while call #1 is still parked awaiting
    // its (deliberately delayed) response, so the second request lands
    // inside that window and `overlap_detected` comes back `true` — which
    // is exactly the assertion below.
    //
    // The final-committed-token assertion at the end is the same lost-update
    // guard the original version of this test carried: the call that runs
    // SECOND always re-reads the FIRST call's own fresh commit as `previous`
    // and must strictly advance past it, so the final stored token is always
    // the newer one, never clobbered by an older one racing in behind it.
    let earlier_payload = token::TokenPayload {
        issued_at: baseline_payload.issued_at + Duration::minutes(1),
        ..baseline_payload.clone()
    };
    let later_payload = token::TokenPayload {
        issued_at: baseline_payload.issued_at + Duration::minutes(2),
        ..baseline_payload.clone()
    };
    let first_response_delay = std::time::Duration::from_millis(200);
    let (url2, handle2) = spawn_two_shot_server(
        [
            signed_success_body(&signing_key, &earlier_payload),
            signed_success_body(&signing_key, &later_payload),
        ],
        first_response_delay,
    );
    let _api_env2 = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url2);

    let (r1, r2) = tokio::join!(
        license::refresh_in(Some(tmp.path())),
        license::refresh_in(Some(tmp.path())),
    );
    let overlap_detected = join_mock_server(handle2);

    assert_eq!(r1, Ok(LicenseStatus::Pro { expires_at: None }), "first-resolving refresh: {r1:?}");
    assert_eq!(r2, Ok(LicenseStatus::Pro { expires_at: None }), "second-resolving refresh: {r2:?}");

    // THE property that actually depends on `activate_lock`: the two
    // requests' windows must never overlap. Removing the lock makes this
    // assertion fail (verified manually — see the item-3 fix notes).
    assert!(
        !overlap_detected,
        "the two refresh requests overlapped — the second request reached the server while the \
         first was still being held open — this means the two `refresh_in` calls ran \
         concurrently, which activate_lock must prevent"
    );

    let final_record: LicenseRecord =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join("license.json")).unwrap()).unwrap();
    let final_payload = token::verify_token(&final_record.token, &signing_key.verifying_key())
        .expect("final stored token must still verify");
    assert_eq!(
        final_payload.issued_at, later_payload.issued_at,
        "the final stored token must be the NEWEST one committed, never an older one clobbering it"
    );
}

// ---------------------------------------------------------------------
// item 1 fix: `deactivate` must be serialized through the SAME
// `activate_lock` as activate/refresh — otherwise a concurrent
// activation/refresh already in flight can commit its watermark+record
// writes AFTER `deactivate` has already removed `license.json`, silently
// RESURRECTING the license the user just asked to remove even though
// `deactivate` itself reported success.
// ---------------------------------------------------------------------

#[tokio::test]
#[allow(clippy::await_holding_lock)] // Process-wide env mutation must remain serialized.
async fn deactivate_never_loses_to_a_refresh_that_commits_after_it_starts() {
    let _lock = license::LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let signing_key = test_signing_key();
    let _pubkey_env = EnvVarGuard::set(
        "USAGECHECK_LICENSE_PUBKEY",
        &STANDARD.encode(signing_key.verifying_key().to_bytes()),
    );

    let tmp = tempfile::tempdir().unwrap();
    let this_device = device::device_id_in(Some(tmp.path()));

    let baseline_payload = token::TokenPayload {
        v: 1,
        key_id: "deactivate-race-test".into(),
        plan: "pro".into(),
        device: this_device,
        issued_at: Utc::now() - Duration::hours(1),
        expires_at: None,
    };
    let (url, handle) = spawn_one_shot_server(200, signed_success_body(&signing_key, &baseline_payload));
    let _api_env = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url);
    let status = license::activate_in(Some(tmp.path()), "TEST-KEY")
        .await
        .expect("baseline activation should succeed");
    join_mock_server(handle);
    assert_eq!(status, LicenseStatus::Pro { expires_at: None });
    assert!(
        tmp.path().join("license.json").exists(),
        "sanity: baseline record persisted"
    );

    // A refresh whose response the mock server deliberately DELAYS, but
    // which notifies `request_received` the instant the HTTP request has
    // actually reached it — i.e. the instant `refresh_in` is genuinely in
    // flight, with `activate_lock` already held by it.
    let refreshed_payload = token::TokenPayload {
        issued_at: baseline_payload.issued_at + Duration::minutes(5),
        ..baseline_payload.clone()
    };
    let response_delay = std::time::Duration::from_millis(200);
    let (url2, request_received, handle2) = spawn_delayed_one_shot_server(
        200,
        signed_success_body(&signing_key, &refreshed_payload),
        response_delay,
    );
    let _api_env2 = EnvVarGuard::set("USAGECHECK_LICENSE_API", &url2);

    let refresh_future = license::refresh_in(Some(tmp.path()));
    let deactivate_future = async {
        // Don't call deactivate until refresh's HTTP request has genuinely
        // reached the server — proving `refresh_in` is in flight and
        // already holding `activate_lock` — rather than racing to see
        // which of the two happens to start first.
        request_received.notified().await;
        license::deactivate_in(Some(tmp.path())).await
    };
    let (refresh_result, deactivate_result) = tokio::join!(refresh_future, deactivate_future);
    join_mock_server(handle2);

    refresh_result.expect("refresh should still succeed despite the concurrent deactivate");
    deactivate_result.expect("deactivate should succeed");

    // WITHOUT `deactivate_in` taking `activate_lock`: once notified, the
    // deactivate future would run immediately — well before the mock
    // server's deliberately delayed response arrives — and delete
    // `license.json` right away. `refresh_in`'s already-in-flight request
    // would then receive its response ~200ms later, verify it, and commit
    // its watermark+record writes — RECREATING `license.json` and silently
    // resurrecting the license the user just deactivated, even though
    // `deactivate` itself already reported success. Serializing deactivate
    // through the same `activate_lock` forces it to wait until refresh_in's
    // commit has fully completed and released the lock, so deactivate is
    // always the LAST writer here and the file stays removed.
    assert!(
        !tmp.path().join("license.json").exists(),
        "license.json was resurrected by a refresh that committed after deactivate ran — \
         deactivate_in must be serialized through activate_lock"
    );
}

// ---------------------------------------------------------------------
// H4: never log server-provided text. `classify()` must strip all text from
// an `ActivationError`, including a `Server{code, message}` variant whose
// content is attacker-controlled (the server can choose which error VARIANT
// is produced, but never what text a caller logging the classification
// would emit).
// ---------------------------------------------------------------------

#[test]
fn activation_error_classification_never_contains_server_provided_text() {
    let err = ActivationError::Server {
        code: "SECRET-LEAK-CODE".into(),
        message: "attacker controlled message containing SECRET-LEAK-CODE".into(),
    };
    let class_text = err.classify().to_string();
    assert_eq!(class_text, "server-rejected");
    assert!(!class_text.contains("SECRET-LEAK-CODE"));
    assert!(!class_text.contains("attacker"));
}

// ---------------------------------------------------------------------
// classify_error: pure logic, no network — covers the DEFAULT_ENDPOINT-only
// 404/405 mapping that a mock server (never bound at the real production
// URL) cannot exercise end to end.
// ---------------------------------------------------------------------

#[test]
fn classify_error_404_from_the_default_host_is_endpoint_not_implemented() {
    assert_eq!(
        classify_error(404, DEFAULT_ENDPOINT, None),
        ActivationError::EndpointNotImplemented
    );
    assert_eq!(
        classify_error(405, DEFAULT_ENDPOINT, None),
        ActivationError::EndpointNotImplemented
    );
}

#[test]
fn classify_error_404_from_a_non_default_host_is_a_generic_server_error() {
    assert_eq!(
        classify_error(404, "http://127.0.0.1:9/", None),
        ActivationError::Server {
            code: "http_error".into(),
            message: "HTTP 404".into(),
        }
    );
}

#[test]
fn classify_error_surfaces_a_structured_body_over_the_generic_fallback() {
    let body = ActivateErrorBody {
        error: "device_limit".into(),
        message: "too many devices".into(),
    };
    assert_eq!(
        classify_error(403, "http://127.0.0.1:9/", Some(body)),
        ActivationError::Server {
            code: "device_limit".into(),
            message: "too many devices".into(),
        }
    );
}

#[test]
fn resolve_endpoint_defaults_when_env_unset_or_blank() {
    let _lock = license::LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let previous = std::env::var_os("USAGECHECK_LICENSE_API");
    std::env::remove_var("USAGECHECK_LICENSE_API");
    assert_eq!(resolve_endpoint(), DEFAULT_ENDPOINT);

    std::env::set_var("USAGECHECK_LICENSE_API", "   ");
    assert_eq!(resolve_endpoint(), DEFAULT_ENDPOINT);

    match previous {
        Some(p) => std::env::set_var("USAGECHECK_LICENSE_API", p),
        None => std::env::remove_var("USAGECHECK_LICENSE_API"),
    }
}

#[test]
fn resolve_endpoint_honors_a_set_override() {
    let _lock = license::LICENSE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _guard = EnvVarGuard::set("USAGECHECK_LICENSE_API", "http://staging.example.test/verify");
    assert_eq!(resolve_endpoint(), "http://staging.example.test/verify");
}

/// Regression coverage for the worked example in `docs/LICENSE_API.md` §5:
/// the exact token/pubkey pair documented there must actually verify and
/// decode to the exact payload shown, so the doc can never silently drift
/// from what the client really accepts. If this test ever needs updating,
/// the doc's §5 values need the identical update.
#[test]
fn doc_worked_example_token_verifies_against_the_documented_pubkey() {
    let seed = [0x0bu8; 32];
    let signing_key = SigningKey::from_bytes(&seed);
    let public_key = signing_key.verifying_key();
    assert_eq!(
        STANDARD.encode(public_key.to_bytes()),
        "Zr5+Myx6RTMyvZ0Kf32wVfXF7xoGraZtmLOftoEMRzo=",
        "docs/LICENSE_API.md §5's documented public key must match this seed's derived key"
    );

    let documented_token = "eyJ2IjoxLCJrZXlfaWQiOiJ0ZXN0LWtleS1pZCIsInBsYW4iOiJwcm8iLCJkZXZpY2UiOiIwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwIiwiaXNzdWVkX2F0IjoiMjAyNi0wMS0wMVQwMDowMDowMFoiLCJleHBpcmVzX2F0IjpudWxsfQ.DDEAV_QZp5y1haEGFq-v1hoioIvhwOGdftYv_V8OWDs5fjpz9wzl0GqmASw2ckNemhou742GShFbRE3LhLCNAA";

    let payload = token::verify_token(documented_token, &public_key)
        .expect("the documented worked-example token must verify against the documented key");
    assert_eq!(payload.v, 1);
    assert_eq!(payload.key_id, "test-key-id");
    assert_eq!(payload.plan, "pro");
    assert_eq!(payload.device, "0".repeat(64));
    assert_eq!(payload.issued_at, "2026-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap());
    assert_eq!(payload.expires_at, None);

    // And the token is REPRODUCIBLE from the payload + key, matching what
    // the doc tells the server team to implement.
    let regenerated = token::encode_token(&payload, &signing_key);
    assert_eq!(regenerated, documented_token);
}
