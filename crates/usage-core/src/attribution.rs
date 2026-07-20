//! Local usage attribution and prove/assume/merge logic per PLAN §4.3.
use crate::aggregate::aggregate;
use crate::models::{LocalProvenance, LocalUsage, ModelTokenEvent, RootIdentity};
use chrono::{DateTime, Utc};
use std::cmp::Ordering;
use std::path::{Component, Path, PathBuf};

/// Reference to an account's credentials and profile roots.
#[derive(Clone, Debug)]
pub struct AccountRef<'a> {
    pub account_id: &'a str,
    pub creds_account_id: Option<&'a str>,
    pub expected_identity: Option<&'a str>,
    pub is_browser_oauth: bool,
    pub profile_roots: Vec<PathBuf>,
}

/// A scanned profile root with raw events and provenance metadata.
#[derive(Clone, Debug)]
pub struct ScannedRoot {
    pub root_key: PathBuf,
    pub source_roots: Vec<PathBuf>,
    pub events: Vec<ModelTokenEvent>,
    pub health: LocalProvenance,
    pub identity: RootIdentity,
}

/// 2-phase attribution: proof first (Phase A), then sole-associate no-proof (Phase B).
/// Returns (account_id, LocalUsage) pairs.
pub fn assign_local_usage(
    accounts: &[AccountRef],
    roots: &[ScannedRoot],
    now: DateTime<Utc>,
) -> Vec<(String, LocalUsage)> {
    let mut assignments = vec![Assignment::default(); accounts.len()];

    for root in roots {
        let matched = matching_accounts(accounts, &root.identity);
        let associated = associated_accounts(accounts, root);

        match matched.as_slice() {
            [owner] => {
                assignments[*owner].add_root(root, proven_provenance(root));
                for account in associated {
                    if account != *owner {
                        assignments[account].add_signal(LocalProvenance::SharedProfileOther);
                    }
                }
            }
            [] if root.identity.is_absent() => match associated.as_slice() {
                [owner] => assignments[*owner].add_root(
                    root,
                    merge_provenance(LocalProvenance::Assumed, root.health),
                ),
                owners if owners.len() >= 2 => {
                    for owner in owners {
                        assignments[*owner].add_signal(LocalProvenance::Ambiguous);
                    }
                }
                _ => {}
            },
            [] => {
                for account in associated {
                    if !accounts[account].is_browser_oauth {
                        assignments[account].add_signal(LocalProvenance::Conflict);
                    }
                }
            }
            owners => {
                for owner in owners {
                    assignments[*owner].add_signal(LocalProvenance::Ambiguous);
                }
            }
        }
    }

    accounts
        .iter()
        .zip(assignments)
        .map(|(account, assignment)| (account.account_id.to_string(), assignment.finish(now)))
        .collect()
}

#[derive(Clone, Debug, Default)]
struct Assignment {
    events: Vec<ModelTokenEvent>,
    provenance: Option<LocalProvenance>,
}

impl Assignment {
    fn add_root(&mut self, root: &ScannedRoot, provenance: LocalProvenance) {
        self.events.extend(root.events.iter().cloned());
        self.add_signal(provenance);
    }

    fn add_signal(&mut self, provenance: LocalProvenance) {
        self.provenance = Some(match self.provenance {
            Some(current) => merge_provenance(current, provenance),
            None => provenance,
        });
    }

    fn finish(mut self, now: DateTime<Utc>) -> LocalUsage {
        let provenance = self.provenance.unwrap_or(LocalProvenance::NoLocalProfile);
        if matches!(
            provenance,
            LocalProvenance::Ambiguous | LocalProvenance::Conflict
        ) {
            return LocalUsage::none(provenance);
        }

        self.events.sort_by(canonical_event_order);
        LocalUsage {
            totals: aggregate(&self.events, now),
            provenance,
        }
    }
}

fn matching_accounts(accounts: &[AccountRef], identity: &RootIdentity) -> Vec<usize> {
    let (root_account_id, root_email) = match identity {
        RootIdentity::CodexAuth { account_id, email } => (account_id.as_deref(), email.as_deref()),
        RootIdentity::ClaudeEmail { email } => (None, email.as_deref()),
        RootIdentity::None => (None, None),
    };

    accounts
        .iter()
        .enumerate()
        .filter_map(|(index, account)| {
            let account_id_matches =
                root_account_id
                    .zip(account.creds_account_id)
                    .is_some_and(|(root, expected)| {
                        !root.trim().is_empty() && !expected.trim().is_empty() && root == expected
                    });
            let email_matches =
                root_email
                    .zip(account.expected_identity)
                    .is_some_and(|(root, expected)| {
                        !root.trim().is_empty()
                            && !expected.trim().is_empty()
                            && root.eq_ignore_ascii_case(expected)
                    });
            (account_id_matches || email_matches).then_some(index)
        })
        .collect()
}

fn associated_accounts(accounts: &[AccountRef], root: &ScannedRoot) -> Vec<usize> {
    let source_roots: Vec<PathBuf> = root
        .source_roots
        .iter()
        .map(|path| normalize_path(path))
        .collect();
    accounts
        .iter()
        .enumerate()
        .filter_map(|(index, account)| {
            account
                .profile_roots
                .iter()
                .map(|path| normalize_path(path))
                .any(|path| source_roots.contains(&path))
                .then_some(index)
        })
        .collect()
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn proven_provenance(root: &ScannedRoot) -> LocalProvenance {
    let proof = if root.events.is_empty() {
        LocalProvenance::NoEvents
    } else {
        LocalProvenance::Ok
    };
    merge_provenance(proof, root.health)
}

fn merge_provenance(left: LocalProvenance, right: LocalProvenance) -> LocalProvenance {
    if left.severity_rank() >= right.severity_rank() {
        left
    } else {
        right
    }
}

fn canonical_event_order(left: &ModelTokenEvent, right: &ModelTokenEvent) -> Ordering {
    right
        .tokens
        .cmp(&left.tokens)
        .then_with(|| right.timestamp.cmp(&left.timestamp))
        .then_with(|| left.model.cmp(&right.model))
}

#[cfg(test)]
#[path = "attribution_tests.rs"]
mod tests;
