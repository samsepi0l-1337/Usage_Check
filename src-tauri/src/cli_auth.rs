use crate::terminal::{TerminalCommand, TerminalLauncher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use usage_core::account::{Account, AuthSource, ProfileOwnership};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    TerminalError,
    NeedsSetup,
    DeadlineReached,
    AdapterError(String),
}

pub trait ProviderAdapter: Send + Sync {
    fn probe(&self) -> Result<Option<Account>, String>;
    fn login_command(&self, profile_root: &Path) -> TerminalCommand;
    fn resolve_account(&self, auth_source: AuthSource) -> Result<Account, String>;
    fn managed_profile_root(&self) -> Result<std::path::PathBuf, String>;
}

pub struct RetrySchedule {
    pub interval: Duration,
    pub max_wait: Duration,
}

impl RetrySchedule {
    pub fn production() -> Self {
        Self {
            interval: Duration::from_secs(2),
            max_wait: Duration::from_secs(300),
        }
    }

    #[cfg(test)]
    pub fn immediate() -> Self {
        Self {
            interval: Duration::from_millis(0),
            max_wait: Duration::from_millis(10),
        }
    }
}

pub struct CliAuthCoordinator {
    adapter: Box<dyn ProviderAdapter>,
    launcher: Box<dyn TerminalLauncher>,
    retry_schedule: RetrySchedule,
}

impl CliAuthCoordinator {
    pub fn new(
        adapter: Box<dyn ProviderAdapter>,
        launcher: Box<dyn TerminalLauncher>,
        retry_schedule: RetrySchedule,
    ) -> Self {
        Self {
            adapter,
            launcher,
            retry_schedule,
        }
    }

    pub async fn execute(&self) -> Result<Account, AuthError> {
        match self.adapter.probe() {
            Ok(Some(account)) => Ok(account),
            Ok(None) => {
                let profile_path = self.get_managed_profile_path()?;
                let login_cmd = self.adapter.login_command(&profile_path);

                if !login_cmd.executable.exists() {
                    return Err(AuthError::NeedsSetup);
                }

                if self.launcher.launch(&login_cmd).is_err() {
                    return Err(AuthError::TerminalError);
                }

                let auth_source = AuthSource::CliProfile {
                    profile_root: profile_path,
                    ownership: ProfileOwnership::Managed,
                    expected_identity: String::new(),
                };

                if self.wait_for_authentication().await.is_err() {
                    return Err(AuthError::DeadlineReached);
                }

                match self.adapter.resolve_account(auth_source) {
                    Ok(account) => Ok(account),
                    Err(e) => Err(AuthError::AdapterError(e)),
                }
            }
            Err(e) => Err(AuthError::AdapterError(e)),
        }
    }

    fn get_managed_profile_path(&self) -> Result<PathBuf, AuthError> {
        let profile_dir = self
            .adapter
            .managed_profile_root()
            .map_err(AuthError::AdapterError)?;
        std::fs::create_dir_all(&profile_dir)
            .map_err(|_| AuthError::AdapterError("Failed to create profile directory".into()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o700);
            std::fs::set_permissions(&profile_dir, perms).map_err(|_| {
                AuthError::AdapterError("Failed to set directory permissions".into())
            })?;
        }

        Ok(profile_dir)
    }

    async fn wait_for_authentication(&self) -> Result<(), AuthError> {
        let deadline = Instant::now() + self.retry_schedule.max_wait;
        loop {
            if Instant::now() >= deadline {
                return Err(AuthError::DeadlineReached);
            }
            match self.adapter.probe() {
                Ok(Some(_)) => return Ok(()),
                _ => {
                    tokio::time::sleep(self.retry_schedule.interval).await;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "cli_auth_tests.rs"]
mod tests;
