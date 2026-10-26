//! Test-build confinement of the process home (issue #63). Compiled only for
//! `cargo test`: a `HOME` or `CLAUDE_CONFIG_DIR` outside the OS temp directory
//! can never be resolved, so no test can write to the developer's real home.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub(super) fn is_in_temp_dir(path: &Path) -> bool {
    let temp = std::env::temp_dir();
    let canonical_temp = temp.canonicalize().unwrap_or_else(|_| temp.clone());
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    path.starts_with(&temp) || canonical.starts_with(&canonical_temp)
}

/// `raw` when it is inside the temp dir, else a process-wide sandbox home.
pub(super) fn confine_home(raw: Option<PathBuf>) -> PathBuf {
    match raw {
        Some(path) if is_in_temp_dir(&path) => path,
        _ => sandbox_home().to_path_buf(),
    }
}

fn sandbox_home() -> &'static Path {
    static SANDBOX: OnceLock<PathBuf> = OnceLock::new();
    SANDBOX.get_or_init(|| {
        std::env::temp_dir().join(format!("usagecheck-test-home-{}", std::process::id()))
    })
}
