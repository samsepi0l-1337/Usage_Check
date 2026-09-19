//! Resolve provider CLI binaries from a PATH superset.
//!
//! A GUI process launched by launchd / Finder inherits a minimal PATH
//! (`/usr/bin:/bin:/usr/sbin:/sbin`) that omits Homebrew and user-local
//! dirs. Searching only `std::env::var("PATH")` then fails even when the
//! binary is installed.

use std::path::{Path, PathBuf};

/// Directories to search for a provider CLI, process PATH first, then
/// well-known install locations. Duplicates are possible; [`which_bin_in`]
/// returns the first existing file.
pub(crate) fn candidate_bin_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(path) = std::env::var("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    #[cfg(not(windows))]
    for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        dirs.push(PathBuf::from(extra));
    }
    #[cfg(windows)]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Programs"));
        }
        // npm global shims (`mmx.cmd`, `auggie.cmd`) live here even when the
        // GUI process PATH omits `%APPDATA%\npm`.
        if let Some(appdata) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
    }
    if let Some(home) = crate::paths::home_dir() {
        for sub in [
            ".local/bin",
            ".cargo/bin",
            ".claude/local",
            ".bun/bin",
            ".deno/bin",
            ".volta/bin",
            ".npm-global/bin",
            "bin",
        ] {
            dirs.push(home.join(sub));
        }
    }
    dirs
}

/// Filenames to try for `name` in one directory.
///
/// Windows npm CLIs install `.cmd` (sometimes `.bat`) shims, not `.exe`.
/// `Command::new("mmx")` would still find those via PATHEXT; this helper
/// returns a concrete path, so it must probe the same suffixes itself.
/// Order matches typical PATHEXT: `.exe`, `.cmd`, `.bat`, then the bare name.
fn executable_names_with(name: &str, windows: bool) -> Vec<String> {
    if !windows || Path::new(name).extension().is_some() {
        return vec![name.to_string()];
    }
    vec![
        format!("{name}.exe"),
        format!("{name}.cmd"),
        format!("{name}.bat"),
        name.to_string(),
    ]
}

fn executable_names(name: &str) -> Vec<String> {
    executable_names_with(name, cfg!(windows))
}

fn which_bin_in_named<I>(names: &[String], dirs: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    for dir in dirs {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Absolute path of `name` in `dirs`, or `None` if no entry is a file.
/// On Windows, probes `.exe` / `.cmd` / `.bat` / bare name when `name` has
/// no extension.
pub(crate) fn which_bin_in<I>(name: &str, dirs: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    which_bin_in_named(&executable_names(name), dirs)
}

/// Resolve `name` on the PATH superset from [`candidate_bin_dirs`].
pub(crate) fn which_bin(name: &str) -> Option<PathBuf> {
    which_bin_in(name, candidate_bin_dirs())
}

#[cfg(test)]
#[path = "cli_bin_tests.rs"]
mod tests;
