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
use std::sync::{Mutex, MutexGuard};

/// Serialises every test that mutates `HOME`.
static HOME_ENV_LOCK: Mutex<()> = Mutex::new(());

/// Sets `HOME` to `path` and clears `CLAUDE_CONFIG_DIR` until dropped. It holds
/// `HOME_ENV_LOCK` and `CLAUDE_CONFIG_DIR_ENV_LOCK` (the lock the import tests
/// use for the same variable) for its whole lifetime. Lock order:
/// `LICENSE_ENV_LOCK`, then the guard.
pub(crate) struct HomeGuard {
    home: Option<OsString>,
    config_dir: Option<OsString>,
    home_lock: Option<MutexGuard<'static, ()>>,
    config_lock: Option<MutexGuard<'static, ()>>,
    config_lock_was_clean: bool,
}

impl HomeGuard {
    pub(crate) fn set(path: &Path) -> Self {
        let home_lock = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let config_lock_was_clean = !crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK.is_poisoned();
        let config_lock = crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let guard = Self {
            home: std::env::var_os("HOME"),
            config_dir: std::env::var_os("CLAUDE_CONFIG_DIR"),
            home_lock: Some(home_lock),
            config_lock: Some(config_lock),
            config_lock_was_clean,
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
        drop(self.config_lock.take());
        drop(self.home_lock.take());
        // A failing test must not poison the import tests' shared lock.
        if self.config_lock_was_clean {
            crate::import::CLAUDE_CONFIG_DIR_ENV_LOCK.clear_poison();
        }
    }
}

use crate::paths::paths_confine::is_in_temp_dir;

/// A directory that exists and is outside the OS temp dir regardless of where
/// the checkout lives: the filesystem root. Nothing writes to it; the tests
/// only check how it is resolved.
fn non_temp_dir() -> PathBuf {
    let root = std::env::temp_dir()
        .ancestors()
        .last()
        .map(Path::to_path_buf)
        .expect("temp dir has a root");
    assert!(
        root.exists() && !is_in_temp_dir(&root),
        "filesystem root must be outside the temp dir"
    );
    root
}

#[test]
fn home_dir_refuses_a_home_outside_the_temp_dir() {
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
    let temp = tempfile::tempdir().expect("temp home");
    let _home = HomeGuard::set(temp.path());

    assert_eq!(crate::paths::home_dir().as_deref(), Some(temp.path()));
}

#[test]
fn claude_config_dir_outside_the_temp_dir_is_ignored() {
    let temp = tempfile::tempdir().expect("temp home");
    let _home = HomeGuard::set(temp.path());
    let outside = non_temp_dir();
    std::env::set_var("CLAUDE_CONFIG_DIR", &outside);

    let roots = crate::paths::claude_config_roots();
    let default = crate::paths::ProfileRoots::default().default_claude_config_dir_checked();

    assert!(
        roots.iter().all(|root| is_in_temp_dir(root)),
        "claude_config_roots leaked {roots:?}"
    );
    assert!(
        default.as_deref().is_none_or(is_in_temp_dir),
        "default_claude_config_dir_checked leaked {default:?}"
    );
}

#[test]
fn dotdot_and_symlink_escapes_from_the_temp_dir_are_outside() {
    let temp = tempfile::tempdir().expect("temp dir");
    assert!(is_in_temp_dir(&temp.path().join("not-created-yet/home")));
    assert!(!is_in_temp_dir(&temp.path().join("..").join("escape")));
    assert!(!is_in_temp_dir(&non_temp_dir()));
    #[cfg(unix)]
    {
        let link = temp.path().join("link-to-root");
        std::os::unix::fs::symlink(non_temp_dir(), &link).expect("symlink");
        assert!(!is_in_temp_dir(&link));
        assert!(!is_in_temp_dir(&link.join("child")));
    }
}
