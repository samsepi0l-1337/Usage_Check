use serde_json::{json, Value};
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

const BRIDGE_FLAG: &str = "--claude-statusline-bridge";
const PRIOR_SIDECAR: &str = ".statusline_prior.json";

pub(crate) fn validate_account_id(account_id: &str) -> Result<(), String> {
    if !account_id.is_empty()
        && account_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        Ok(())
    } else {
        Err("account id must match ^[A-Za-z0-9._-]+$".to_string())
    }
}

fn statusline_command(value: &Value) -> Option<&str> {
    value
        .get("command")
        .and_then(Value::as_str)
        .or_else(|| value.as_str())
}

fn is_usagecheck_bridge(value: &Value) -> bool {
    statusline_command(value).is_some_and(|command| command.contains(BRIDGE_FLAG))
}

fn sidecar_path(settings_path: &Path) -> Result<std::path::PathBuf, String> {
    settings_path
        .parent()
        .map(|parent| parent.join(PRIOR_SIDECAR))
        .ok_or_else(|| "settings path has no parent".to_string())
}

fn prior_command(settings_path: &Path) -> Option<String> {
    let body = fs::read_to_string(sidecar_path(settings_path).ok()?).ok()?;
    let sidecar: Value = serde_json::from_str(&body).ok()?;
    statusline_command(sidecar.get("prior_command")?).map(str::to_owned)
}

fn write_snapshot(account_id: &str, snapshot: &Value) -> Result<(), String> {
    let snapshot_path = crate::paths::claude_statusline_snapshot(account_id);
    let parent = snapshot_path
        .parent()
        .ok_or_else(|| "snapshot path has no parent".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create snapshot directory: {error}"))?;
    let temp_path = snapshot_path.with_extension("tmp");
    let body = serde_json::to_vec(snapshot)
        .map_err(|error| format!("failed to serialize snapshot: {error}"))?;
    fs::write(&temp_path, body)
        .map_err(|error| format!("failed to write temporary snapshot: {error}"))?;

    #[cfg(windows)]
    if snapshot_path.exists() {
        fs::remove_file(&snapshot_path)
            .map_err(|error| format!("failed to replace snapshot: {error}"))?;
    }

    fs::rename(&temp_path, &snapshot_path)
        .map_err(|error| format!("failed to publish snapshot: {error}"))
}

fn run_prior(command: &str, stdin_data: &[u8]) -> Result<Vec<u8>, String> {
    let mut shell = if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.arg("/C");
        command
    } else {
        let mut command = Command::new("sh");
        command.arg("-c");
        command
    };
    let mut child = shell
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to spawn prior status-line command: {error}"))?;
    let mut child_stdin = child
        .stdin
        .take()
        .ok_or_else(|| "prior status-line command has no stdin".to_string())?;
    child_stdin
        .write_all(stdin_data)
        .map_err(|error| format!("failed to forward status-line stdin: {error}"))?;
    drop(child_stdin);
    let output = child
        .wait_with_output()
        .map_err(|error| format!("failed to wait for prior status-line command: {error}"))?;
    Ok(output.stdout)
}

/// Translate Claude's status-line rate-limit window
/// (`{ "used_percentage": <f64>, "resets_at": <epoch> }`) into the app's canonical
/// shape consumed by `usage_core::fetch::claude::parse_claude_usage`
/// (`{ "utilization": <f64>, "resets_at": <epoch|rfc3339> }`). `resets_at` passes
/// through unchanged (the parser already accepts an epoch number).
fn normalize_rate_window(window: &Value) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(percent) = window.get("used_percentage").and_then(Value::as_f64) {
        out.insert("utilization".to_string(), json!(percent));
    }
    if let Some(resets_at) = window.get("resets_at") {
        out.insert("resets_at".to_string(), resets_at.clone());
    }
    Value::Object(out)
}

