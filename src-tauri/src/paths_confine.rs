//! Test-build confinement of the process home (issue #63). Compiled only for
//! `cargo test`: a `HOME` or `CLAUDE_CONFIG_DIR` outside the OS temp directory
//! can never be resolved, so no test can write to the developer's real home.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// True when `path` resolves inside the OS temp dir. Symlinks are resolved
/// through the deepest existing ancestor (macOS `/var` -> `/private/var`) and a
/// `..` component is never accepted, so `/tmp/../home/x` is outside.
pub(crate) fn is_in_temp_dir(path: &Path) -> bool {
    use std::path::Component;
    if path.components().any(|part| part == Component::ParentDir) {
        return false;
    }
    let temp = std::env::temp_dir();
    let canonical_temp = temp.canonicalize().unwrap_or(temp);
    let mut existing = path;
    let mut missing: Vec<&std::ffi::OsStr> = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                missing.push(name);
                existing = parent;
            }
            _ => return false,
        }
    }
    let Ok(mut resolved) = existing.canonicalize() else {
        return false;
    };
    resolved.extend(missing.iter().rev());
    resolved.starts_with(canonical_temp)
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
