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

    // Executable and args must share ONE command line — a trailing `\n` after
    // each token would make bash run them as separate commands (dropping the
    // args, and invoking the system `login` binary for `codex login`).
    let exe_str = command.executable.display().to_string().replace("'", "'\\''");
    let mut command_line = format!("'{}'", exe_str);
    for arg in &command.args {
        let arg_str = arg.to_string_lossy().replace("'", "'\\''");
        command_line.push_str(&format!(" '{}'", arg_str));
    }
    let rm_path = script_path.display().to_string().replace("'", "'\\''");
    script.push_str(&format!("__usagecheck_script='{}'\n", rm_path));
    script.push_str("trap 'rm -f \"$__usagecheck_script\"' EXIT\n");

    script.push_str(&command_line);
    script.push('\n');

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

        let script_path_escaped = script_path
            .display()
            .to_string()
            .replace("'", "'\\''")
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
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
        let var_str = var.to_string_lossy().replace("'", "''");
        script.push_str(&format!(
            "[Environment]::SetEnvironmentVariable('{}', $null, 'Process')\n",
            var_str
        ));
    }

    for (key, value) in &command.env {
        let key_str = key.to_string_lossy().replace("'", "''");
        let value_str = value.to_string_lossy().replace("'", "''");
        script.push_str(&format!(
            "[Environment]::SetEnvironmentVariable('{}', '{}', 'Process')\n",
            key_str,
            value_str
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
#[path = "terminal_tests.rs"]
mod tests;
