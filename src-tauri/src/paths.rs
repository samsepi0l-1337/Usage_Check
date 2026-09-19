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
///
/// Debug-build-only test seam: `USAGECHECK_APP_DATA_DIR`, if set and
/// non-empty, overrides the platform-derived path below. This exists so
/// integration tests can point the license/account-store machinery at an
/// isolated tempdir WITHOUT mutating the much broader-blast-radius `HOME`
/// env var (which also redirects Claude/Codex/Cursor config resolution) and
/// without racing the `HOME` mutation another test file already owns (see
/// `claude_statusline_tests.rs`). Gated by `cfg!(debug_assertions)` for the
/// same reason `USAGECHECK_LICENSE_PUBKEY` is (`license/pubkey.rs`): a
/// release binary must never have its app-data location redirected by an
/// environment variable.
pub fn usagecheck_app_data_dir() -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        if let Ok(raw) = std::env::var("USAGECHECK_APP_DATA_DIR") {
            if !raw.trim().is_empty() {
                return Some(PathBuf::from(raw));
            }
        }
    }
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

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

fn json_files_in(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().and_then(|ext| ext.to_str()) == Some("json") && path.is_file()
        })
        .collect();
    files.sort();
    files
}

fn json_files_preferring(dir: &Path, preferred: &str) -> Vec<PathBuf> {
    let mut files = json_files_in(dir);
    files.sort_by(|a, b| {
        let a_pref = a.file_name().and_then(|n| n.to_str()) == Some(preferred);
        let b_pref = b.file_name().and_then(|n| n.to_str()) == Some(preferred);
        b_pref.cmp(&a_pref).then_with(|| a.cmp(b))
    });
    files
}

/// Kimi Code credential JSON files, first usable wins:
/// `$KIMI_CODE_HOME/credentials/*.json`, then `~/.kimi-code/credentials/*.json`
/// (kimi-code.json first), then `~/.kimi/credentials/kimi-code.json`.
pub fn kimi_credential_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(home) = env_path("KIMI_CODE_HOME") {
        files.extend(json_files_in(&home.join("credentials")));
    }
    if let Some(home) = home_dir() {
        files.extend(json_files_preferring(
            &home.join(".kimi-code").join("credentials"),
            "kimi-code.json",
        ));
        files.push(
            home.join(".kimi")
                .join("credentials")
                .join("kimi-code.json"),
        );
    }
    let mut seen = HashSet::new();
    files
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

/// OpenCode `auth.json`: `$OPENCODE_DATA_DIR`, else `$XDG_DATA_HOME/opencode`,
/// else `~/.local/share/opencode` (including Windows `%USERPROFILE%`).
pub fn opencode_auth_file() -> Option<PathBuf> {
    if let Some(dir) = env_path("OPENCODE_DATA_DIR") {
        return Some(dir.join("auth.json"));
    }
    if let Some(xdg) = env_path("XDG_DATA_HOME") {
        return Some(xdg.join("opencode").join("auth.json"));
    }
    home_dir().map(|h| {
        h.join(".local")
            .join("share")
            .join("opencode")
            .join("auth.json")
    })
}

/// DeepSeek Harness home: `$DSH_HOME` or `~/.dsh`.
pub fn dsh_home() -> Option<PathBuf> {
    env_path("DSH_HOME").or_else(|| home_dir().map(|h| h.join(".dsh")))
}

/// Ori home: `$ORI_HOME` or `~/.ori`.
pub fn ori_home() -> Option<PathBuf> {
    env_path("ORI_HOME").or_else(|| home_dir().map(|h| h.join(".ori")))
}

/// Ori `config.json` path.
pub fn ori_config_file() -> Option<PathBuf> {
    ori_home().map(|home| home.join("config.json"))
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

/// Claude keychain service name for an EXPLICIT config dir (per-profile), matching
/// Claude Code CLI: `Claude Code-credentials-{sha256(dir)[0..8]}`.
pub fn claude_keychain_service_name_for(config_dir: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};

    let hash = Sha256::digest(config_dir.to_string_lossy().as_bytes());
    let short = hex_prefix(hash, 8);
    format!("Claude Code-credentials-{short}")
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

fn xdg_config_dir() -> Option<PathBuf> {
    env_path("XDG_CONFIG_HOME").or_else(|| home_dir().map(|h| h.join(".config")))
}

fn github_copilot_token_files_from(
    xdg: Option<PathBuf>,
    localappdata: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for root in [xdg, localappdata].into_iter().flatten() {
        let dir = root.join("github-copilot");
        files.push(dir.join("apps.json"));
        files.push(dir.join("hosts.json"));
    }
    files
}

/// GitHub Copilot token JSON files (`apps.json` then `hosts.json`).
/// Windows also tries `%LOCALAPPDATA%/github-copilot/` after XDG/`~/.config`.
pub fn github_copilot_token_files() -> Vec<PathBuf> {
    github_copilot_token_files_from(
        xdg_config_dir(),
        {
            #[cfg(target_os = "windows")]
            {
                env_path("LOCALAPPDATA")
            }
            #[cfg(not(target_os = "windows"))]
            {
                None
            }
        },
    )
}

/// `gh` `hosts.yml` candidates (`oauth_token` / `token` under github.com).
pub fn gh_hosts_yml_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(config) = xdg_config_dir() {
        files.push(config.join("gh").join("hosts.yml"));
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = env_path("APPDATA") {
            files.push(appdata.join("GitHub CLI").join("hosts.yml"));
        }
    }
    files
}