pub fn run_bridge<R: Read, W: Write>(
    account_id: &str,
    identity: &str,
    profile_settings_path: &Path,
    mut stdin: R,
    mut stdout: W,
) -> Result<(), String> {
    validate_account_id(account_id)?;
    let mut stdin_data = Vec::new();
    stdin
        .read_to_end(&mut stdin_data)
        .map_err(|error| format!("failed to read status-line stdin: {error}"))?;
    // Claude Code's status-line JSON has no user-identity field, and `rate_limits`
    // is absent until the first API response of a Pro/Max session — both are optional
    // here. Identity comes from the caller (the stored account label); a missing or
    // partial rate_limits simply yields an empty snapshot ("waiting for usage").
    let status: Value = serde_json::from_slice(&stdin_data)
        .map_err(|error| format!("failed to parse status-line JSON: {error}"))?;
    let rate_limits = status.get("rate_limits").and_then(Value::as_object);
    let five_hour = rate_limits
        .and_then(|limits| limits.get("five_hour"))
        .map(normalize_rate_window)
        .unwrap_or(Value::Null);
    let seven_day = rate_limits
        .and_then(|limits| limits.get("seven_day"))
        .map(normalize_rate_window)
        .unwrap_or(Value::Null);
    let snapshot = json!({
        "identity": identity,
        "rate_limits": { "five_hour": five_hour, "seven_day": seven_day }
    });
    write_snapshot(account_id, &snapshot)?;

    if let Some(command) = prior_command(profile_settings_path) {
        stdout
            .write_all(&run_prior(&command, &stdin_data)?)
            .map_err(|error| format!("failed to write prior status-line stdout: {error}"))?;
    } else {
        writeln!(stdout, "{identity} · Usage check ready")
            .map_err(|error| format!("failed to write status-line fallback: {error}"))?;
    }
    stdout
        .flush()
        .map_err(|error| format!("failed to flush status-line stdout: {error}"))
}

pub fn handle_statusline_bridge(
    account_id: &str,
    profile_settings_path: &Path,
) -> Result<(), String> {
    // Claude does not pass user identity to the status line; recover it from the
    // stored account — its label is the identity the poller compares against.
    let identity = crate::store::AccountStore::new()
        .account(account_id)
        .map(|account| account.label)
        .unwrap_or_default();
    let stdin = io::stdin();
    let stdout = io::stdout();
    run_bridge(
        account_id,
        &identity,
        profile_settings_path,
        stdin.lock(),
        stdout.lock(),
    )
}

