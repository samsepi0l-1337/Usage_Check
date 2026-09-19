use super::*;
use std::fs;

#[test]
fn which_bin_in_finds_a_file_in_the_first_matching_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();
    let filename = if cfg!(windows) { "mmx.exe" } else { "mmx" };
    let exe = bin_dir.join(filename);
    fs::write(&exe, b"").unwrap();

    let empty = tmp.path().join("empty");
    fs::create_dir(&empty).unwrap();

    let found = which_bin_in("mmx", [empty, bin_dir.clone()]);
    assert_eq!(found.as_deref(), Some(exe.as_path()));
}

#[test]
fn which_bin_in_skips_directories_named_like_the_bin() {
    let tmp = tempfile::tempdir().unwrap();
    let decoy = tmp.path().join("mmx");
    fs::create_dir(&decoy).unwrap();
    assert!(which_bin_in("mmx", [tmp.path().to_path_buf()]).is_none());
}

#[test]
fn which_bin_in_finds_windows_cmd_shim() {
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();
    let cmd = bin_dir.join("mmx.cmd");
    fs::write(&cmd, b"").unwrap();

    let found = which_bin_in_named(&executable_names_with("mmx", true), [bin_dir]);
    assert_eq!(found.as_deref(), Some(cmd.as_path()));
}

#[test]
fn which_bin_in_prefers_exe_over_cmd_on_windows() {
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();
    let exe = bin_dir.join("auggie.exe");
    let cmd = bin_dir.join("auggie.cmd");
    fs::write(&exe, b"").unwrap();
    fs::write(&cmd, b"").unwrap();

    let found = which_bin_in_named(&executable_names_with("auggie", true), [bin_dir]);
    assert_eq!(found.as_deref(), Some(exe.as_path()));
}

#[test]
fn executable_names_with_windows_lists_pathext_order() {
    assert_eq!(
        executable_names_with("mmx", true),
        ["mmx.exe", "mmx.cmd", "mmx.bat", "mmx"]
    );
    assert_eq!(executable_names_with("mmx.cmd", true), ["mmx.cmd"]);
    assert_eq!(executable_names_with("mmx", false), ["mmx"]);
}