fn editor_state_vscdb(app_name: &str) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        home_dir().map(|h| {
            h.join("Library")
                .join("Application Support")
                .join(app_name)
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
                h.join(app_name)
                    .join("User")
                    .join("globalStorage")
                    .join("state.vscdb")
            });
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        return home_dir().map(|h| {
            h.join(".config")
                .join(app_name)
                .join("User")
                .join("globalStorage")
                .join("state.vscdb")
        });
    }
}

/// Cursor `state.vscdb` (read-only) under globalStorage.
pub fn cursor_state_vscdb() -> Option<PathBuf> {
    editor_state_vscdb("Cursor")
}

/// Windsurf `state.vscdb` (read-only) under globalStorage.
pub fn windsurf_state_vscdb() -> Option<PathBuf> {
    editor_state_vscdb("Windsurf")
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
    fn app_data_dir_env_override_wins_when_set() {
        // `USAGECHECK_APP_DATA_DIR` is a process-wide env var ALSO mutated by
        // `license/http_tests.rs`, `license/status_tests.rs`, and
        // `menu_actions_tests.rs` — a file-local lock here would not prevent
        // this test's mutation from racing theirs under `cargo test`'s
        // default parallel execution, so this must take the ONE crate-wide
        // lock instead (see `crate::license::LICENSE_ENV_LOCK`'s doc comment).
        let _lock = crate::license::LICENSE_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let previous = std::env::var_os("USAGECHECK_APP_DATA_DIR");

        std::env::set_var("USAGECHECK_APP_DATA_DIR", "/tmp/usagecheck-test-override");
        assert_eq!(
            usagecheck_app_data_dir(),
            Some(PathBuf::from("/tmp/usagecheck-test-override"))
        );

        std::env::set_var("USAGECHECK_APP_DATA_DIR", "   ");
        assert_ne!(
            usagecheck_app_data_dir(),
            Some(PathBuf::from("   ")),
            "a blank override must fall back to the platform-derived path"
        );

        match previous {
            Some(p) => std::env::set_var("USAGECHECK_APP_DATA_DIR", p),
            None => std::env::remove_var("USAGECHECK_APP_DATA_DIR"),
        }
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

    #[test]
    fn copilot_token_files_prefer_apps_then_windows_localappdata() {
        let files = github_copilot_token_files_from(
            Some(PathBuf::from("/xdg")),
            Some(PathBuf::from("/local")),
        );
        assert_eq!(
            files,
            vec![
                PathBuf::from("/xdg/github-copilot/apps.json"),
                PathBuf::from("/xdg/github-copilot/hosts.json"),
                PathBuf::from("/local/github-copilot/apps.json"),
                PathBuf::from("/local/github-copilot/hosts.json"),
            ]
        );
    }

    #[test]
    fn copilot_and_windsurf_paths_are_stable() {
        let files = github_copilot_token_files();
        assert!(
            files
                .iter()
                .any(|p| p.file_name().and_then(|n| n.to_str()) == Some("apps.json")),
            "expected apps.json in {files:?}"
        );
        let hosts = gh_hosts_yml_files();
        assert!(
            hosts
                .iter()
                .any(|p| p.file_name().and_then(|n| n.to_str()) == Some("hosts.yml")),
            "expected hosts.yml in {hosts:?}"
        );
        if let Some(p) = windsurf_state_vscdb() {
            assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("state.vscdb"));
            assert!(p.to_string_lossy().contains("Windsurf"));
        }
    }

    #[test]
    fn claude_keychain_service_name_for_is_deterministic() {
        let path = Path::new("/tmp/x");
        let name = claude_keychain_service_name_for(path);
        assert_eq!(name, claude_keychain_service_name_for(path));
        let suffix = name.strip_prefix("Claude Code-credentials-").unwrap();
        assert_eq!(suffix.len(), 8);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
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
