use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use sha2::{Digest, Sha256};
use tiny_http::Server;

use super::ProviderOAuth;

/// Generates a PKCE verifier (43-128 chars, unreserved base64url charset) and
/// its S256 code challenge. Pure function — no I/O, no logging.
pub fn make_pkce() -> (String, String) {
    let mut bytes = [0u8; 64];
    rand::thread_rng().fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let digest = hasher.finalize();
    let challenge = URL_SAFE_NO_PAD.encode(digest);

    (verifier, challenge)
}

/// Generates a random opaque `state` value for CSRF protection on the OAuth
/// callback. Pure function — no I/O.
pub fn make_state() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Assembles the provider authorize URL with OAuth query parameters (plus any
/// provider-specific extras). When `cfg.use_pkce` is true, includes S256
/// `code_challenge`. Pure function — no I/O.
pub fn build_authorize_url(cfg: &ProviderOAuth, challenge: &str, redirect: &str, state: &str) -> String {
    let mut url = String::with_capacity(512);
    url.push_str(&cfg.auth_url);
    url.push('?');
    url.push_str("response_type=code");
    url.push_str("&client_id=");
    url.push_str(&urlencoding::encode(&cfg.client_id));
    url.push_str("&redirect_uri=");
    url.push_str(&urlencoding::encode(redirect));
    url.push_str("&scope=");
    url.push_str(&urlencoding::encode(&cfg.scopes));
    if cfg.use_pkce {
        url.push_str("&code_challenge=");
        url.push_str(&urlencoding::encode(challenge));
        url.push_str("&code_challenge_method=S256");
    }
    url.push_str("&state=");
    url.push_str(&urlencoding::encode(state));
    for (key, value) in &cfg.extra_authorize_params {
        url.push('&');
        url.push_str(&urlencoding::encode(key));
        url.push('=');
        url.push_str(&urlencoding::encode(value));
    }
    url
}

/// Parsed `?code=...&state=...` callback query parameters.
pub(crate) struct CallbackParams {
    pub(crate) code: String,
    pub(crate) state: String,
}

/// Extracts `code` and `state` from a callback request path/query string.
/// Pure function — no I/O.
pub(crate) fn parse_callback_query(url: &str) -> Option<CallbackParams> {
    let query = url.split_once('?')?.1;
    let mut code: Option<String> = None;
    let mut state: Option<String> = None;

    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        // The callback query values are percent-decoded before use.
        let decoded = urlencoding::decode(value).ok()?.into_owned();
        match key {
            "code" => code = Some(decoded),
            "state" => state = Some(decoded),
            _ => {}
        }
    }

    Some(CallbackParams {
        code: code?,
        state: state?,
    })
}

pub(super) fn bind_callback_server(cfg: &ProviderOAuth) -> Result<(Server, String), String> {
    match cfg.fixed_redirect {
        Some((port, path)) => {
            let server = Server::http(format!("127.0.0.1:{port}")).map_err(|e| {
                format!(
                    "failed to bind OAuth callback on 127.0.0.1:{port}{path}: {e} \
                     (is another login already using this port?)"
                )
            })?;
            // OpenAI registers `localhost`, not `127.0.0.1` — must match exactly.
            Ok((server, format!("http://localhost:{port}{path}")))
        }
        None => {
            let server = Server::http("127.0.0.1:0")
                .map_err(|e| format!("failed to bind localhost callback server: {e}"))?;
            let port = server
                .server_addr()
                .to_ip()
                .map(|a| a.port())
                .ok_or_else(|| "failed to determine callback server port".to_string())?;
            Ok((server, format!("http://127.0.0.1:{port}/callback")))
        }
    }
}
