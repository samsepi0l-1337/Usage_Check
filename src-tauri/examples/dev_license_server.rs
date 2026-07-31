//! Local-only implementation of the real UsageCheck license wire contract.
//!
//! This example is intentionally separate from the application binary: it
//! owns a throwaway private key under `.dev/`, while the app receives only the
//! printed public key through its debug-build-only environment override.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Duration, Utc};
use ed25519_dalek::{Signer, SigningKey};
use rand::Rng;
use serde::{Deserialize, Serialize};
use tiny_http::{Header, Request, Response, Server};

const DEFAULT_PORT: u16 = 5179;

#[derive(Debug)]
struct Config {
    key_file: PathBuf,
    port: u16,
}

#[derive(Deserialize)]
struct LicenseRequest {
    key: String,
    device: String,
    #[serde(rename = "app_version")]
    _app_version: String,
}

#[derive(Serialize)]
struct TokenPayload {
    v: u8,
    key_id: String,
    plan: String,
    device: String,
    issued_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
}

fn repo_root_default_key_file() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri manifest must have a repository-root parent")
        .join(".dev/license-signing-key")
}

fn parse_args() -> Result<Config, String> {
    let mut key_file = repo_root_default_key_file();
    let mut port = DEFAULT_PORT;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--key-file" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--key-file needs a path".to_string())?;
                key_file = PathBuf::from(value);
            }
            "--port" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--port needs a port number".to_string())?;
                port = value
                    .parse::<u16>()
                    .ok()
                    .filter(|port| *port != 0)
                    .ok_or_else(|| "--port must be an integer from 1 to 65535".to_string())?;
            }
            "--help" | "-h" => {
                return Err("usage: cargo run -p usage-app --example dev_license_server -- [--key-file PATH] [--port PORT]".to_string());
            }
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }
    Ok(Config { key_file, port })
}

fn open_private_key_file(path: &Path) -> Result<fs::File, String> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|error| format!("create dev signing key: {error}"))
}

fn load_or_create_signing_key(path: &Path) -> Result<SigningKey, String> {
    let parent = path
        .parent()
        .ok_or_else(|| "dev signing key path has no parent directory".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("create dev key directory: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("restrict dev key directory permissions: {error}"))?;
    }

    match fs::read(path) {
        Ok(bytes) => {
            let seed: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| "dev signing key file must contain exactly 32 bytes".to_string())?;
            Ok(SigningKey::from_bytes(&seed))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut seed = [0u8; 32];
            rand::rng().fill_bytes(&mut seed);
            let mut file = open_private_key_file(path)?;
            if let Err(error) = file.write_all(&seed) {
                drop(file);
                let _ = fs::remove_file(path);
                return Err(format!("write dev signing key: {error}"));
            }
            Ok(SigningKey::from_bytes(&seed))
        }
        Err(error) => Err(format!("read dev signing key: {error}")),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn private_key_mode_is_restricted_at_open_time() {
        let temp = tempfile::tempdir().unwrap();
        let key_path = temp.path().join("license-signing-key");

        let _file = open_private_key_file(&key_path).unwrap();

        assert_eq!(
            fs::metadata(&key_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn private_key_file_and_parent_are_restricted_when_created() {
        let temp = tempfile::tempdir().unwrap();
        let key_path = temp.path().join("private").join("license-signing-key");

        let _key = load_or_create_signing_key(&key_path).unwrap();

        assert_eq!(
            fs::metadata(key_path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&key_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn existing_private_key_parent_is_restricted_when_loaded() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("private");
        let key_path = parent.join("license-signing-key");
        fs::create_dir(&parent).unwrap();
        fs::write(&key_path, [7u8; 32]).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();

        let _key = load_or_create_signing_key(&key_path).unwrap();

        assert_eq!(
            fs::metadata(parent).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}

fn encode_token(payload: &TokenPayload, signing_key: &SigningKey) -> Result<String, String> {
    let payload_bytes = serde_json::to_vec(payload).map_err(|error| format!("serialize token: {error}"))?;
    let signature = signing_key.sign(&payload_bytes);
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(payload_bytes),
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    ))
}

fn json_response(request: Request, status: u16, body: serde_json::Value) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
        .expect("static content-type header is valid");
    let response = Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(header);
    let _ = request.respond(response);
}

fn error_response(request: Request, status: u16, error: &str, message: &str) {
    json_response(
        request,
        status,
        serde_json::json!({ "error": error, "message": message }),
    );
}

fn main() {
    let config = match parse_args() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("dev-license-server: {message}");
            std::process::exit(2);
        }
    };
    let signing_key = match load_or_create_signing_key(&config.key_file) {
        Ok(signing_key) => signing_key,
        Err(message) => {
            eprintln!("dev-license-server: {message}");
            std::process::exit(1);
        }
    };
    let address = format!("127.0.0.1:{}", config.port);
    let server = match Server::http(&address) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("dev-license-server: failed to bind {address}: {error}");
            std::process::exit(1);
        }
    };
    let endpoint = format!("http://{address}/api/license/verify");
    {
        let mut stdout = std::io::stdout().lock();
        writeln!(
            stdout,
            "USAGECHECK_LICENSE_PUBKEY={}",
            base64::engine::general_purpose::STANDARD.encode(signing_key.verifying_key().to_bytes())
        )
        .expect("write dev public key");
        writeln!(stdout, "USAGECHECK_LICENSE_API={endpoint}").expect("write dev endpoint");
        stdout.flush().expect("flush dev server configuration");
    }

    let mut last_issued_at: Option<DateTime<Utc>> = None;
    for mut request in server.incoming_requests() {
        if request.method().as_str() != "POST" {
            error_response(request, 405, "method_not_allowed", "only POST is supported");
            continue;
        }

        let mut body = String::new();
        if request.as_reader().read_to_string(&mut body).is_err() {
            error_response(request, 400, "invalid_request", "could not read request body");
            continue;
        }
        let request_body: LicenseRequest = match serde_json::from_str(&body) {
            Ok(request_body) => request_body,
            Err(_) => {
                error_response(request, 400, "invalid_request", "request must be valid JSON");
                continue;
            }
        };

        match request_body.key.as_str() {
            "invalid" => {
                error_response(request, 400, "invalid_key", "the development key is invalid");
                continue;
            }
            "revoked" => {
                error_response(request, 403, "revoked", "the development key is revoked");
                continue;
            }
            "device_limit" => {
                error_response(request, 403, "device_limit", "the development key reached its device limit");
                continue;
            }
            _ => {}
        }

        let now = Utc::now();
        let issued_at = match last_issued_at {
            Some(previous) if now <= previous => previous + Duration::nanoseconds(1),
            _ => now,
        };
        last_issued_at = Some(issued_at);
        let payload = TokenPayload {
            v: 1,
            key_id: "dev".to_string(),
            plan: "pro".to_string(),
            device: request_body.device,
            issued_at,
            expires_at: (request_body.key == "expired").then_some(issued_at - Duration::seconds(1)),
        };
        match encode_token(&payload, &signing_key) {
            Ok(token) => json_response(request, 200, serde_json::json!({ "token": token })),
            Err(_) => error_response(request, 500, "server_error", "could not mint development token"),
        }
    }
}
