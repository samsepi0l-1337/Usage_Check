use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use usage_core::account::{Account, AuthSource, Provider};
use usage_core::attribution::{assign_local_usage, AccountRef, ScannedRoot};
use usage_core::models::{LocalProvenance, LocalUsage, ModelTokenEvent};
use usage_core::scanners::{claude as claude_scanner, codex as codex_scanner};

use crate::paths;
use crate::store::AccountStore;

const MAX_LOCAL_FILES: usize = 50_000;
const MAX_LOCAL_SCAN_TIME: Duration = Duration::from_secs(5);

pub(super) async fn local_usage_for_provider(
    store: &AccountStore,
    accounts: &[Account],
    provider: Provider,
    now: DateTime<Utc>,
) -> HashMap<String, LocalUsage> {
    let provider_accounts: Vec<&Account> = accounts
        .iter()
        .filter(|account| account.provider == provider)
        .collect();
    if provider_accounts.is_empty() {
        return HashMap::new();
    }

    let mut profile_roots = match provider {
        Provider::Codex => paths::codex_home().into_iter().collect(),
        Provider::Claude => paths::claude_config_roots(),
        _ => return HashMap::new(),
    };
    profile_roots.extend(provider_accounts.iter().filter_map(|account| {
        if let AuthSource::CliProfile { profile_root, .. } = &account.auth_source {
            Some(profile_root.clone())
        } else {
            None
        }
    }));
    let profile_roots = match provider {
        Provider::Codex => paths::codex_profile_roots(&profile_roots),
        Provider::Claude => paths::claude_profile_roots(&profile_roots),
        _ => Vec::new(),
    };

    let mut scanned = Vec::with_capacity(profile_roots.len());
    for profile_root in profile_roots {
        let scan_roots = match provider {
            Provider::Codex => paths::codex_session_roots_for(&profile_root),
            Provider::Claude => paths::claude_project_roots_for(&profile_root),
            _ => Vec::new(),
        };
        let scan = scan_local_events(provider, &scan_roots, now).await;
        let health = scan_provenance(&scan);
        scanned.push(ScannedRoot {
            root_key: profile_root.clone(),
            source_roots: vec![profile_root.clone()],
            events: scan.events,
            health,
            identity: paths::root_identity(provider, &profile_root),
        });
    }

    let credential_ids: Vec<Option<String>> = provider_accounts
        .iter()
        .map(|account| {
            store
                .credentials(&account.id)
                .and_then(|credentials| credentials.account_id)
        })
        .collect();
    let account_refs: Vec<AccountRef<'_>> = provider_accounts
        .iter()
        .zip(&credential_ids)
        .map(|(account, credential_id)| AccountRef {
            account_id: &account.id,
            creds_account_id: credential_id.as_deref(),
            expected_identity: expected_identity(account),
            is_browser_oauth: matches!(account.auth_source, AuthSource::BrowserOAuth { .. }),
            profile_roots: account_profile_roots(provider, account),
        })
        .collect();

    assign_local_usage(&account_refs, &scanned, now)
        .into_iter()
        .collect()
}

fn account_profile_roots(provider: Provider, account: &Account) -> Vec<PathBuf> {
    let AuthSource::CliProfile { profile_root, .. } = &account.auth_source else {
        return Vec::new();
    };
    match provider {
        Provider::Codex => paths::codex_profile_roots(std::slice::from_ref(profile_root)),
        Provider::Claude => paths::claude_profile_roots(std::slice::from_ref(profile_root)),
        _ => Vec::new(),
    }
}

fn expected_identity(account: &Account) -> Option<&str> {
    match &account.auth_source {
        AuthSource::CliProfile {
            expected_identity, ..
        } => Some(expected_identity),
        AuthSource::BrowserOAuth { .. } => Some(&account.label),
        _ => None,
    }
}

fn scan_provenance(scan: &ScanResult) -> LocalProvenance {
    if scan.health.truncated {
        LocalProvenance::Truncated
    } else if scan.health.root_unreadable {
        LocalProvenance::Unavailable
    } else if scan.health.any_read_error {
        LocalProvenance::Partial
    } else if scan.events.is_empty() {
        LocalProvenance::NoEvents
    } else {
        LocalProvenance::Ok
    }
}

