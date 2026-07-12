//! Cross-platform home / provider config paths.
//!
//! Mirrors the Swift `UsagePaths` layout so local-log scanning and CLI auth
//! import work the same on macOS and Windows.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

const APP_DIR: &str = "UsageCheck";

#[path = "paths_profiles.rs"]
pub mod paths_profiles;
pub use paths_profiles::{claude_managed_root, claude_settings_json, claude_statusline_snapshot};

/// User home directory. Prefers `HOME` (Unix) then `USERPROFILE` (Windows).
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// UsageCheck's application-owned data root.
pub fn usagecheck_app_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        home_dir().map(|home| {
            home.join("Library")
                .join("Application Support")
                .join(APP_DIR)
        })
    }
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(home_dir)
            .map(|root| root.join(APP_DIR));
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        home_dir().map(|home| home.join(".local").join("share").join(APP_DIR))
    }
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
    codex_home().map(|home| codex_auth_file_for(&home))
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
            let short = hex_prefix(hash, 8);
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

/// Cursor `state.vscdb` (read-only) under globalStorage.
#[cfg(feature = "edition-pro")]
pub fn cursor_state_vscdb() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        home_dir().map(|h| {
            h.join("Library")
                .join("Application Support")
                .join("Cursor")
                .join("User")
                .join("globalStorage")
                .join("state.vscdb")
        })
    }
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(home_dir)
            .map(|h| {
                h.join("Cursor")
                    .join("User")
                    .join("globalStorage")
                    .join("state.vscdb")
            });
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        return home_dir().map(|h| {
            h.join(".config")
                .join("Cursor")
                .join("User")
                .join("globalStorage")
                .join("state.vscdb")
        });
    }
}

/// Higgsfield CLI credentials (`higgsfield auth login`).
#[cfg(feature = "edition-pro")]
pub fn higgsfield_credentials_file() -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("HIGGSFIELD_CONFIG_DIR") {
        let p = PathBuf::from(raw);
        if !p.as_os_str().is_empty() {
            return Some(p.join("credentials.json"));
        }
    }
    home_dir().map(|h| {
        h.join(".config")
            .join("higgsfield")
            .join("credentials.json")
    })
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
            name == "Claude Code-credentials" || name.starts_with("Claude Code-credentials-"),
            "unexpected service name: {name}"
        );
        if name.contains('-') && name != "Claude Code-credentials" {
            let suffix = name.rsplit('-').next().unwrap();
            assert_eq!(suffix.len(), 8);
            assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }
}

use usage_core::account::Provider;
use usage_core::models::RootIdentity;

/// Codex session roots for a given profile root.
pub fn codex_session_roots_for(profile_root: &Path) -> Vec<PathBuf> {
    vec![
        profile_root.join("sessions"),
        profile_root.join("archived_sessions"),
    ]
}

/// Claude project roots for a given profile root.
pub fn claude_project_roots_for(profile_root: &Path) -> Vec<PathBuf> {
    let root = if profile_root.file_name().and_then(|name| name.to_str()) == Some("projects") {
        profile_root.to_path_buf()
    } else {
        profile_root.join("projects")
    };
    vec![root]
}

/// Codex auth.json file for a given profile root (returns PathBuf, not Option).
pub fn codex_auth_file_for(profile_root: &Path) -> PathBuf {
    profile_root.join("auth.json")
}

/// Canonical, deduplicated Codex CLI profile roots.
pub fn codex_profile_roots(extra_roots: &[PathBuf]) -> Vec<PathBuf> {
    deduplicated_roots(extra_roots)
}

/// Canonical, deduplicated Claude CLI profile roots.
pub fn claude_profile_roots(extra_roots: &[PathBuf]) -> Vec<PathBuf> {
    deduplicated_roots(extra_roots)
}

/// Extract identity (RootIdentity enum, not Option<String>) from a profile root.
pub fn root_identity(provider: Provider, profile_root: &Path) -> RootIdentity {
    match provider {
        Provider::Codex => codex_identity(profile_root),
        Provider::Claude => RootIdentity::ClaudeEmail { email: None },
        _ => RootIdentity::None,
    }
}

fn codex_identity(profile_root: &Path) -> RootIdentity {
    let identity = std::fs::read_to_string(codex_auth_file_for(profile_root))
        .ok()
        .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
        .and_then(|json| crate::import::parse_codex_auth_json(&json));

    match identity {
        Some((credentials, email)) => RootIdentity::CodexAuth {
            account_id: credentials.account_id,
            email,
        },
        None => RootIdentity::None,
    }
}

fn deduplicated_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    roots
        .iter()
        .filter_map(|root| {
            let normalized = root
                .canonicalize()
                .unwrap_or_else(|_| normalize_lexically(root));
            seen.insert(normalized.clone()).then_some(normalized)
        })
        .collect()
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests_paths {
    use super::*;

    #[test]
    fn test_path_normalization_dedup() {
        // §6.11: relative paths, trailing slashes → canonical dedup (no double-count).
        let roots = vec![
            PathBuf::from("./profiles/codex"),
            PathBuf::from("./profiles/codex/"),
            PathBuf::from("profiles/codex"),
        ];
        let result = codex_profile_roots(&roots);
        // Expected: 1 (canonical dedup). Stub panics, test fails (RED).
        assert_eq!(result.len(), 1, "Should deduplicate to 1 root");
    }

    #[test]
    fn test_codex_auth_file_path() {
        // §6.10: auth file path is constructed correctly (real path, not Option).
        let root = PathBuf::from("/profiles/codex");
        let result = codex_auth_file_for(root.as_path());
        // Expected: PathBuf pointing to /profiles/codex/auth.json (or similar).
        // Stub panics, test fails (RED).
        assert!(
            result.to_string_lossy().contains("auth.json"),
            "Auth file path should include auth.json"
        );
    }
}

/// Codex managed root for app-isolated profiles: profiles/codex/<uuid> under app data dir.
pub fn codex_managed_root() -> Option<PathBuf> {
    usagecheck_app_data_dir().map(|d| {
        d.join("profiles")
            .join("codex")
            .join(uuid::Uuid::new_v4().to_string())
    })
}

/// Codex home for a given profile root (returns the root unchanged).
pub fn codex_home_for_profile(profile_root: &Path) -> PathBuf {
    profile_root.to_path_buf()
}

/// Codex default home: CODEX_HOME env or ~/.codex.
pub fn codex_default_home() -> Option<PathBuf> {
    codex_home() // Reuse existing helper
}

#[cfg(test)]
mod tests_codex_managed {
    use super::*;

    #[test]
    fn test_codex_managed_root_under_app_data() {
        if let Some(root) = codex_managed_root() {
            let root_str = root.to_string_lossy();
            assert!(
                root_str.contains("profiles") && root_str.contains("codex"),
                "managed root should contain profiles/codex: {}",
                root_str
            );
        }
    }

    #[test]
    fn test_codex_home_for_profile_returns_input() {
        let input = PathBuf::from("/tmp/test_profile");
        let output = codex_home_for_profile(&input);
        assert_eq!(output, input);
    }
}

/// Get the default CLAUDE_CONFIG_DIR if it exists
pub fn default_claude_config_dir_checked() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        let path = PathBuf::from(dir);
        if path.exists() {
            return Some(path);
        }
    }
    home_dir().and_then(|h| {
        let default = h.join(".claude");
        if default.exists() {
            Some(default)
        } else {
            None
        }
    })
}
