use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::TimeZone;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use usage_core::account::{AuthSource, Provider};

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap()
}

fn jwt_with_exp(exp: i64) -> String {
    let payload = URL_SAFE_NO_PAD.encode(json!({ "exp": exp }).to_string());
    format!("aaa.{payload}.ccc")
}

fn account() -> Account {
    Account {
        id: "grok-cli-1".into(),
        provider: Provider::Grok,
        label: "user@example.com".into(),
        auth_source: AuthSource::GrokCli {
            expected_identity: "user-1".into(),
        },
    }
}

fn write_session(dir: &Path, token: &str, refresh: Option<&str>) -> std::path::PathBuf {
    let mut entry = json!({ "key": token, "user_id": "user-1", "email": "user@example.com" });
    if let Some(r) = refresh {
        entry["refresh_token"] = json!(r);
    }
    let path = dir.join("auth.json");
    std::fs::write(&path, json!({ "https://auth.x.ai::c": entry }).to_string()).unwrap();
    path
}

/// Serves one canned response per URL slot, recording raw request heads.
struct Stub {
    base: String,
    heads: Arc<Mutex<Vec<String>>>,
}

async fn stub(responses: Vec<(u16, &'static str)>) -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let heads = Arc::new(Mutex::new(Vec::new()));
    let seen = heads.clone();
    tokio::spawn(async move {
        for (status, body) in responses {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            seen.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf[..n]).to_string());
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(reply.as_bytes()).await;
        }
    });
    Stub { base, heads }
}

async fn poll(stub: &Stub, path: Option<&Path>) -> AccountUsage {
    let a = format!("{}/v1/billing?format=credits", stub.base);
    let b = format!("{}/v1/billing", stub.base);
    poll_grok_cli_at(
        &reqwest::Client::new(),
        &[a.as_str(), b.as_str()],
        path,
        &account(),
        now(),
    )
    .await
}

const OK_BODY: &str =
    r#"{"config":{"creditUsagePercent":42.5,"currentPeriod":{"end":"2026-10-08T00:00:00Z"}}}"#;

#[tokio::test]
async fn ok_response_maps_percent_and_sends_bearer() {
    let dir = tempfile::tempdir().unwrap();
    let token = jwt_with_exp(now().timestamp() + 3600);
    let path = write_session(dir.path(), &token, Some("r"));
    let s = stub(vec![(200, OK_BODY)]).await;
    let usage = poll(&s, Some(&path)).await;
    assert_eq!(usage.status, "ok");
    assert!((usage.week.as_ref().unwrap().percent - 42.5).abs() < 0.01);
    assert_eq!(usage.plan.as_deref(), Some("Grok"));
    let heads = s.heads.lock().unwrap();
    assert!(
        heads[0].contains(&format!("Bearer {token}")),
        "{}",
        heads[0]
    );
    assert!(heads[0]
        .to_lowercase()
        .contains("x-xai-token-auth: xai-grok-cli"));
}

#[tokio::test]
async fn http_failures_use_upstream_status_mapping() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_session(dir.path(), &jwt_with_exp(now().timestamp() + 3600), None);
    for (code, want) in [(401, "needs_login"), (403, "needs_login")] {
        let s = stub(vec![(code, "{}")]).await;
        assert_eq!(poll(&s, Some(&path)).await.status, want, "{code}");
    }
    let s = stub(vec![(429, "{}"), (429, "{}")]).await;
    assert_eq!(poll(&s, Some(&path)).await.status, "throttled");
    let s = stub(vec![(500, "{}"), (500, "{}")]).await;
    assert_eq!(poll(&s, Some(&path)).await.status, "error");
}

#[tokio::test]
async fn second_endpoint_is_tried_after_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_session(dir.path(), &jwt_with_exp(now().timestamp() + 3600), None);
    let s = stub(vec![(404, "{}"), (200, OK_BODY)]).await;
    assert_eq!(poll(&s, Some(&path)).await.status, "ok");
    assert_eq!(s.heads.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn unparseable_success_body_is_needs_setup() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_session(dir.path(), &jwt_with_exp(now().timestamp() + 3600), None);
    let s = stub(vec![(200, r#"{"error":"nope"}"#)]).await;
    let usage = poll(&s, Some(&path)).await;
    assert_eq!(usage.status, "needs_setup");
    assert!(usage.week.is_none());
}

#[tokio::test]
async fn unusable_session_is_needs_login_without_any_network_call() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.json");
    let expired = write_session(
        dir.path(),
        &jwt_with_exp(now().timestamp() - 10),
        Some("refresh-example"),
    );
    let s = stub(vec![(200, OK_BODY)]).await;
    assert_eq!(poll(&s, None).await.status, "needs_login");
    assert_eq!(poll(&s, Some(&missing)).await.status, "needs_login");
    assert_eq!(poll(&s, Some(&expired)).await.status, "needs_login");
    std::fs::write(dir.path().join("bad.json"), "{{{").unwrap();
    assert_eq!(
        poll(&s, Some(&dir.path().join("bad.json"))).await.status,
        "needs_login"
    );
    assert!(s.heads.lock().unwrap().is_empty());
}

#[tokio::test]
async fn session_for_a_different_identity_is_never_used() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    std::fs::write(
        &path,
        json!({ "s": { "key": jwt_with_exp(now().timestamp() + 3600), "user_id": "someone-else" } })
            .to_string(),
    )
    .unwrap();
    let s = stub(vec![(200, OK_BODY)]).await;
    assert_eq!(poll(&s, Some(&path)).await.status, "needs_login");
    assert!(s.heads.lock().unwrap().is_empty());
}

#[tokio::test]
async fn session_file_is_not_modified_by_polling() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_session(
        dir.path(),
        &jwt_with_exp(now().timestamp() + 3600),
        Some("r"),
    );
    let before = std::fs::read(&path).unwrap();
    let s = stub(vec![(200, OK_BODY)]).await;
    poll(&s, Some(&path)).await;
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn production_endpoints_are_fixed_https_constants() {
    assert!(!GROK_CONSUMER_BILLING_URLS.is_empty());
    for url in GROK_CONSUMER_BILLING_URLS {
        assert!(url.starts_with("https://cli-chat-proxy.grok.com/"), "{url}");
    }
}
