use std::path::Path;

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use scrypt::{scrypt, Params};
use usage_core::account::{Credentials, Provider};

use crate::paths;

use super::{default_label, ImportedAccount};

const MISSING_POE: &str =
    "Poe CLI credentials not found — run `npx poe-code login` first (writes ~/.poe-code/credentials.enc)";
const ENCRYPTED_UNREADABLE: &str =
    "Poe CLI credentials.enc could not be decrypted on this machine — run `npx poe-code login`";

/// Matches poe-code `auth-store` EncryptedFileStore (AES-256-GCM, scrypt
/// over `hostname:username`, salt `poe-code:encrypted-file-auth-store:v1`).
/// Machine-derived — no user password.
const POE_ENC_SALT: &[u8] = b"poe-code:encrypted-file-auth-store:v1";
const POE_ENC_VERSION: u64 = 1;
const SCRYPT_LOG_N: u8 = 14;
const SCRYPT_R: u32 = 8;
const SCRYPT_P: u32 = 1;
const KEY_LEN: usize = 32;
const IV_LEN: usize = 12;
const TAG_LEN: usize = 16;

fn nonempty(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn json_key(v: &serde_json::Value) -> Option<String> {
    v.as_str().and_then(nonempty)
}

/// Pulls an API key from a plaintext Poe JSON document (`credentials.json` or
/// `config.json`). Never logs the value.
pub fn parse_poe_plaintext_key(root: &serde_json::Value) -> Option<String> {
    for key_name in ["apiKey", "api_key", "key", "POE_API_KEY"] {
        if let Some(key) = root.get(key_name).and_then(json_key) {
            return Some(key);
        }
    }
    if let Some(nested) = root.get("core") {
        for key_name in ["apiKey", "api_key"] {
            if let Some(key) = nested.get(key_name).and_then(json_key) {
                return Some(key);
            }
        }
    }
    None
}

fn derive_poe_key(hostname: &str, username: &str) -> Option<[u8; KEY_LEN]> {
    let secret = format!("{hostname}:{username}");
    let params = Params::new(SCRYPT_LOG_N, SCRYPT_R, SCRYPT_P, KEY_LEN).ok()?;
    let mut key = [0u8; KEY_LEN];
    scrypt(secret.as_bytes(), POE_ENC_SALT, &params, &mut key).ok()?;
    Some(key)
}

/// Decrypts poe-code `credentials.enc` JSON `{version, iv, authTag, ciphertext}`.
pub fn decrypt_poe_credentials_enc(raw: &str, hostname: &str, username: &str) -> Option<String> {
    let doc: serde_json::Value = serde_json::from_str(raw).ok()?;
    let version = doc.get("version")?.as_u64()?;
    if version != POE_ENC_VERSION {
        return None;
    }
    let iv = STANDARD.decode(doc.get("iv")?.as_str()?).ok()?;
    let tag = STANDARD.decode(doc.get("authTag")?.as_str()?).ok()?;
    let ciphertext = STANDARD.decode(doc.get("ciphertext")?.as_str()?).ok()?;
    if iv.len() != IV_LEN || tag.len() != TAG_LEN {
        return None;
    }
    let key = derive_poe_key(hostname, username)?;
    let cipher = Aes256Gcm::new_from_slice(&key).ok()?;
    let mut payload = ciphertext;
    payload.extend_from_slice(&tag);
    let plain = cipher
        .decrypt(Nonce::from_slice(&iv), payload.as_ref())
        .ok()?;
    let text = String::from_utf8(plain).ok()?;
    key_from_plaintext(&text)
}

fn key_from_plaintext(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(root) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(key) = parse_poe_plaintext_key(&root) {
            return Some(key);
        }
    }
    nonempty(trimmed)
}

fn load_json_key(path: &Path) -> Option<String> {
    let data = std::fs::read_to_string(path).ok()?;
    if let Ok(root) = serde_json::from_str::<serde_json::Value>(&data) {
        if let Some(key) = parse_poe_plaintext_key(&root) {
            return Some(key);
        }
    }
    key_from_plaintext(&data)
}

fn machine_identity() -> Option<(String, String)> {
    let hostname = hostname::get().ok()?.into_string().ok()?;
    let username = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .or_else(|_| std::env::var("LOGNAME"))
        .ok()
        .and_then(|s| nonempty(&s))?;
    Some((hostname, username))
}

fn imported(key: String) -> ImportedAccount {
    ImportedAccount {
        label: default_label(Provider::Poe),
        credentials: Credentials {
            access_token: key,
            refresh_token: None,
            account_id: None,
            expires_at: None,
        },
    }
}

pub(crate) fn load_poe_cli_auth_from(
    enc: Option<&Path>,
    credentials_json: Option<&Path>,
    config_json: Option<&Path>,
    hostname: Option<&str>,
    username: Option<&str>,
) -> Result<ImportedAccount, String> {
    let mut saw_enc = false;
    if let Some(path) = enc {
        if path.is_file() {
            saw_enc = true;
            if let Ok(raw) = std::fs::read_to_string(path) {
                if let (Some(host), Some(user)) = (hostname, username) {
                    if let Some(key) = decrypt_poe_credentials_enc(&raw, host, user) {
                        return Ok(imported(key));
                    }
                }
            }
        }
    }
    for path in [credentials_json, config_json].into_iter().flatten() {
        if let Some(key) = load_json_key(path) {
            return Ok(imported(key));
        }
    }
    if saw_enc {
        return Err(ENCRYPTED_UNREADABLE.into());
    }
    Err(MISSING_POE.into())
}

pub fn load_poe_cli_auth() -> Result<ImportedAccount, String> {
    let (hostname, username) = match machine_identity() {
        Some(pair) => (Some(pair.0), Some(pair.1)),
        None => (None, None),
    };
    load_poe_cli_auth_from(
        paths::poe_credentials_enc().as_deref(),
        paths::poe_credentials_json().as_deref(),
        paths::poe_config_json().as_deref(),
        hostname.as_deref(),
        username.as_deref(),
    )
}

#[cfg(test)]
pub(crate) fn encrypt_poe_credentials_enc(
    plaintext: &str,
    hostname: &str,
    username: &str,
    iv: &[u8; IV_LEN],
) -> String {
    let key = derive_poe_key(hostname, username).expect("derive poe key");
    let cipher = Aes256Gcm::new_from_slice(&key).expect("aes key");
    let sealed = cipher
        .encrypt(Nonce::from_slice(iv), plaintext.as_bytes())
        .expect("encrypt poe key");
    let split = sealed.len().saturating_sub(TAG_LEN);
    let (ciphertext, tag) = sealed.split_at(split);
    serde_json::json!({
        "version": POE_ENC_VERSION,
        "iv": STANDARD.encode(iv),
        "authTag": STANDARD.encode(tag),
        "ciphertext": STANDARD.encode(ciphertext),
    })
    .to_string()
}
