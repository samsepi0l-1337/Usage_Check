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
