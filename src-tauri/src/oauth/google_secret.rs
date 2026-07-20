use super::{AGY_PREFERRED_CLIENT_PREFIX, GOCSPX_PREFIX, GOOGLE_CLIENT_SUFFIX};

/// Resolves Antigravity Google OAuth client_id + client_secret without
/// embedding them in source (GitHub push protection blocks that).
///
/// Order:
/// 1. `ANTIGRAVITY_OAUTH_CLIENT_ID` + `ANTIGRAVITY_OAUTH_CLIENT_SECRET`
/// 2. Scan a local `agy` / Antigravity.app binary for the embedded pair
pub fn resolve_agy_oauth_client() -> Result<(String, String), String> {
    if let (Ok(id), Ok(secret)) = (
        std::env::var("ANTIGRAVITY_OAUTH_CLIENT_ID"),
        std::env::var("ANTIGRAVITY_OAUTH_CLIENT_SECRET"),
    ) {
        let id = id.trim().to_string();
        let secret = secret.trim().to_string();
        if !id.is_empty() && !secret.is_empty() {
            return Ok((id, secret));
        }
    }

    for path in agy_oauth_binary_candidates() {
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() || meta.len() == 0 {
            continue;
        }
        // Skip tiny stubs / huge unrelated files.
        if meta.len() < 1024 || meta.len() > 400_000_000 {
            continue;
        }
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        if let Some(pair) = extract_google_oauth_pair(&data) {
            return Ok(pair);
        }
    }

    Err(
        "Antigravity OAuth credentials not found — set ANTIGRAVITY_OAUTH_CLIENT_ID and \
         ANTIGRAVITY_OAUTH_CLIENT_SECRET, or install Antigravity.app / agy so UsageCheck can \
         read the embedded Google OAuth client"
            .into(),
    )
}

fn agy_oauth_binary_candidates() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut push_unique = |p: std::path::PathBuf| {
        if !out.iter().any(|x| x == &p) {
            out.push(p);
        }
    };

    if let Ok(p) = std::env::var("ANTIGRAVITY_CLI_PATH") {
        push_unique(std::path::PathBuf::from(p));
    }
    // GUI apps often lack Homebrew on PATH — still probe absolute locations.
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = std::path::PathBuf::from(home);
        push_unique(home.join(".local/bin/agy"));
        push_unique(home.join("bin/agy"));
    }
    push_unique(std::path::PathBuf::from("/opt/homebrew/bin/agy"));
    push_unique(std::path::PathBuf::from("/usr/local/bin/agy"));
    push_unique(std::path::PathBuf::from(
        "/Applications/Antigravity.app/Contents/Resources/bin/language_server",
    ));
    push_unique(std::path::PathBuf::from(
        "/Applications/Antigravity.app/Contents/MacOS/Antigravity",
    ));
    // Windows installs (best-effort).
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        let pf = std::path::PathBuf::from(pf);
        push_unique(pf.join("Antigravity/Antigravity.exe"));
        push_unique(pf.join("Antigravity/resources/bin/language_server.exe"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = std::path::PathBuf::from(local);
        push_unique(local.join("Programs/Antigravity/Antigravity.exe"));
        push_unique(local.join("Antigravity/agy.exe"));
    }
    // PATH lookup for `agy` when present (CLI shells / packaged PATH).
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            push_unique(dir.join("agy"));
            push_unique(dir.join("agy.exe"));
        }
    }
    out
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    if needle.is_empty() || haystack.len() < needle.len() {
        return out;
    }
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            out.push(i);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

