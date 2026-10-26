//! Guards against the `tauri` crate and the `@tauri-apps/api` npm package
//! drifting to different major.minor versions (the Tauri CLI flags mismatches).

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn major_minor(version: &str) -> (u64, u64) {
    let mut parts = version
        .split('.')
        .map(|p| p.parse::<u64>().expect("numeric"));
    (parts.next().expect("major"), parts.next().expect("minor"))
}

fn locked_tauri_crate_version() -> String {
    let lock = std::fs::read_to_string(repo_root().join("Cargo.lock")).expect("Cargo.lock");
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line == "name = \"tauri\"" {
            let version = lines.next().expect("version line");
            return version
                .strip_prefix("version = \"")
                .and_then(|v| v.strip_suffix('"'))
                .expect("version format")
                .to_string();
        }
    }
    panic!("tauri not found in Cargo.lock");
}

fn locked_tauri_api_npm_version() -> String {
    let raw = std::fs::read_to_string(repo_root().join("ui/package-lock.json"))
        .expect("ui/package-lock.json");
    let lock: serde_json::Value = serde_json::from_str(&raw).expect("valid json");
    lock["packages"]["node_modules/@tauri-apps/api"]["version"]
        .as_str()
        .expect("@tauri-apps/api in package-lock")
        .to_string()
}

#[test]
fn tauri_crate_and_npm_api_share_major_minor() {
    let krate = locked_tauri_crate_version();
    let npm = locked_tauri_api_npm_version();
    assert_eq!(
        major_minor(&krate),
        major_minor(&npm),
        "tauri crate {krate} (Cargo.lock) and @tauri-apps/api {npm} (ui/package-lock.json) must share major.minor"
    );
}
