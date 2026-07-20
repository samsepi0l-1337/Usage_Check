//! HTTP transport for the local usage API: binds the localhost `tiny_http`
//! server, applies optional bearer-token auth ([`crate::api_auth`]), and maps
//! each request through the pure router in [`crate::api`].
//!
//! Kept separate from `api.rs` so the DTOs/router/state stay transport-agnostic
//! and within the file-size budget.

use tiny_http::{Header, Response, Server};

use crate::api::{self, ApiState, Reply};

fn unauthorized() -> Reply {
    api::json(
        401,
        r#"{"error":"unauthorized","message":"missing or invalid bearer token"}"#.to_string(),
    )
}

/// Reads the `Authorization` header value from a request, if present.
fn auth_header(request: &tiny_http::Request) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .map(|h| h.value.as_str().to_string())
}

/// Starts the localhost API server on a dedicated thread. No-op when disabled
/// via env. Bind failures are logged (not fatal) so the tray still runs.
pub fn spawn(state: ApiState) {
    if api::is_disabled() {
        return;
    }
    let port = api::configured_port();
    std::thread::Builder::new()
        .name("usagecheck-api".into())
        .spawn(move || {
            let addr = format!("127.0.0.1:{port}");
            let server = match Server::http(&addr) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("api: failed to bind {addr}: {e} (is another instance running?)");
                    return;
                }
            };
            for request in server.incoming_requests() {
                // Strip any query string before routing.
                let path = request.url().split('?').next().unwrap_or("/").to_string();
                let method = request.method().as_str().to_string();
                let header_value = auth_header(&request);
                let reply = if crate::api_auth::check(&path, header_value.as_deref()) {
                    api::route(&state, &method, &path)
                } else {
                    unauthorized()
                };
                let header =
                    Header::from_bytes(&b"Content-Type"[..], reply.content_type.as_bytes())
                        .expect("static content-type header is valid");
                let response = Response::from_string(reply.body)
                    .with_status_code(reply.status)
                    .with_header(header);
                let _ = request.respond(response);
            }
        })
        .expect("failed to spawn usagecheck-api thread");
}
