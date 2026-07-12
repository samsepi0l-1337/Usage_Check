//! Claude managed-profile paths used by the CLI and status-line bridge.

use std::path::PathBuf;

use super::home_dir;

pub fn claude_managed_root(account_id: &str) -> Result<PathBuf, std::io::Error> {
    let root = home_dir()
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "home directory not found")
        })?
        .join(".usagecheck")
        .join("profiles")
        .join("claude")
        .join(account_id);
    std::fs::create_dir_all(&root)?;
    #[cfg(unix)]
    {
        use std::fs::Permissions;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, Permissions::from_mode(0o700))?;
    }
    Ok(root)
}

pub fn login_script_dir() -> Result<PathBuf, std::io::Error> {
    use super::usagecheck_app_data_dir;
    let script_dir = usagecheck_app_data_dir()
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "app data directory not found")
        })?
        .join("tmp");
    std::fs::create_dir_all(&script_dir)?;
    #[cfg(unix)]
    {
        use std::fs::Permissions;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_dir, Permissions::from_mode(0o700))?;
    }
    Ok(script_dir)
}

pub fn claude_statusline_snapshot(account_id: &str) -> PathBuf {
    claude_profile_root(account_id)
        .map(|root| root.join("statusline_snapshot.json"))
        .unwrap_or_else(|| PathBuf::from(format!("/tmp/claude-snapshot-{account_id}.json")))
}

pub fn claude_settings_json(account_id: &str) -> PathBuf {
    claude_profile_root(account_id)
        .map(|root| root.join("settings.json"))
        .unwrap_or_else(|| PathBuf::from(format!("/tmp/claude-settings-{account_id}.json")))
}

fn claude_profile_root(account_id: &str) -> Option<PathBuf> {
    home_dir().map(|home| {
        home.join(".usagecheck")
            .join("profiles")
            .join("claude")
            .join(account_id)
    })
}
