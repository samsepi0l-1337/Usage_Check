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
