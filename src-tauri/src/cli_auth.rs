use crate::terminal::{TerminalCommand, TerminalLauncher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use usage_core::account::{Account, AuthSource, ProfileOwnership};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    TerminalError,
    NeedsSetup,
    NeedsLogin,
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
mod tests {
    use super::*;
    use crate::terminal::TerminalError;
    use std::sync::{Arc, Mutex};

    struct MockTerminalLauncher {
        should_fail: bool,
        identity: Option<String>,
        internal_fail: Arc<Mutex<bool>>,
    }

    impl MockTerminalLauncher {
        fn new() -> Self {
            Self {
                should_fail: false,
                identity: None,
                internal_fail: Arc::new(Mutex::new(false)),
            }
        }
    }

    impl TerminalLauncher for MockTerminalLauncher {
        fn launch(&self, _cmd: &TerminalCommand) -> Result<(), TerminalError> {
            if self.should_fail || *self.internal_fail.lock().unwrap() {
                Err(TerminalError::LaunchFailed("launch failed".to_string()))
            } else {
                Ok(())
            }
        }
    }

    struct AlwaysAuthenticatedAdapter {
        exe_path: PathBuf,
    }

    impl ProviderAdapter for AlwaysAuthenticatedAdapter {
        fn probe(&self) -> Result<Option<Account>, String> {
            Ok(Some(Account {
                id: "test-account".to_string(),
                provider: usage_core::account::Provider::Codex,
                label: "test@example.com".to_string(),
                auth_source: AuthSource::CliProfile {
                    profile_root: PathBuf::from("/tmp/test"),
                    ownership: ProfileOwnership::Managed,
                    expected_identity: "test".to_string(),
                },
            }))
        }

        fn managed_profile_root(&self) -> Result<PathBuf, String> {
            Ok(std::env::temp_dir().join("uc-cli-test"))
        }

        fn login_command(&self, _profile_root: &Path) -> TerminalCommand {
            TerminalCommand {
                executable: self.exe_path.clone(),
                args: vec![],
                env: vec![],
                env_remove: vec![],
            }
        }

        fn resolve_account(&self, _auth_source: AuthSource) -> Result<Account, String> {
            Ok(Account {
                id: "test-account".to_string(),
                provider: usage_core::account::Provider::Codex,
                label: "test@example.com".to_string(),
                auth_source: AuthSource::CliProfile {
                    profile_root: PathBuf::from("/tmp/test"),
                    ownership: ProfileOwnership::Managed,
                    expected_identity: "test".to_string(),
                },
            })
        }
    }

    struct NeverAuthenticatedAdapter {
        exe_path: PathBuf,
    }

    impl ProviderAdapter for NeverAuthenticatedAdapter {
        fn probe(&self) -> Result<Option<Account>, String> {
            Ok(None)
        }

        fn managed_profile_root(&self) -> Result<PathBuf, String> {
            Ok(std::env::temp_dir().join("uc-cli-test"))
        }

        fn login_command(&self, _profile_root: &Path) -> TerminalCommand {
            TerminalCommand {
                executable: self.exe_path.clone(),
                args: vec![],
                env: vec![],
                env_remove: vec![],
            }
        }

        fn resolve_account(&self, _auth_source: AuthSource) -> Result<Account, String> {
            Err("no account found".to_string())
        }
    }

    struct NeverAuthenticatedMissingExeAdapter;

    impl ProviderAdapter for NeverAuthenticatedMissingExeAdapter {
        fn probe(&self) -> Result<Option<Account>, String> {
            Ok(None)
        }

        fn managed_profile_root(&self) -> Result<PathBuf, String> {
            Ok(std::env::temp_dir().join("uc-cli-test"))
        }

        fn login_command(&self, _profile_root: &Path) -> TerminalCommand {
            TerminalCommand {
                executable: PathBuf::from("/nonexistent/exe"),
                args: vec![],
                env: vec![],
                env_remove: vec![],
            }
        }

        fn resolve_account(&self, _auth_source: AuthSource) -> Result<Account, String> {
            Err("no account found".to_string())
        }
    }

    #[tokio::test]
    async fn test_execute_returns_account_when_already_authenticated() {
        let adapter = AlwaysAuthenticatedAdapter {
            exe_path: PathBuf::from("/bin/ls"),
        };
        let launcher = MockTerminalLauncher::new();
        let coord = CliAuthCoordinator::new(
            Box::new(adapter),
            Box::new(launcher),
            RetrySchedule::immediate(),
        );
        let result = coord.execute().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_launch_error_returns_terminal_error() {
        let adapter = NeverAuthenticatedAdapter {
            exe_path: PathBuf::from("/bin/ls"),
        };
        let launcher = MockTerminalLauncher {
            should_fail: true,
            identity: None,
            internal_fail: Arc::new(Mutex::new(false)),
        };
        let coord = CliAuthCoordinator::new(
            Box::new(adapter),
            Box::new(launcher),
            RetrySchedule::immediate(),
        );
        let result = coord.execute().await;
        assert_eq!(result, Err(AuthError::TerminalError));
    }

    #[tokio::test]
    async fn test_deadline_reached_no_account() {
        let adapter = NeverAuthenticatedAdapter {
            exe_path: PathBuf::from("/bin/ls"),
        };
        let launcher = MockTerminalLauncher::new();
        let coord = CliAuthCoordinator::new(
            Box::new(adapter),
            Box::new(launcher),
            RetrySchedule::immediate(),
        );
        let result = coord.execute().await;
        assert_eq!(result, Err(AuthError::DeadlineReached));
    }

    #[tokio::test]
    async fn test_missing_executable_returns_needs_setup() {
        let adapter = NeverAuthenticatedMissingExeAdapter;
        let launcher = MockTerminalLauncher::new();
        let coord = CliAuthCoordinator::new(
            Box::new(adapter),
            Box::new(launcher),
            RetrySchedule::immediate(),
        );
        let result = coord.execute().await;
        assert_eq!(result, Err(AuthError::NeedsSetup));
    }

    #[test]
    fn test_adapter_error_propagates_from_managed_profile_root() {
        struct ErrorAdapter;
        impl ProviderAdapter for ErrorAdapter {
            fn probe(&self) -> Result<Option<Account>, String> {
                Ok(None)
            }
            fn login_command(&self, _profile_root: &Path) -> TerminalCommand {
                panic!("login command must not be built when profile root resolution fails")
            }
            fn resolve_account(&self, _auth_source: AuthSource) -> Result<Account, String> {
                Ok(Account {
                    id: "test".to_string(),
                    provider: usage_core::account::Provider::Codex,
                    label: "test".to_string(),
                    auth_source: AuthSource::BrowserOAuth {
                        credential_id: "token".to_string(),
                    },
                })
            }
            fn managed_profile_root(&self) -> Result<std::path::PathBuf, String> {
                Err("simulated error".to_string())
            }
        }

        let launcher = MockTerminalLauncher {
            should_fail: false,
            identity: Some("test-id".to_string()),
            internal_fail: Arc::new(Mutex::new(false)),
        };

        let coordinator = CliAuthCoordinator::new(
            Box::new(ErrorAdapter),
            Box::new(launcher),
            RetrySchedule::immediate(),
        );

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(coordinator.execute());

        assert!(result.is_err());
        assert!(format!("{:?}", result.unwrap_err()).contains("AdapterError"));
    }
}
