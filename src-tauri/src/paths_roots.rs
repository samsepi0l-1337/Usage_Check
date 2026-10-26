//! Injectable roots for the profile-level files the CLI sign-in flow touches.
//!
//! Production uses [`ProfileRoots::system`], which resolves through the process
//! `HOME` exactly like the free functions in `paths`. Callers that must not
//! touch the real home (tests driving menu actions) pass
//! [`ProfileRoots::rooted_at`] instead of mutating the environment.

use std::path::PathBuf;

use super::{default_claude_config_dir_checked_system, home_dir};

/// `ProfileRoots::default()` resolves through the process environment.
#[derive(Clone, Debug, Default)]
pub struct ProfileRoots {
    home: Option<PathBuf>,
}

impl ProfileRoots {
    /// Resolve everything below `home` and ignore `HOME` / `CLAUDE_CONFIG_DIR`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rooted_at(home: impl Into<PathBuf>) -> Self {
        Self {
            home: Some(home.into()),
        }
    }

    fn home(&self) -> Option<PathBuf> {
        self.home.clone().or_else(home_dir)
    }

    /// The default Claude config dir if it exists.
    pub fn default_claude_config_dir_checked(&self) -> Option<PathBuf> {
        match &self.home {
            None => default_claude_config_dir_checked_system(),
            Some(home) => Some(home.join(".claude")).filter(|dir| dir.exists()),
        }
    }

    /// `<home>/.usagecheck/profiles/claude/<account_id>`, created with 0700.
    pub fn claude_managed_root(&self, account_id: &str) -> Result<PathBuf, std::io::Error> {
        let root = self
            .home()
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
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(root)
    }
}
