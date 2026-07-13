use std::ffi::OsString;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct TerminalCommand {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub env_remove: Vec<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalError {
    LaunchFailed(String),
    IoError(String),
}

pub trait TerminalLauncher: Send + Sync {
    fn launch(&self, command: &TerminalCommand) -> Result<(), TerminalError>;
}

#[cfg(target_os = "macos")]
pub struct MacosTerminalLauncher;

#[cfg(target_os = "macos")]
fn render_macos_script(command: &TerminalCommand, script_path: &Path) -> String {
    let mut script = String::from("#!/bin/bash\nset -e\n");

    for var in &command.env_remove {
        let var_str = var.to_string_lossy();
        script.push_str(&format!("unset '{}'\n", var_str.replace("'", "'\\''")));
    }

    for (key, value) in &command.env {
        let key_str = key.to_string_lossy();
        let value_str = value.to_string_lossy();
        script.push_str(&format!(
            "export '{}'='{}'\n",
            key_str.replace("'", "'\\''"),
            value_str.replace("'", "'\\''")
        ));
    }

    let exe_str = command.executable.display().to_string().replace("'", "'\\''");
    script.push_str(&format!("'{}'\n", exe_str));

    for arg in &command.args {
        let arg_str = arg.to_string_lossy().replace("'", "'\\''");
        script.push_str(&format!(" '{}'\n", arg_str));
    }

    let rm_path = script_path.display().to_string().replace("'", "'\\''");
    script.push_str(&format!("rm -f '{}'\n", rm_path));

    script
}

#[cfg(target_os = "macos")]
impl TerminalLauncher for MacosTerminalLauncher {
    fn launch(&self, command: &TerminalCommand) -> Result<(), TerminalError> {
        use crate::paths::paths_profiles::login_script_dir;
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt;
        use std::process::Command;

        let script_dir = login_script_dir()
            .map_err(|e| TerminalError::IoError(e.to_string()))?;
        let script_path = script_dir.join(format!("login_{}.sh", Uuid::new_v4()));

        let script = render_macos_script(command, &script_path);

        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true).mode(0o700);
        let mut file = opts
            .open(&script_path)
            .map_err(|e| TerminalError::IoError(e.to_string()))?;

        use std::io::Write;
        file.write_all(script.as_bytes())
            .map_err(|e| TerminalError::IoError(e.to_string()))?;

        let script_path_escaped = script_path.display().to_string().replace("'", "'\\''");
        let osascript_cmd = format!(
            "tell application \"Terminal\" to do script \"'{}'\"",
            script_path_escaped
        );

        let output = Command::new("osascript")
            .arg("-e")
            .arg(&osascript_cmd)
            .output()
            .map_err(|e| TerminalError::LaunchFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TerminalError::LaunchFailed(stderr.to_string()));
        }

        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub struct WindowsTerminalLauncher;

#[cfg(target_os = "windows")]
fn render_windows_script(command: &TerminalCommand, script_path: &Path) -> String {
    let mut script = String::from("$ErrorActionPreference = 'Stop'\n");

    for var in &command.env_remove {
        let var_str = var.to_string_lossy();
        script.push_str(&format!("Remove-Item Env:'{}' -ErrorAction SilentlyContinue\n", var_str));
    }

    for (key, value) in &command.env {
        let key_str = key.to_string_lossy();
        let value_str = value.to_string_lossy();
        script.push_str(&format!(
            "$Env:{} = '{}'\n",
            key_str,
            value_str.replace("'", "''")
        ));
    }

    let exe_path = command.executable.display().to_string().replace("'", "''");
    script.push_str(&format!("& '{}' ", exe_path));

    for arg in &command.args {
        let arg_str = arg.to_string_lossy().replace("'", "''");
        script.push_str(&format!("'{}' ", arg_str));
    }
    script.push('\n');

    let rm_path = script_path.display().to_string().replace("'", "''");
    script.push_str(&format!("Remove-Item '{}' -Force\n", rm_path));

    script
}

#[cfg(target_os = "windows")]
impl TerminalLauncher for WindowsTerminalLauncher {
    fn launch(&self, command: &TerminalCommand) -> Result<(), TerminalError> {
        use crate::paths::paths_profiles::login_script_dir;
        use std::fs::OpenOptions;
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};

        let script_dir = login_script_dir()
            .map_err(|e| TerminalError::IoError(e.to_string()))?;
        let script_path = script_dir.join(format!("login_{}.ps1", Uuid::new_v4()));

        let script = render_windows_script(command, &script_path);

        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        let mut file = opts
            .open(&script_path)
            .map_err(|e| TerminalError::IoError(e.to_string()))?;

        use std::io::Write;
        file.write_all(script.as_bytes())
            .map_err(|e| TerminalError::IoError(e.to_string()))?;

        let output = Command::new("powershell.exe")
            .arg("-NoProfile")
            .arg("-File")
            .arg(&script_path)
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| TerminalError::LaunchFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(TerminalError::LaunchFailed("PowerShell script failed".into()));
        }

        Ok(())
    }
}

#[cfg(test)]
pub struct FakeTerminalLauncher {
    pub should_fail: bool,
    pub launched_commands: std::sync::Arc<std::sync::Mutex<Vec<TerminalCommand>>>,
}

#[cfg(test)]
impl FakeTerminalLauncher {
    pub fn new() -> Self {
        Self {
            should_fail: false,
            launched_commands: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }
}

#[cfg(test)]
impl TerminalLauncher for FakeTerminalLauncher {
    fn launch(&self, command: &TerminalCommand) -> Result<(), TerminalError> {
        if self.should_fail {
            return Err(TerminalError::LaunchFailed("Fake failure".into()));
        }
        self.launched_commands.lock().unwrap().push(command.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
    fn test_macos_script_uses_app_private_path() {
        let cmd = TerminalCommand {
            executable: PathBuf::from("/usr/bin/test"),
            args: vec![],
            env: vec![],
            env_remove: vec![],
        };
        let script_path = PathBuf::from("/var/folders/test/app/tmp/login_abc123.sh");
        let script = render_macos_script(&cmd, &script_path);
        assert!(script.contains(&format!("rm -f '{}'", script_path.display())));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_windows_render_escapes_quotes() {
        let cmd = TerminalCommand {
            executable: PathBuf::from("C:\\Users\\test user\\it's app\\login"),
            args: vec![OsString::from("arg's value")],
            env: vec![],
            env_remove: vec![],
        };
        let script_path = PathBuf::from("C:\\Users\\user\\AppData\\Local\\UsageCheck\\tmp\\login_abc123.ps1");
        let script = render_windows_script(&cmd, &script_path);
        assert!(script.contains("''"));
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
}
