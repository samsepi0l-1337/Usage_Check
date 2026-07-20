use super::*;
use crate::terminal::TerminalError;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static PROFILE_SEQ: AtomicU64 = AtomicU64::new(0);

fn unique_profile_root() -> std::path::PathBuf {
    let n = PROFILE_SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("uc-cli-test-{}-{}", std::process::id(), n))
}

struct MockTerminalLauncher {
    should_fail: bool,
    internal_fail: Arc<Mutex<bool>>,
}

impl MockTerminalLauncher {
    fn new() -> Self {
        Self {
            should_fail: false,
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
        Ok(unique_profile_root())
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
        Ok(unique_profile_root())
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
        Ok(unique_profile_root())
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