fn is_secret_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// Finds `N-xxx.apps.googleusercontent.com` even when glued to adjacent ASCII
/// in a Go binary (no NUL separators).
fn find_google_client_ids(data: &[u8]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for end_at in find_bytes(data, GOOGLE_CLIENT_SUFFIX) {
        let suffix_end = end_at + GOOGLE_CLIENT_SUFFIX.len();
        // Parse only the token immediately before this suffix:
        //   {digits}-{alphanumeric_id}.apps.googleusercontent.com
        // Do not walk through earlier glued tokens (binaries concatenate many
        // ASCII strings with no separators).
        let mut i = end_at;
        while i > 0 && data[i - 1].is_ascii_alphanumeric() {
            i -= 1;
        }
        if i == end_at {
            continue;
        }
        if i == 0 || data[i - 1] != b'-' {
            continue;
        }
        i -= 1; // consume '-'
        let digit_end = i;
        while i > 0 && data[i - 1].is_ascii_digit() {
            i -= 1;
        }
        if i == digit_end {
            continue;
        }
        let start = i;
        let raw = &data[start..suffix_end];
        let Ok(text) = std::str::from_utf8(raw) else {
            continue;
        };
        if text.len() < 40 {
            continue;
        }
        if !out.iter().any(|(_, c)| c == text) {
            out.push((start, text.to_string()));
        }
    }
    out
}

#[derive(Clone, Debug)]
struct FoundSecret {
    pos: usize,
    value: String,
    /// True when this secret is immediately followed by another `GOCSPX-`
    /// (Antigravity packs two secrets back-to-back; the first is the
    /// enterprise client secret used with the `1071006060591-` client).
    followed_by_gocspx: bool,
}

/// Finds `GOCSPX-…` secrets. Antigravity binaries concatenate two secrets
/// then a URL (`GOCSPX-…GOCSPX-…https://…`); stop at the next `GOCSPX-`,
/// before `http`, and at the observed 28-char body length.
fn find_gocspx_secrets(data: &[u8]) -> Vec<FoundSecret> {
    // Live Antigravity/agy client secrets use a 28-char body after `GOCSPX-`.
    const MAX_BODY: usize = 28;
    let mut out = Vec::new();
    for start in find_bytes(data, GOCSPX_PREFIX) {
        let body_start = start + GOCSPX_PREFIX.len();
        let mut end = body_start;
        while end < data.len()
            && end - body_start < MAX_BODY
            && is_secret_byte(data[end])
            && !data[end..].starts_with(GOCSPX_PREFIX)
            && !data[end..].starts_with(b"http")
        {
            end += 1;
        }
        if end - body_start < 10 {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&data[start..end]) else {
            continue;
        };
        if out.iter().any(|s: &FoundSecret| s.value == text) {
            continue;
        }
        out.push(FoundSecret {
            pos: start,
            value: text.to_string(),
            followed_by_gocspx: data[end..].starts_with(GOCSPX_PREFIX),
        });
    }
    out
}

/// Scans binary bytes for a Google OAuth client id + `GOCSPX-…` secret pair.
/// Prefers the Antigravity enterprise client id prefix when several exist.
pub fn extract_google_oauth_pair(data: &[u8]) -> Option<(String, String)> {
    let clients = find_google_client_ids(data);
    let secrets = find_gocspx_secrets(data);
    if clients.is_empty() || secrets.is_empty() {
        return None;
    }

    let (client_pos, client) = clients
        .iter()
        .find(|(_, c)| c.starts_with(AGY_PREFERRED_CLIENT_PREFIX))
        .cloned()
        .unwrap_or_else(|| clients[0].clone());

    // Enterprise client (`1071006…`) is paired with the first secret of the
    // concatenated GOCSPX pair in Antigravity binaries — not merely the
    // nearest-by-offset secret (which can be the secondary one).
    if client.starts_with(AGY_PREFERRED_CLIENT_PREFIX) {
        if let Some(s) = secrets.iter().find(|s| s.followed_by_gocspx) {
            return Some((client, s.value.clone()));
        }
    }

    let mut best: Option<(usize, String)> = None;
    for secret in &secrets {
        let dist = client_pos.abs_diff(secret.pos);
        if best.as_ref().map(|(d, _)| dist < *d).unwrap_or(true) {
            best = Some((dist, secret.value.clone()));
        }
    }
    let secret = best?.1;
    Some((client, secret))
}