pub fn install_statusline_bridge(
    profile_settings_path: &Path,
    account_id: &str,
) -> Result<(), String> {
    validate_account_id(account_id)?;
    let mut settings = if profile_settings_path.exists() {
        let body = fs::read_to_string(profile_settings_path)
            .map_err(|error| format!("failed to read Claude settings: {error}"))?;
        serde_json::from_str::<Value>(&body).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    let parent = profile_settings_path
        .parent()
        .ok_or_else(|| "settings path has no parent".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create Claude profile directory: {error}"))?;
    if let Some(prior) = settings
        .get("statusLine")
        .filter(|value| !is_usagecheck_bridge(value))
    {
        let sidecar = json!({"prior_command": prior});
        fs::write(sidecar_path(profile_settings_path)?, sidecar.to_string())
            .map_err(|error| format!("failed to preserve prior status line: {error}"))?;
    }
    let exe = std::env::current_exe()
        .ok()
        .and_then(|path| path.to_str().map(str::to_string))
        .unwrap_or_else(|| "usage-app".to_string());
    settings["statusLine"] = json!({
        "type": "command",
        "command": format!("\"{}\" {BRIDGE_FLAG} {account_id}", exe.replace('"', "")),
    });
    let body = serde_json::to_string_pretty(&settings)
        .map_err(|error| format!("failed to serialize Claude settings: {error}"))?;
    fs::write(profile_settings_path, body)
        .map_err(|error| format!("failed to write Claude settings: {error}"))
}

pub fn remove_statusline_bridge(profile_settings_path: &Path) -> Result<(), String> {
    if !profile_settings_path.exists() {
        return Ok(());
    }
    let body = fs::read_to_string(profile_settings_path)
        .map_err(|error| format!("failed to read Claude settings: {error}"))?;
    let mut settings: Value = serde_json::from_str(&body)
        .map_err(|error| format!("failed to parse Claude settings: {error}"))?;
    let Some(current) = settings.get("statusLine") else {
        return Ok(());
    };
    if !is_usagecheck_bridge(current) {
        return Ok(());
    }

    let sidecar_path = sidecar_path(profile_settings_path)?;
    let prior = fs::read_to_string(&sidecar_path)
        .ok()
        .and_then(|body| serde_json::from_str::<Value>(&body).ok())
        .and_then(|sidecar| sidecar.get("prior_command").cloned());
    match prior {
        Some(value) => settings["statusLine"] = value,
        None => {
            settings
                .as_object_mut()
                .ok_or_else(|| "Claude settings root must be an object".to_string())?
                .remove("statusLine");
        }
    }
    if sidecar_path.exists() {
        fs::remove_file(&sidecar_path)
            .map_err(|error| format!("failed to remove status-line sidecar: {error}"))?;
    }
    let body = serde_json::to_string_pretty(&settings)
        .map_err(|error| format!("failed to serialize Claude settings: {error}"))?;
    fs::write(profile_settings_path, body)
        .map_err(|error| format!("failed to write Claude settings: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct HomeGuard(Option<OsString>);

    impl HomeGuard {
        fn set(path: &Path) -> Self {
            let previous = std::env::var_os("HOME");
            std::env::set_var("HOME", path);
            Self(previous)
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(previous) => std::env::set_var("HOME", previous),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    fn read_json(path: &Path) -> Value {
        let body = fs::read_to_string(path).expect("test JSON should be readable");
        serde_json::from_str(&body).expect("test JSON should parse")
    }

    #[test]
    fn install_and_remove_use_object_form_and_preserve_complete_prior() {
        let temp = TempDir::new().expect("temp directory");
        let settings_path = temp.path().join("settings.json");
        let prior = json!({"type": "command", "command": "old-command", "padding": 3});
        fs::write(&settings_path, json!({"statusLine": prior}).to_string())
            .expect("write settings");

        install_statusline_bridge(&settings_path, "test-account").expect("install bridge");

        let installed = read_json(&settings_path);
        assert_eq!(installed["statusLine"]["type"], "command");
        let command = installed["statusLine"]["command"].as_str().expect("command string");
        assert!(command.contains("--claude-statusline-bridge test-account"), "{command}");
        assert!(command.contains("usage-app") || command.contains("usage_app"), "{command}");
        assert_eq!(
            read_json(&temp.path().join(".statusline_prior.json"))["prior_command"],
            prior
        );

        remove_statusline_bridge(&settings_path).expect("remove bridge");
        assert_eq!(read_json(&settings_path)["statusLine"], prior);
    }

    #[test]
    fn remove_preserves_later_user_edit() {
        let temp = TempDir::new().expect("temp directory");
        let settings_path = temp.path().join("settings.json");
        fs::write(
            &settings_path,
            json!({"statusLine": "old-command"}).to_string(),
        )
        .expect("write settings");
        install_statusline_bridge(&settings_path, "test-account").expect("install bridge");
        let mut settings = read_json(&settings_path);
        settings["statusLine"] = json!({"type": "command", "command": "user-edit"});
        fs::write(&settings_path, settings.to_string()).expect("update settings");

        remove_statusline_bridge(&settings_path).expect("remove is no-op");

        assert_eq!(
            read_json(&settings_path)["statusLine"]["command"],
            "user-edit"
        );
    }

    #[test]
    fn account_id_rejection_blocks_shell_metacharacters() {
        let temp = TempDir::new().expect("temp directory");
        let settings_path = temp.path().join("settings.json");
        fs::write(&settings_path, "{}").expect("write settings");
        for invalid in ["bad;id", "bad id", "$(bad)"] {
            assert!(
                validate_account_id(invalid).is_err(),
                "accepted {invalid:?}"
            );
            assert!(
                install_statusline_bridge(&settings_path, invalid).is_err(),
                "installed {invalid:?}"
            );
        }
        assert_eq!(read_json(&settings_path), json!({}));
    }

    #[test]
    fn runtime_bridge_preserves_bytes_and_writes_minimal_snapshot() {
        let _lock = ENV_LOCK.lock().expect("environment lock");
        let temp = TempDir::new().expect("temp directory");
        let _home = HomeGuard::set(temp.path());
        let account_id = "runtime-test";
        let settings_path = crate::paths::claude_settings_json(account_id);
        fs::create_dir_all(settings_path.parent().expect("settings parent"))
            .expect("create profile");
        #[cfg(unix)]
        let prior_command = "cat";
        #[cfg(windows)]
        let prior_command = "more";
        fs::write(
            &settings_path,
            json!({"statusLine": {"type": "command", "command": prior_command}}).to_string(),
        )
        .expect("write settings");
        install_statusline_bridge(&settings_path, account_id).expect("install bridge");
        let input = br#"{"model":{"id":"claude-opus-4-8"},"rate_limits":{"five_hour":{"used_percentage":23.5,"resets_at":1738425600},"seven_day":{"used_percentage":41.2,"resets_at":1738857600}}}"#;
        let mut output = Vec::new();
        run_bridge(account_id, "user@example.com", &settings_path, &input[..], &mut output).expect("run bridge");
        assert_eq!(output, input); // prior "cat" echoes stdin through
        let snap = read_json(&crate::paths::claude_statusline_snapshot(account_id));
        assert_eq!(snap["identity"], "user@example.com");
        assert_eq!(snap["rate_limits"]["five_hour"]["utilization"], 23.5);
        assert_eq!(snap["rate_limits"]["five_hour"]["resets_at"], 1738425600);
        let quota = usage_core::fetch::claude::parse_claude_usage(&snap["rate_limits"]);
        assert!(quota.five_hour.is_some() && quota.week.is_some());
        assert_eq!(quota.five_hour.unwrap().percent, 23.5);
    }

    #[test]
    fn runtime_bridge_emits_fallback_without_prior_command() {
        let _lock = ENV_LOCK.lock().expect("environment lock");
        let temp = TempDir::new().expect("temp directory");
        let _home = HomeGuard::set(temp.path());
        let account_id = "fallback-test";
        let settings_path = crate::paths::claude_settings_json(account_id);
        fs::create_dir_all(settings_path.parent().expect("settings parent"))
            .expect("create profile");
        fs::write(&settings_path, "{}").expect("write settings");
        let input = br#"{"model":{"id":"claude-opus-4-8"}}"#;
        let mut output = Vec::new();

        run_bridge(account_id, "ready@example.com", &settings_path, &input[..], &mut output).expect("run bridge");

        let rendered = String::from_utf8(output).expect("UTF-8 fallback");
        assert!(rendered.contains("ready@example.com"));
        assert!(rendered.contains("Usage check ready"));
    }

    #[test]
    fn runtime_bridge_writes_empty_snapshot_without_rate_limits() {
        let _lock = ENV_LOCK.lock().expect("environment lock");
        let temp = TempDir::new().expect("temp directory");
        let _home = HomeGuard::set(temp.path());
        let account_id = "norates";
        let settings_path = crate::paths::claude_settings_json(account_id);
        fs::create_dir_all(settings_path.parent().expect("parent")).expect("create profile");
        fs::write(&settings_path, "{}").expect("write settings");
        let input = br#"{"model":{"id":"claude-opus-4-8"},"cost":{"total_cost_usd":0.01}}"#;
        let mut output = Vec::new();
        run_bridge(account_id, "u@e.com", &settings_path, &input[..], &mut output).expect("run bridge");
        let snap = read_json(&crate::paths::claude_statusline_snapshot(account_id));
        let quota = usage_core::fetch::claude::parse_claude_usage(&snap["rate_limits"]);
        assert!(quota.five_hour.is_none() && quota.week.is_none());
    }
}