/// Result of scanning a local provider root for events.
#[derive(Clone, Debug)]
pub struct ScanResult {
    pub events: Vec<ModelTokenEvent>,
    pub health: ScanHealth,
}

/// Health status of a scan operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanHealth {
    pub any_read_error: bool,
    pub root_unreadable: bool,
    pub truncated: bool,
}

/// Scan local provider roots for token events (raw, not aggregated).
pub async fn scan_local_events(
    provider: Provider,
    scan_roots: &[PathBuf],
    _now: DateTime<Utc>,
) -> ScanResult {
    let roots = scan_roots.to_vec();
    tokio::task::spawn_blocking(move || scan_local_events_blocking(provider, &roots))
        .await
        .unwrap_or_else(|_| ScanResult {
            events: Vec::new(),
            health: ScanHealth {
                any_read_error: true,
                root_unreadable: true,
                truncated: false,
            },
        })
}


fn scan_local_events_blocking(provider: Provider, roots: &[PathBuf]) -> ScanResult {
    let started = Instant::now();
    let mut result = ScanResult {
        events: Vec::new(),
        health: ScanHealth {
            any_read_error: false,
            root_unreadable: false,
            truncated: false,
        },
    };
    let mut visited = HashSet::new();
    let mut files_read = 0;

    for root in roots {
        if result.health.truncated {
            break;
        }
        let metadata = match std::fs::symlink_metadata(root) {
            Ok(metadata) => metadata,
            // A scan root that simply does not exist contributes no events and is NOT an
            // error — e.g. a managed profile with no `projects/` dir. Only genuine read
            // failures (permission, I/O) count as unreadable/"unavailable".
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                result.health.root_unreadable = true;
                continue;
            }
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            result.health.root_unreadable = true;
            continue;
        }
        if std::fs::read_dir(root).is_err() {
            result.health.root_unreadable = true;
            continue;
        }
        let root_key = root.canonicalize().unwrap_or_else(|_| root.clone());
        if !visited.insert(root_key) {
            continue;
        }
        scan_directory(
            provider,
            root,
            &mut result,
            &mut visited,
            &mut files_read,
            started,
        );
    }
    result
}

fn scan_directory(
    provider: Provider,
    root: &Path,
    result: &mut ScanResult,
    visited: &mut HashSet<PathBuf>,
    files_read: &mut usize,
    started: Instant,
) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        if started.elapsed() >= MAX_LOCAL_SCAN_TIME {
            result.health.truncated = true;
            return;
        }
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                result.health.any_read_error = true;
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    result.health.any_read_error = true;
                    continue;
                }
            };
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => {
                    result.health.any_read_error = true;
                    continue;
                }
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                let key = path.canonicalize().unwrap_or_else(|_| path.clone());
                if visited.insert(key) {
                    stack.push(path);
                }
                continue;
            }
            if !is_jsonl(&path) {
                continue;
            }
            if *files_read >= MAX_LOCAL_FILES || started.elapsed() >= MAX_LOCAL_SCAN_TIME {
                result.health.truncated = true;
                return;
            }
            *files_read += 1;
            read_event_file(provider, &path, result);
        }
    }
}

fn read_event_file(provider: Provider, path: &Path, result: &mut ScanResult) {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => {
            result.health.any_read_error = true;
            return;
        }
    };
    for line in BufReader::new(file).lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => {
                result.health.any_read_error = true;
                return;
            }
        };
        if let Some(event) = parse_local_event(provider, &line) {
            result.events.push(event);
        }
    }
}

fn parse_local_event(provider: Provider, line: &str) -> Option<ModelTokenEvent> {
    let parsed = match provider {
        Provider::Codex => codex_scanner::parse_codex_line(line),
        Provider::Claude => claude_scanner::parse_claude_line(line),
        _ => None,
    };
    parsed.or_else(|| serde_json::from_str(line).ok())
}

fn is_jsonl(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("jsonl")
}

#[cfg(test)]
#[path = "local_scan_tests.rs"]
mod tests;
