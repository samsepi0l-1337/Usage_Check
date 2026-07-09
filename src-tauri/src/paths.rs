//! Cross-platform home / provider config paths.
//!
//! Mirrors the Swift `UsagePaths` layout so local-log scanning and CLI auth
//! import work the same on macOS and Windows.

use std::path::PathBuf;

/// User home directory. Prefers `HOME` (Unix) then `USERPROFILE` (Windows).
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Codex config root: `CODEX_HOME` if set, otherwise `~/.codex`.
pub fn codex_home() -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os("CODEX_HOME") {
        let p = PathBuf::from(raw);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    home_dir().map(|h| h.join(".codex"))
}

/// Codex `auth.json` path.
pub fn codex_auth_file() -> Option<PathBuf> {
    codex_home().map(|h| h.join("auth.json"))
}

/// Codex session roots (`sessions` + `archived_sessions`).
pub fn codex_session_roots() -> Vec<PathBuf> {
    let Some(home) = codex_home() else {
        return Vec::new();
    };
    vec![home.join("sessions"), home.join("archived_sessions")]
}

/// Claude config roots: `CLAUDE_CONFIG_DIR` (comma-separated) or the default
/// `~/.claude` and `~/.config/claude`.
pub fn claude_config_roots() -> Vec<PathBuf> {
    if let Ok(raw) = std::env::var("CLAUDE_CONFIG_DIR") {
        let parts: Vec<PathBuf> = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect();
        if !parts.is_empty() {
            return parts;
        }
    }
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    vec![home.join(".claude"), home.join(".config").join("claude")]
}

/// Claude `.credentials.json` candidates.
pub fn claude_credential_files() -> Vec<PathBuf> {
    claude_config_roots()
        .into_iter()
        .map(|r| r.join(".credentials.json"))
        .collect()
}

/// macOS Keychain / Windows Credential Manager service name used by Claude Code.
///
/// Matches Claude Code CLI: default `Claude Code-credentials`, or
/// `Claude Code-credentials-{sha256(CLAUDE_CONFIG_DIR)[0..8]}` when
/// `CLAUDE_CONFIG_DIR` is set.
pub fn claude_keychain_service_name() -> String {
    use sha2::{Digest, Sha256};

    match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(dir) if !dir.trim().is_empty() => {
            let hash = Sha256::digest(dir.as_bytes());
            let short = hex_prefix(&hash, 8);
            format!("Claude Code-credentials-{short}")
        }
        _ => "Claude Code-credentials".to_string(),
    }
}

fn hex_prefix(bytes: impl AsRef<[u8]>, n: usize) -> String {
    let needed = n.div_ceil(2);
    bytes.as_ref()[..needed.min(bytes.as_ref().len())]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
        .chars()
        .take(n)
        .collect()
}

/// Claude project roots used for JSONL scanning.
pub fn claude_project_roots() -> Vec<PathBuf> {
    claude_config_roots()
        .into_iter()
        .map(|root| {
            if root.file_name().and_then(|n| n.to_str()) == Some("projects") {
                root
            } else {
                root.join("projects")
            }
        })
        .collect()
}

/// Gemini / agy transcript roots.
pub fn gemini_log_roots() -> Vec<PathBuf> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    vec![home.join(".gemini"), home.join(".config").join("gemini")]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_dir_resolves_something() {
        // In CI/dev this should always be set on macOS/Linux; on Windows
        // USERPROFILE is expected. Either way the helper must not panic.
        let _ = home_dir();
    }

    #[test]
    fn codex_auth_file_ends_with_auth_json() {
        if let Some(p) = codex_auth_file() {
            assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("auth.json"));
        }
    }

    #[test]
    fn claude_credential_files_end_with_credentials() {
        for p in claude_credential_files() {
            assert_eq!(
                p.file_name().and_then(|n| n.to_str()),
                Some(".credentials.json")
            );
        }
    }

    #[test]
    fn claude_keychain_service_default_name() {
        // Unset CLAUDE_CONFIG_DIR for this assertion when possible; if the
        // ambient env already sets it, just check the hashed form.
        let name = claude_keychain_service_name();
        assert!(
            name == "Claude Code-credentials"
                || name.starts_with("Claude Code-credentials-"),
            "unexpected service name: {name}"
        );
        if name.contains('-') && name != "Claude Code-credentials" {
            let suffix = name.rsplit('-').next().unwrap();
            assert_eq!(suffix.len(), 8);
            assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }
}
