//! Hermetic-HOME guard for the test binary.
//!
//! A test must never resolve, read or write the developer's real `HOME`
//! (`~/.claude/settings.json` was overwritten with a cargo test binary path
//! this way). Test builds therefore refuse any `HOME` / `CLAUDE_CONFIG_DIR`
//! outside the OS temp directory and fall back to a process-wide sandbox.
//! The helpers below are the one shared way for tests to point `HOME` at a
//! tempdir; they also serialise the mutation with a single crate-wide lock.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Serialises every test that mutates `HOME` / `CLAUDE_CONFIG_DIR`.
/// Lock order: `LICENSE_ENV_LOCK` first, then this one.
pub(crate) static HOME_ENV_LOCK: Mutex<()> = Mutex::new(());

/// Sets `HOME` to `path` and clears `CLAUDE_CONFIG_DIR` until dropped.
pub(crate) struct HomeGuard {
    home: Option<OsString>,
    config_dir: Option<OsString>,
}

impl HomeGuard {
    pub(crate) fn set(path: &Path) -> Self {
        let guard = Self {
            home: std::env::var_os("HOME"),
            config_dir: std::env::var_os("CLAUDE_CONFIG_DIR"),
        };
        std::env::set_var("HOME", path);
        std::env::remove_var("CLAUDE_CONFIG_DIR");
        guard
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.home.take() {
            Some(previous) => std::env::set_var("HOME", previous),
            None => std::env::remove_var("HOME"),
        }
        match self.config_dir.take() {
            Some(previous) => std::env::set_var("CLAUDE_CONFIG_DIR", previous),
            None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
        }
    }
}

fn is_in_temp_dir(path: &Path) -> bool {
    let temp = std::env::temp_dir();
    let canonical_temp = temp.canonicalize().unwrap_or_else(|_| temp.clone());
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    path.starts_with(&temp) || canonical.starts_with(&canonical_temp)
}

/// A directory that exists, is outside the OS temp dir, and is safe to point
/// `HOME` at because nothing writes into the source tree: the crate manifest dir.
fn non_temp_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    assert!(!is_in_temp_dir(&dir), "manifest dir must not be a temp dir");
    dir
}

#[test]
fn home_dir_refuses_a_home_outside_the_temp_dir() {
    let _lock = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _home = HomeGuard::set(&non_temp_dir());

    let resolved = crate::paths::home_dir().expect("test home");

    assert!(
        is_in_temp_dir(&resolved),
        "test build resolved a non-temp HOME: {}",
        resolved.display()
    );
}

#[test]
fn home_dir_keeps_a_temp_home_unchanged() {
    let _lock = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let temp = tempfile::tempdir().expect("temp home");
    let _home = HomeGuard::set(temp.path());

    assert_eq!(crate::paths::home_dir().as_deref(), Some(temp.path()));
}

#[test]
fn claude_config_dir_outside_the_temp_dir_is_ignored() {
    let _lock = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let temp = tempfile::tempdir().expect("temp home");
    let _home = HomeGuard::set(temp.path());
    let outside = non_temp_dir();
    std::env::set_var("CLAUDE_CONFIG_DIR", &outside);

    let roots = crate::paths::claude_config_roots();
    let default = crate::paths::default_claude_config_dir_checked();

    assert!(
        roots.iter().all(|root| is_in_temp_dir(root)),
        "claude_config_roots leaked {roots:?}"
    );
    assert!(
        default.as_deref().is_none_or(is_in_temp_dir),
        "default_claude_config_dir_checked leaked {default:?}"
    );
}
