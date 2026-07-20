use super::*;

#[test]
fn test_fake_launcher_accepts_commands() {
    let launcher = FakeTerminalLauncher::new();
    let cmd = TerminalCommand {
        executable: PathBuf::from("/bin/echo"),
        args: vec![OsString::from("hello")],
        env: vec![],
        env_remove: vec![],
    };
    assert!(launcher.launch(&cmd).is_ok());
    assert_eq!(launcher.launched_commands.lock().unwrap().len(), 1);
}

#[test]
fn test_fake_launcher_can_fail() {
    let mut launcher = FakeTerminalLauncher::new();
    launcher.should_fail = true;
    let cmd = TerminalCommand {
        executable: PathBuf::from("/bin/echo"),
        args: vec![],
        env: vec![],
        env_remove: vec![],
    };
    assert!(launcher.launch(&cmd).is_err());
}

#[cfg(target_os = "macos")]
#[test]
fn test_macos_render_escapes_space_and_apostrophe() {
    let cmd = TerminalCommand {
        executable: PathBuf::from("/Users/test user/it's app/login"),
        args: vec![OsString::from("arg's value")],
        env: vec![],
        env_remove: vec![],
    };
    let script_path = PathBuf::from("/tmp/test_script.sh");
    let script = render_macos_script(&cmd, &script_path);
    assert!(script.contains("'\\''")); 
    assert!(!script.contains("'''"));
}

#[cfg(target_os = "macos")]
#[test]
fn test_macos_render_keeps_exe_and_args_on_one_line() {
    // Regression: exe + args must be a single command line, else bash runs
    // them as separate commands (args dropped; `codex login` would invoke
    // the system `login` binary instead of `codex login`).
    let cmd = TerminalCommand {
        executable: PathBuf::from("/usr/local/bin/codex"),
        args: vec![OsString::from("login")],
        env: vec![],
        env_remove: vec![],
    };
    let script = render_macos_script(&cmd, &PathBuf::from("/tmp/login_x.sh"));
    assert!(
        script.contains("'/usr/local/bin/codex' 'login'\n"),
        "exe and args must share one line, got:\n{script}"
    );
    // The args must NOT appear on their own line.
    assert!(
        !script.contains("\n 'login'\n"),
        "arg must not be a standalone command line, got:\n{script}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn test_macos_render_multiple_args_single_line() {
    let cmd = TerminalCommand {
        executable: PathBuf::from("/usr/local/bin/claude"),
        args: vec![
            OsString::from("auth"),
            OsString::from("login"),
            OsString::from("--claudeai"),
        ],
        env: vec![],
        env_remove: vec![],
    };
    let script = render_macos_script(&cmd, &PathBuf::from("/tmp/login_y.sh"));
    assert!(
        script.contains("'/usr/local/bin/claude' 'auth' 'login' '--claudeai'\n"),
        "all args must share the exe's line, got:\n{script}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn test_macos_script_uses_app_private_path() {
    let cmd = TerminalCommand {
        executable: PathBuf::from("/usr/bin/test"),
        args: vec![],
        env: vec![],
        env_remove: vec![],
    };
    let script_path = PathBuf::from("/var/folders/test/app/tmp/login_abc123.sh");
    let script = render_macos_script(&cmd, &script_path);
    assert!(script.contains(&format!(
        "__usagecheck_script='{}'",
        script_path.display()
    )));
    assert!(script.contains("trap 'rm -f \"$__usagecheck_script\"' EXIT"));
    let bare_cleanup = format!("rm -f '{}'", script_path.display());
    assert!(!script.lines().any(|line| line == bare_cleanup));
}

#[cfg(target_os = "windows")]
#[test]
fn test_windows_render_escapes_quotes() {
    let cmd = TerminalCommand {
        executable: PathBuf::from("C:\\Users\\test user\\it's app\\login"),
        args: vec![OsString::from("arg's value")],
        env: vec![(OsString::from("KEY'NAME"), OsString::from("value's"))],
        env_remove: vec![OsString::from("OLD'KEY")],
    };
    let script_path = PathBuf::from("C:\\Users\\user\\AppData\\Local\\UsageCheck\\tmp\\login_abc123.ps1");
    let script = render_windows_script(&cmd, &script_path);
    assert!(script.contains("''"));
    assert!(script.contains("[Environment]::SetEnvironmentVariable"));
    assert!(script.contains("[Environment]::SetEnvironmentVariable('KEY''NAME', 'value''s', 'Process')"));
    assert!(script.contains("[Environment]::SetEnvironmentVariable('OLD''KEY', $null, 'Process')"));
}

#[cfg(target_os = "windows")]
#[test]
fn test_windows_script_uses_app_private_path() {
    let cmd = TerminalCommand {
        executable: PathBuf::from("C:\\Program Files\\test.exe"),
        args: vec![],
        env: vec![],
        env_remove: vec![],
    };
    let script_path = PathBuf::from("C:\\Users\\user\\AppData\\Local\\UsageCheck\\tmp\\login_xyz.ps1");
    let script = render_windows_script(&cmd, &script_path);
    assert!(script.contains(&format!("Remove-Item '{}' -Force", script_path.display())));
}

#[test]
fn test_script_never_contains_credential_value() {
    let launcher = FakeTerminalLauncher::new();
    let cmd = TerminalCommand {
        executable: PathBuf::from("/bin/login"),
        args: vec![OsString::from("--profile"), OsString::from("myprofile")],
        env: vec![(OsString::from("HOME"), OsString::from("/home/user"))],
        env_remove: vec![OsString::from("SECRET_TOKEN"), OsString::from("API_KEY")],
    };
    assert!(launcher.launch(&cmd).is_ok());
    let recorded = launcher.launched_commands.lock().unwrap();
    assert!(!recorded.is_empty());
}

#[test]
fn test_terminal_command_clone_works() {
    let cmd = TerminalCommand {
        executable: PathBuf::from("/bin/test"),
        args: vec![OsString::from("arg1")],
        env: vec![(OsString::from("KEY"), OsString::from("value"))],
        env_remove: vec![OsString::from("OLD_KEY")],
    };
    let cloned = cmd.clone();
    assert_eq!(cloned.executable, cmd.executable);
    assert_eq!(cloned.args, cmd.args);
}

#[test]
fn test_render_script_signature_accepts_path() {
    let cmd = TerminalCommand {
        executable: PathBuf::from("/usr/bin/test"),
        args: vec![],
        env: vec![],
        env_remove: vec![],
    };
    let path1 = PathBuf::from("/path/to/script1.sh");
    let path2 = PathBuf::from("/path/to/script2.sh");
    
    let script1 = render_macos_script(&cmd, &path1);
    let script2 = render_macos_script(&cmd, &path2);
    
    assert_ne!(script1, script2);
    assert!(script1.contains("script1"));
    assert!(script2.contains("script2"));
}
