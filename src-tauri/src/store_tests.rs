use super::*;
use usage_core::account::{Account, AuthSource, ProfileOwnership, Provider};

struct TestSandbox {
    root: PathBuf,
}

impl TestSandbox {
    fn new() -> Self {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).expect("create test sandbox");
        Self { root }
    }

    fn store(&self) -> AccountStore {
        AccountStore::new_at(self.root.join("UsageCheck"))
    }
}

impl Drop for TestSandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn credentials(identity: &str) -> Credentials {
    Credentials {
        access_token: "test-only-placeholder".into(),
        refresh_token: None,
        account_id: Some(identity.into()),
        expires_at: None,
    }
}

/// Redirects `USAGECHECK_APP_DATA_DIR` for the duration of a test, restoring
/// the previous value on drop. Taken only while holding `LICENSE_ENV_LOCK`.
struct AppDataDirGuard(Option<std::ffi::OsString>);

impl AppDataDirGuard {
    fn set(path: &Path) -> Self {
        let previous = std::env::var_os("USAGECHECK_APP_DATA_DIR");
        std::env::set_var("USAGECHECK_APP_DATA_DIR", path);
        Self(previous)
    }
}

impl Drop for AppDataDirGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(previous) => std::env::set_var("USAGECHECK_APP_DATA_DIR", previous),
            None => std::env::remove_var("USAGECHECK_APP_DATA_DIR"),
        }
    }
}

#[test]
fn index_roundtrips() {
    let accts = vec![Account {
        id: "1".into(),
        provider: Provider::Codex,
        label: "work".into(),
        auth_source: AuthSource::CliProfile {
            profile_root: "/profiles/codex-work".into(),
            ownership: ProfileOwnership::External,
            expected_identity: "work".into(),
        },
    }];
    let s = serialize_index(&accts);
    assert_eq!(parse_index(&s), accts);
}

#[test]
fn parse_index_empty_on_garbage() {
    assert_eq!(parse_index("not json"), Vec::<Account>::new());
}

#[test]
fn parse_index_empty_on_empty_string() {
    assert_eq!(parse_index(""), Vec::<Account>::new());
}

// H6: `write_private_file` must create its target atomically at mode 0600
// (never briefly at a wider, umask-derived mode) and must propagate a
// permission-setting failure rather than silently ignoring it. Both are
// meaningless on Windows (no POSIX permission bits), hence unix-only.
#[cfg(unix)]
#[test]
fn write_private_file_creates_file_at_mode_0600() {
    use std::os::unix::fs::PermissionsExt;

    let sandbox = TestSandbox::new();
    let path = sandbox.root.join("secret.txt");
    write_private_file(&path, "top secret").expect("write_private_file should succeed");

    let mode = fs::metadata(&path)
        .expect("stat written file")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o600,
        "file must be created at exactly 0600, got {mode:o}"
    );
}

#[cfg(unix)]
#[test]
fn write_private_file_propagates_permission_failure() {
    use std::os::unix::fs::PermissionsExt;

    // Locking `parent` itself (rather than its ANCESTOR) does not work as a
    // fault-injection here: `write_private_file` calls
    // `set_private_dir_permissions` (chmod 0700) on `parent` unconditionally
    // before creating the temp file, and chmod only requires being the
    // owner — not write access — so a directly-locked `parent` would be
    // silently re-widened before the create attempt. Locking the PARENT's
    // OWN parent instead means `parent` cannot even be created by
    // `fs::create_dir_all`, which fails and propagates before
    // `set_private_dir_permissions` (or anything else) ever runs.
    let sandbox = TestSandbox::new();
    let locked_root = sandbox.root.join("locked-root");
    fs::create_dir_all(&locked_root).expect("create locked root");
    fs::set_permissions(&locked_root, fs::Permissions::from_mode(0o500))
        .expect("lock down root permissions (read+execute only, no write)");

    let target = locked_root.join("nested").join("secret.txt");
    let result = write_private_file(&target, "top secret");

    // Restore write access before the sandbox's Drop tries to remove it.
    fs::set_permissions(&locked_root, fs::Permissions::from_mode(0o700))
        .expect("restore permissions for cleanup");

    assert!(
        result.is_err(),
        "write_private_file must propagate a directory/file-creation failure, not ignore it"
    );
}

fn known_account(id: &str) -> Account {
    Account {
        id: id.into(),
        provider: Provider::Codex,
        label: "known@example.com".into(),
        auth_source: AuthSource::CliProfile {
            profile_root: "/profiles/codex-known".into(),
            ownership: ProfileOwnership::External,
            expected_identity: "known@example.com".into(),
        },
    }
}

fn unknown_account() -> serde_json::Value {
    serde_json::json!({
        "id": "unknown-account",
        "provider": "__nope__",
        "label": "unknown@example.com",
        "auth_source": { "kind": "unknown_source" }
    })
}

fn write_mixed_index(store: &AccountStore, known: &Account) {
    store.initialize_v2().unwrap();
    let entries = vec![serde_json::to_value(known).unwrap(), unknown_account()];
    fs::write(
        store.index_path(),
        serde_json::to_string_pretty(&entries).unwrap(),
    )
    .unwrap();
}

fn claude_reference(profile_root: PathBuf, expected_identity: &str, label: &str) -> Account {
    Account {
        id: "claude-account".into(),
        provider: Provider::Claude,
        label: label.into(),
        auth_source: AuthSource::CliProfile {
            profile_root,
            ownership: ProfileOwnership::External,
            expected_identity: expected_identity.into(),
        },
    }
}

fn write_claude_oauth_identity(
    profile_root: &Path,
    email: &str,
    account_uuid: Option<&str>,
    organization_uuid: &str,
) {
    fs::create_dir_all(profile_root).unwrap();
    let mut oauth_account = serde_json::json!({
        "emailAddress": email,
        "organizationUuid": organization_uuid,
    });
    if let Some(account_uuid) = account_uuid {
        oauth_account["accountUuid"] = serde_json::Value::String(account_uuid.into());
    }
    fs::write(
        profile_root.join(".claude.json"),
        serde_json::json!({ "oauthAccount": oauth_account }).to_string(),
    )
    .unwrap();
}

#[test]
fn list_skips_unknown_provider_entries() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let known = known_account("known-account");
    write_mixed_index(&store, &known);

    assert_eq!(store.list(), vec![known]);
}

#[test]
fn mutation_preserves_unknown_provider_entries() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let known = known_account("known-account");
    write_mixed_index(&store, &known);

    store
        .add_reference_with(
            Provider::Claude,
            "new@example.com".into(),
            AuthSource::CliProfile {
                profile_root: "/profiles/claude-new".into(),
                ownership: ProfileOwnership::External,
                expected_identity: "new@example.com".into(),
            },
            || true,
        )
        .unwrap();

    let values: Vec<serde_json::Value> =
        serde_json::from_str(&fs::read_to_string(store.index_path()).unwrap()).unwrap();
    assert!(values.contains(&unknown_account()));
    assert!(values
        .iter()
        .any(|value| value["label"] == "new@example.com"));
}

#[test]
fn migrate_claude_identity_anchors_upgrades_anchor_and_label() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let profile_root = sandbox.root.join("claude-profile");
    write_claude_oauth_identity(&profile_root, "me@x.io", Some("acc-9"), "org-z");
    let account = claude_reference(profile_root, "org-z", "org-z");
    store.initialize_v2().unwrap();
    store.save_index(std::slice::from_ref(&account)).unwrap();

    assert_eq!(store.migrate_claude_identity_anchors().unwrap(), 1);
    let migrated = store.account(&account.id).unwrap();
    let AuthSource::CliProfile {
        expected_identity, ..
    } = migrated.auth_source
    else {
        panic!("migrated account must remain a Claude CLI profile");
    };
    assert_eq!(expected_identity, "acc-9");
    assert_eq!(migrated.label, "me@x.io");
}

#[test]
fn migrate_claude_identity_anchors_skips_profiles_without_account_uuid() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let profile_root = sandbox.root.join("claude-profile");
    write_claude_oauth_identity(&profile_root, "me@x.io", None, "org-z");
    let account = claude_reference(profile_root, "org-z", "org-z");
    store.initialize_v2().unwrap();
    store.save_index(std::slice::from_ref(&account)).unwrap();

    assert_eq!(store.migrate_claude_identity_anchors().unwrap(), 0);
    assert_eq!(store.account(&account.id), Some(account));
}

#[test]
fn migrate_claude_identity_anchors_preserves_unknown_entries() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let profile_root = sandbox.root.join("claude-profile");
    write_claude_oauth_identity(&profile_root, "me@x.io", Some("acc-9"), "org-z");
    let account = claude_reference(profile_root, "org-z", "org-z");
    write_mixed_index(&store, &account);

    assert_eq!(store.migrate_claude_identity_anchors().unwrap(), 1);
    let values: Vec<serde_json::Value> =
        serde_json::from_str(&fs::read_to_string(store.index_path()).unwrap()).unwrap();
    assert!(values.contains(&unknown_account()));
}

#[test]
fn migrate_claude_identity_anchors_is_idempotent_and_ignores_non_claude() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let profile_root = sandbox.root.join("claude-profile");
    write_claude_oauth_identity(&profile_root, "me@x.io", Some("acc-9"), "org-z");
    let migrated = claude_reference(profile_root, "acc-9", "me@x.io");
    let non_claude = Account {
        id: "codex-account".into(),
        provider: Provider::Codex,
        label: "codex@example.com".into(),
        auth_source: AuthSource::CliProfile {
            profile_root: sandbox.root.join("codex-profile"),
            ownership: ProfileOwnership::External,
            expected_identity: "codex@example.com".into(),
        },
    };
    store.initialize_v2().unwrap();
    store
        .save_index(&[migrated.clone(), non_claude.clone()])
        .unwrap();

    assert_eq!(store.migrate_claude_identity_anchors().unwrap(), 0);
    assert_eq!(store.list(), vec![migrated, non_claude]);
}

#[test]
fn remove_preserves_unknown_provider_entries() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let known = known_account("known-account");
    write_mixed_index(&store, &known);

    assert_eq!(store.remove(&known.id).unwrap(), Some(known));

    let values: Vec<serde_json::Value> =
        serde_json::from_str(&fs::read_to_string(store.index_path()).unwrap()).unwrap();
    assert_eq!(values, vec![unknown_account()]);
}

#[test]
fn remove_deletes_cli_profile_token_cache() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let account = store
        .add_reference_with(
            Provider::Claude,
            "claude@example.com".into(),
            AuthSource::CliProfile {
                profile_root: "/profiles/claude".into(),
                ownership: ProfileOwnership::External,
                expected_identity: "claude@example.com".into(),
            },
            || true,
        )
        .unwrap();
    store
        .set_cli_profile_credentials(&account.id, &credentials("x"))
        .unwrap();
    let cache_path = sandbox
        .root
        .join("UsageCheck")
        .join("cli-token-cache")
        .join(format!("{}.json", account.id));
    assert!(cache_path.exists());

    assert_eq!(store.remove(&account.id).unwrap(), Some(account.clone()));

    assert!(store.cli_profile_credentials(&account.id).is_none());
    assert!(!cache_path.exists());
}

#[test]
fn partitioned_read_rejects_genuine_corruption() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store.initialize_v2().unwrap();

    for contents in ["garbage", "{}"] {
        fs::write(store.index_path(), contents).unwrap();
        assert!(store.read_index_partitioned().is_err());
    }
}

#[test]
fn corrupt_index_is_not_silently_wiped_by_mutation() {
    // Regression (B1): a half-written/corrupt index must make mutators fail
    // loudly, NOT be read as "zero accounts" and overwritten — that would
    // permanently destroy the entire catalog.
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store.initialize_v2().unwrap();
    store
        .add_with(
            Provider::Codex,
            "user@example.com".into(),
            credentials("acct-1"),
            || true,
        )
        .unwrap();
    assert_eq!(store.list().len(), 1);

    fs::write(store.index_path(), "{ this is not valid json").unwrap();

    let result = store.add_with(
        Provider::Claude,
        "other@example.com".into(),
        credentials("acct-2"),
        || true,
    );
    assert!(
        result.is_err(),
        "mutation on corrupt index must error, got {result:?}"
    );

    // The corrupt bytes are still present — not clobbered to a fresh list.
    let raw = fs::read_to_string(store.index_path()).unwrap();
    assert!(
        raw.contains("not valid json"),
        "corrupt index must be left intact, got: {raw}"
    );
}

#[test]
fn atomic_write_leaves_no_temp_files() {
    // Regression (B1): the atomic temp+rename must not leave stray temp
    // siblings in the store root after a successful write.
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store.initialize_v2().unwrap();
    store
        .add_with(
            Provider::Codex,
            "user@example.com".into(),
            credentials("acct-1"),
            || true,
        )
        .unwrap();
    let leftovers: Vec<_> = fs::read_dir(store.index_path().parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "temp files leaked: {leftovers:?}");
}

#[test]
fn oauth_reimport_same_identity_is_rejected() {
    // Regression (M5): re-login/re-import of the same OAuth identity must not
    // create a duplicate account (polled and displayed twice).
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store.initialize_v2().unwrap();
    store
        .add_with(
            Provider::Codex,
            "user@example.com".into(),
            credentials("acct-1"),
            || true,
        )
        .unwrap();
    let second = store.add_with(
        Provider::Codex,
        "user@example.com".into(),
        credentials("acct-1"),
        || true,
    );
    assert!(
        second.is_err(),
        "duplicate OAuth identity must be rejected, got {second:?}"
    );
    assert_eq!(store.list().len(), 1);
}

#[test]
fn oauth_email_only_then_id_reimport_is_rejected() {
    // Regression (M5 follow-up): dedup must be symmetric — a legacy
    // email-only account must still be recognized as a duplicate when the
    // SAME identity is re-imported later carrying an account_id (and vice
    // versa), not just when both sides use the same single key kind.
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store.initialize_v2().unwrap();
    let email_only_creds = Credentials {
        access_token: "test-only-placeholder".into(),
        refresh_token: None,
        account_id: None,
        expires_at: None,
    };
    store
        .add_with(
            Provider::Codex,
            "user@example.com".into(),
            email_only_creds,
            || true,
        )
        .unwrap();
    let second = store.add_with(
        Provider::Codex,
        "user@example.com".into(),
        credentials("acct-1"),
        || true,
    );
    assert!(
        second.is_err(),
        "email-only account then id-carrying reimport of the same identity must be rejected, got {second:?}"
    );
    assert_eq!(store.list().len(), 1);
}

#[test]
fn oauth_distinct_identities_coexist() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store.initialize_v2().unwrap();
    store
        .add_with(
            Provider::Codex,
            "a@example.com".into(),
            credentials("acct-a"),
            || true,
        )
        .unwrap();
    store
        .add_with(
            Provider::Codex,
            "b@example.com".into(),
            credentials("acct-b"),
            || true,
        )
        .unwrap();
    assert_eq!(store.list().len(), 2);
}

#[test]
fn v2_initialization_resets_only_the_injected_legacy_store() {
    let sandbox = TestSandbox::new();
    let store_root = sandbox.root.join("UsageCheck");
    let external_profile = sandbox.root.join("provider-profile");
    fs::create_dir_all(store_root.join("credentials")).unwrap();
    fs::create_dir_all(&external_profile).unwrap();
    fs::write(store_root.join("accounts.json"), "legacy").unwrap();
    fs::write(store_root.join("credentials").join("legacy.json"), "legacy").unwrap();
    fs::write(store_root.join("sibling-sentinel"), "keep").unwrap();
    fs::write(external_profile.join("provider-auth.json"), "keep").unwrap();

    sandbox.store().initialize_v2().unwrap();

    assert!(!store_root.join("accounts.json").exists());
    assert!(!store_root.join("credentials").exists());
    assert!(store_root.join("schema-v2").exists());
    assert!(store_root.join("accounts-v2.json").exists());
    assert_eq!(
        fs::read_to_string(store_root.join("sibling-sentinel")).unwrap(),
        "keep"
    );
    assert_eq!(
        fs::read_to_string(external_profile.join("provider-auth.json")).unwrap(),
        "keep"
    );
}

#[test]
fn v2_initialization_is_idempotent() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store.initialize_v2().unwrap();
    let account = store
        .add_reference_with(
            Provider::Codex,
            "work".into(),
            AuthSource::CliProfile {
                profile_root: sandbox.root.join("codex-profile"),
                ownership: ProfileOwnership::External,
                expected_identity: "work@example.com".into(),
            },
            || true,
        )
        .unwrap();

    store.initialize_v2().unwrap();

    assert_eq!(store.list(), vec![account.clone()]);
    assert_eq!(store.account(&account.id), Some(account));
}

#[cfg(unix)]
#[test]
fn v2_rejects_symlinked_root_without_touching_target() {
    use std::os::unix::fs::symlink;

    let sandbox = TestSandbox::new();
    let outside = sandbox.root.join("outside-store");
    let linked_root = sandbox.root.join("linked-store");
    fs::create_dir_all(outside.join("credentials")).unwrap();
    fs::write(outside.join("accounts.json"), "legacy").unwrap();
    fs::write(outside.join("credentials").join("legacy.json"), "keep").unwrap();
    symlink(&outside, &linked_root).unwrap();

    let result = AccountStore::new_at(linked_root).initialize_v2();

    assert!(result.is_err());
    assert_eq!(
        fs::read_to_string(outside.join("accounts.json")).unwrap(),
        "legacy"
    );
    assert_eq!(
        fs::read_to_string(outside.join("credentials").join("legacy.json")).unwrap(),
        "keep"
    );
    assert!(!outside.join("schema-v2").exists());
    assert!(!outside.join("accounts-v2.json").exists());
}

#[cfg(unix)]
#[test]
fn v2_rejects_symlinked_index_and_marker_without_overwriting_targets() {
    use std::os::unix::fs::symlink;

    let sandbox = TestSandbox::new();
    let root = sandbox.root.join("UsageCheck");
    let outside_index = sandbox.root.join("outside-index");
    let outside_marker = sandbox.root.join("outside-marker");
    fs::create_dir_all(&root).unwrap();
    fs::write(&outside_index, "outside-index").unwrap();
    fs::write(&outside_marker, "2\n").unwrap();
    symlink(&outside_index, root.join("accounts-v2.json")).unwrap();
    symlink(&outside_marker, root.join("schema-v2")).unwrap();

    let result = sandbox.store().initialize_v2();

    assert!(result.is_err());
    assert_eq!(fs::read_to_string(outside_index).unwrap(), "outside-index");
    assert_eq!(fs::read_to_string(outside_marker).unwrap(), "2\n");
}

#[cfg(unix)]
#[test]
fn v2_rejects_symlinked_credentials_directory_without_writing_outside() {
    use std::os::unix::fs::symlink;

    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store.initialize_v2().unwrap();
    let outside = sandbox.root.join("outside-credentials");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("sentinel"), "keep").unwrap();
    symlink(
        &outside,
        sandbox.root.join("UsageCheck").join("credentials"),
    )
    .unwrap();

    let result = store.add_secret_with(
        Provider::Grok,
        "xAI API credits".into(),
        SecretSource::XaiManagement {
            team_id: "team-symlink".into(),
        },
        credentials("xai"),
        || true,
    );

    assert!(result.is_err());
    assert_eq!(
        fs::read_to_string(outside.join("sentinel")).unwrap(),
        "keep"
    );
    assert_eq!(fs::read_dir(outside).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn v2_rejects_symlinked_credential_file_without_overwriting_target() {
    use std::os::unix::fs::symlink;

    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let account = store
        .add_secret_with(
            Provider::Grok,
            "xAI API credits".into(),
            SecretSource::XaiManagement {
                team_id: "team-file-symlink".into(),
            },
            credentials("before"),
            || true,
        )
        .unwrap();
    let AuthSource::XaiManagement { credential_id, .. } = &account.auth_source else {
        unreachable!()
    };
    let credential_path = store.credential_path(credential_id).unwrap();
    fs::remove_file(&credential_path).unwrap();
    let outside = sandbox.root.join("outside-credential-file");
    fs::write(&outside, "keep").unwrap();
    symlink(&outside, &credential_path).unwrap();

    let result = store.update_credentials(credential_id, &credentials("after"));

    assert!(result.is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "keep");
}

#[test]
fn v2_grok_compatibility_add_uses_credential_id_for_credentials() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let account = store
        .add_with(
            Provider::Grok,
            "xAI API credits".into(),
            credentials("team-compat"),
            || true,
        )
        .unwrap();

    let AuthSource::XaiManagement { credential_id, .. } = &account.auth_source else {
        panic!("Grok compatibility add must use xAI Management credentials")
    };
    assert_eq!(
        store.credentials(credential_id),
        Some(credentials("team-compat"))
    );
}

#[test]
fn cursor_add_reference_creates_no_secret_file() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let database_path = sandbox.root.join("state.vscdb");
    let auth_source = AuthSource::CursorDatabase {
        database_path,
        expected_identity: "cursor@example.com".into(),
    };

    let account = store
        .add_reference_with(
            Provider::Cursor,
            "cursor".into(),
            auth_source.clone(),
            || true,
        )
        .unwrap();

    assert_eq!(account.auth_source, auth_source);
    assert_eq!(store.account(&account.id), Some(account));
    assert!(!sandbox.root.join("UsageCheck").join("credentials").exists());
}

#[test]
fn v2_reference_sources_create_no_secret_files() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store.initialize_v2().unwrap();

    store
        .add_reference_with(
            Provider::Codex,
            "codex".into(),
            AuthSource::CliProfile {
                profile_root: sandbox.root.join("codex-profile"),
                ownership: ProfileOwnership::Managed,
                expected_identity: "codex@example.com".into(),
            },
            || true,
        )
        .unwrap();
    store
        .add_reference_with(
            Provider::Cursor,
            "cursor".into(),
            AuthSource::CursorDatabase {
                database_path: sandbox.root.join("state.vscdb"),
                expected_identity: "cursor@example.com".into(),
            },
            || true,
        )
        .unwrap();
    store
        .add_reference_with(
            Provider::Higgsfield,
            "higgsfield".into(),
            AuthSource::HiggsfieldCli {
                expected_identity: "higgsfield@example.com".into(),
            },
            || true,
        )
        .unwrap();

    assert_eq!(store.list().len(), 3);
    assert!(!sandbox.root.join("UsageCheck").join("credentials").exists());
}

#[test]
fn v2_app_owned_sources_resolve_and_update_credentials_by_credential_id() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store.initialize_v2().unwrap();

    let browser = store
        .add_secret_with(
            Provider::Agy,
            "agy".into(),
            SecretSource::BrowserOAuth,
            credentials("agy-before"),
            || true,
        )
        .unwrap();
    let xai = store
        .add_secret_with(
            Provider::Grok,
            "xAI API credits".into(),
            SecretSource::XaiManagement {
                team_id: "team-1".into(),
            },
            credentials("xai"),
            || true,
        )
        .unwrap();

    let AuthSource::BrowserOAuth {
        credential_id: browser_credential_id,
    } = &browser.auth_source
    else {
        panic!("browser account must own browser credentials");
    };
    let AuthSource::XaiManagement {
        credential_id: xai_credential_id,
        team_id,
    } = &xai.auth_source
    else {
        panic!("xAI account must own management credentials");
    };
    assert_eq!(team_id, "team-1");
    assert_eq!(
        store.credentials(browser_credential_id.as_str()),
        Some(credentials("agy-before"))
    );
    assert_eq!(
        store.credentials(xai_credential_id.as_str()),
        Some(credentials("xai"))
    );

    let updated = credentials("agy-after");
    store
        .update_credentials(browser_credential_id.as_str(), &updated)
        .unwrap();
    assert_eq!(
        store.credentials(browser_credential_id.as_str()),
        Some(updated)
    );
}

#[test]
fn credential_key_uses_secret_credential_id_and_falls_back_to_account_id() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();

    let browser = store
        .add_secret_with(
            Provider::Agy,
            "agy".into(),
            SecretSource::BrowserOAuth,
            credentials("agy-key-test"),
            || true,
        )
        .unwrap();
    let AuthSource::BrowserOAuth { credential_id } = &browser.auth_source else {
        panic!("browser account must own browser credentials");
    };
    assert_eq!(
        AccountStore::credential_key(&browser),
        credential_id.as_str()
    );
    assert_ne!(AccountStore::credential_key(&browser), browser.id.as_str());

    let reference = store
        .add_reference_with(
            Provider::Codex,
            "codex".into(),
            AuthSource::CliProfile {
                profile_root: sandbox.root.join("cli-profile-test"),
                ownership: ProfileOwnership::External,
                expected_identity: "codex@example.com".into(),
            },
            || true,
        )
        .unwrap();
    assert_eq!(
        AccountStore::credential_key(&reference),
        reference.id.as_str()
    );
}

#[test]
fn credential_key_resolves_the_bug_account_id_keyed_read_fails() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let account = store
        .add_secret_with(
            Provider::Agy,
            "agy".into(),
            SecretSource::BrowserOAuth,
            credentials("agy-regression"),
            || true,
        )
        .unwrap();

    assert_eq!(store.credentials(&account.id), None);
    assert_eq!(
        store.credentials(AccountStore::credential_key(&account)),
        Some(credentials("agy-regression"))
    );
}

#[test]
fn update_credentials_by_credential_key_persists() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let account = store
        .add_secret_with(
            Provider::Agy,
            "agy".into(),
            SecretSource::BrowserOAuth,
            credentials("agy-regression-before"),
            || true,
        )
        .unwrap();
    let updated = credentials("agy-regression-updated");

    assert_eq!(
        store.update_credentials(AccountStore::credential_key(&account), &updated),
        Ok(())
    );
    assert_eq!(
        store.credentials(AccountStore::credential_key(&account)),
        Some(updated)
    );
}

#[test]
fn v2_remove_deletes_only_the_removed_accounts_app_owned_secret() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store.initialize_v2().unwrap();
    let external_profile = sandbox.root.join("external-profile");
    fs::create_dir_all(&external_profile).unwrap();
    fs::write(external_profile.join("provider-auth.json"), "keep").unwrap();

    let reference = store
        .add_reference_with(
            Provider::Claude,
            "claude".into(),
            AuthSource::CliProfile {
                profile_root: external_profile.clone(),
                ownership: ProfileOwnership::External,
                expected_identity: "claude@example.com".into(),
            },
            || true,
        )
        .unwrap();
    let first = store
        .add_secret_with(
            Provider::Agy,
            "first".into(),
            SecretSource::BrowserOAuth,
            credentials("first"),
            || true,
        )
        .unwrap();
    let second = store
        .add_secret_with(
            Provider::Agy,
            "second".into(),
            SecretSource::BrowserOAuth,
            credentials("second"),
            || true,
        )
        .unwrap();
    let AuthSource::BrowserOAuth {
        credential_id: first_credential_id,
    } = &first.auth_source
    else {
        unreachable!()
    };
    let AuthSource::BrowserOAuth {
        credential_id: second_credential_id,
    } = &second.auth_source
    else {
        unreachable!()
    };
    let first_credential_id = first_credential_id.clone();
    let second_credential_id = second_credential_id.clone();

    assert_eq!(store.remove(&reference.id).unwrap(), Some(reference));
    assert!(external_profile.join("provider-auth.json").exists());
    assert!(store.credentials(&first_credential_id).is_some());
    assert!(store.credentials(&second_credential_id).is_some());

    assert_eq!(store.remove(&first.id).unwrap(), Some(first));
    assert!(store.credentials(&first_credential_id).is_none());
    assert!(store.credentials(&second_credential_id).is_some());
    assert_eq!(store.account(&second.id), Some(second));
}

#[test]
fn v2_duplicate_profile_roots_are_rejected_without_overwrite() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store.initialize_v2().unwrap();
    let profile_root = sandbox.root.join("shared-profile");
    let first = store
        .add_reference_with(
            Provider::Codex,
            "first".into(),
            AuthSource::CliProfile {
                profile_root: profile_root.clone(),
                ownership: ProfileOwnership::External,
                expected_identity: "first@example.com".into(),
            },
            || true,
        )
        .unwrap();

    let duplicate = store.add_reference_with(
        Provider::Codex,
        "replacement".into(),
        AuthSource::CliProfile {
            profile_root,
            ownership: ProfileOwnership::Managed,
            expected_identity: "replacement@example.com".into(),
        },
        || true,
    );

    assert!(duplicate.is_err());
    assert_eq!(store.list(), vec![first]);
}

#[test]
fn v2_duplicate_cursor_identities_are_rejected_without_overwrite() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store.initialize_v2().unwrap();
    let first = store
        .add_reference_with(
            Provider::Cursor,
            "first".into(),
            AuthSource::CursorDatabase {
                database_path: sandbox.root.join("first.vscdb"),
                expected_identity: "cursor-user".into(),
            },
            || true,
        )
        .unwrap();

    let duplicate = store.add_reference_with(
        Provider::Cursor,
        "replacement".into(),
        AuthSource::CursorDatabase {
            database_path: sandbox.root.join("replacement.vscdb"),
            expected_identity: "cursor-user".into(),
        },
        || true,
    );

    assert!(duplicate.is_err());
    assert_eq!(store.list(), vec![first]);
}

#[test]
fn v2_duplicate_higgsfield_identities_are_rejected_without_overwrite() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store.initialize_v2().unwrap();
    let first = store
        .add_reference_with(
            Provider::Higgsfield,
            "first".into(),
            AuthSource::HiggsfieldCli {
                expected_identity: "hf-user-1".into(),
            },
            || true,
        )
        .unwrap();
    let second = store
        .add_reference_with(
            Provider::Higgsfield,
            "second".into(),
            AuthSource::HiggsfieldCli {
                expected_identity: "hf-user-2".into(),
            },
            || true,
        )
        .unwrap();
    let duplicate = store.add_reference_with(
        Provider::Higgsfield,
        "replacement".into(),
        AuthSource::HiggsfieldCli {
            expected_identity: "hf-user-1".into(),
        },
        || true,
    );

    assert_eq!(store.list().len(), 2);
    assert_eq!(first.label, "first");
    assert_eq!(second.label, "second");
    assert!(duplicate.is_err());
}

#[test]
fn v2_duplicate_xai_team_ids_are_rejected_without_overwrite() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store.initialize_v2().unwrap();
    let first = store
        .add_secret_with(
            Provider::Grok,
            "first".into(),
            SecretSource::XaiManagement {
                team_id: "team-1".into(),
            },
            credentials("first"),
            || true,
        )
        .unwrap();
    let AuthSource::XaiManagement { credential_id, .. } = &first.auth_source else {
        unreachable!()
    };
    let credential_id = credential_id.clone();

    let duplicate = store.add_secret_with(
        Provider::Grok,
        "replacement".into(),
        SecretSource::XaiManagement {
            team_id: "team-1".into(),
        },
        credentials("replacement"),
        || true,
    );

    assert!(duplicate.is_err());
    assert_eq!(store.list(), vec![first]);
    assert_eq!(
        store.credentials(&credential_id),
        Some(credentials("first"))
    );
    assert_eq!(
        fs::read_dir(sandbox.root.join("UsageCheck").join("credentials"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn free_cap_rejects_a_second_codex_account_and_does_not_write_it() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let first = store
        .add_secret_with(
            Provider::Codex,
            "a@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("acct-a"),
            || false,
        )
        .unwrap();

    let error = store
        .add_secret_with(
            Provider::Codex,
            "b@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("acct-b"),
            || false,
        )
        .unwrap_err();

    assert_eq!(
        error,
        usage_core::edition::free_limit_reason(Provider::Codex)
    );
    let accounts = store.list();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].id, first.id);
}

#[test]
fn free_cap_rejects_a_second_claude_cli_profile() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let first = store
        .add_reference_with(
            Provider::Claude,
            "a@example.com".into(),
            AuthSource::CliProfile {
                profile_root: sandbox.root.join("claude-a"),
                ownership: ProfileOwnership::External,
                expected_identity: "acct-a".into(),
            },
            || false,
        )
        .unwrap();

    let error = store
        .add_reference_with(
            Provider::Claude,
            "b@example.com".into(),
            AuthSource::CliProfile {
                profile_root: sandbox.root.join("claude-b"),
                ownership: ProfileOwnership::External,
                expected_identity: "acct-b".into(),
            },
            || false,
        )
        .unwrap_err();

    assert_eq!(
        error,
        usage_core::edition::free_limit_reason(Provider::Claude)
    );
    let accounts = store.list();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].id, first.id);
}

#[test]
fn free_cap_rejects_a_second_agy_account() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let first = store
        .add_secret_with(
            Provider::Agy,
            "a@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("acct-a"),
            || false,
        )
        .unwrap();

    let error = store
        .add_secret_with(
            Provider::Agy,
            "b@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("acct-b"),
            || false,
        )
        .unwrap_err();

    assert_eq!(error, usage_core::edition::free_limit_reason(Provider::Agy));
    let accounts = store.list();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].id, first.id);
}

#[test]
fn free_cap_allows_one_account_for_each_free_provider() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store
        .add_secret_with(
            Provider::Codex,
            "codex@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("codex-account"),
            || false,
        )
        .unwrap();
    store
        .add_reference_with(
            Provider::Claude,
            "claude@example.com".into(),
            AuthSource::CliProfile {
                profile_root: sandbox.root.join("claude-profile"),
                ownership: ProfileOwnership::External,
                expected_identity: "claude-account".into(),
            },
            || false,
        )
        .unwrap();
    store
        .add_secret_with(
            Provider::Agy,
            "agy@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("agy-account"),
            || false,
        )
        .unwrap();

    let accounts = store.list();
    assert_eq!(accounts.len(), 3);
    for provider in [Provider::Codex, Provider::Claude, Provider::Agy] {
        assert!(accounts.iter().any(|account| account.provider == provider));
    }
}

#[test]
fn free_cap_does_not_apply_when_pro() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    for (label, identity) in [
        ("a@example.com", "acct-a"),
        ("b@example.com", "acct-b"),
        ("c@example.com", "acct-c"),
    ] {
        store
            .add_secret_with(
                Provider::Codex,
                label.into(),
                SecretSource::BrowserOAuth,
                credentials(identity),
                || true,
            )
            .unwrap();
    }

    assert_eq!(store.list().len(), 3);
}

#[test]
fn free_cap_leaves_no_orphan_credential_file() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store
        .add_secret_with(
            Provider::Agy,
            "a@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("acct-a"),
            || false,
        )
        .unwrap();

    assert!(store
        .add_secret_with(
            Provider::Agy,
            "b@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("acct-b"),
            || false,
        )
        .is_err());
    assert_eq!(
        fs::read_dir(sandbox.root.join("UsageCheck").join("credentials"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn free_cap_promotes_the_next_account_after_removal() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let first = store
        .add_secret_with(
            Provider::Codex,
            "a@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("acct-a"),
            || true,
        )
        .unwrap();
    let second = store
        .add_secret_with(
            Provider::Codex,
            "b@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("acct-b"),
            || true,
        )
        .unwrap();

    store.remove(&first.id).unwrap();
    let error = store
        .add_secret_with(
            Provider::Codex,
            "c@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("acct-c"),
            || false,
        )
        .unwrap_err();
    assert_eq!(
        error,
        usage_core::edition::free_limit_reason(Provider::Codex)
    );

    store.remove(&second.id).unwrap();
    store
        .add_secret_with(
            Provider::Codex,
            "c@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("acct-c"),
            || false,
        )
        .unwrap();
}

#[test]
fn free_cap_removal_then_readd_succeeds() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let first = store
        .add_secret_with(
            Provider::Codex,
            "a@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("acct-a"),
            || false,
        )
        .unwrap();
    assert!(store
        .add_secret_with(
            Provider::Codex,
            "b@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("acct-b"),
            || false,
        )
        .is_err());

    store.remove(&first.id).unwrap();
    store
        .add_secret_with(
            Provider::Codex,
            "b@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("acct-b"),
            || false,
        )
        .unwrap();
    assert_eq!(store.list().len(), 1);
}

#[test]
fn duplicate_rejection_wins_over_the_free_cap() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store
        .add_secret_with(
            Provider::Codex,
            "same@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("same-account"),
            || false,
        )
        .unwrap();

    let error = store
        .add_secret_with(
            Provider::Codex,
            "same@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("same-account"),
            || false,
        )
        .unwrap_err();

    assert_eq!(error, "account already registered");
    assert_ne!(
        error,
        usage_core::edition::free_limit_reason(Provider::Codex)
    );
}

#[test]
fn duplicate_profile_root_rejection_wins_over_the_free_cap() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    let profile_root = sandbox.root.join("same-profile");
    store
        .add_reference_with(
            Provider::Claude,
            "a@example.com".into(),
            AuthSource::CliProfile {
                profile_root: profile_root.clone(),
                ownership: ProfileOwnership::External,
                expected_identity: "acct-a".into(),
            },
            || false,
        )
        .unwrap();

    let error = store
        .add_reference_with(
            Provider::Claude,
            "b@example.com".into(),
            AuthSource::CliProfile {
                profile_root,
                ownership: ProfileOwnership::Managed,
                expected_identity: "acct-b".into(),
            },
            || false,
        )
        .unwrap_err();

    assert_eq!(error, "profile root already registered");
}

#[test]
fn free_cap_rejects_an_unidentifiable_second_oauth_account() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    store
        .add_secret_with(
            Provider::Codex,
            "anonymous-a".into(),
            SecretSource::BrowserOAuth,
            Credentials {
                access_token: "token-a".into(),
                refresh_token: None,
                account_id: None,
                expires_at: None,
            },
            || false,
        )
        .unwrap();

    let error = store
        .add_secret_with(
            Provider::Codex,
            "anonymous-b".into(),
            SecretSource::BrowserOAuth,
            Credentials {
                access_token: "token-b".into(),
                refresh_token: None,
                account_id: None,
                expires_at: None,
            },
            || false,
        )
        .unwrap_err();

    assert_eq!(
        error,
        usage_core::edition::free_limit_reason(Provider::Codex)
    );
}

#[test]
fn free_cap_never_applies_to_paid_providers() {
    let sandbox = TestSandbox::new();
    let store = sandbox.store();
    for identity in ["hf-a", "hf-b", "hf-c"] {
        store
            .add_reference_with(
                Provider::Higgsfield,
                identity.into(),
                AuthSource::HiggsfieldCli {
                    expected_identity: identity.into(),
                },
                || false,
            )
            .unwrap();
    }

    assert_eq!(store.list().len(), 3);
}

#[test]
fn bare_add_reference_reads_real_license_state() {
    let sandbox = TestSandbox::new();
    let _lock = crate::license::LICENSE_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let app_data_dir = sandbox.root.join("bare-add-reference");
    let _app_data = AppDataDirGuard::set(&app_data_dir);
    let store = AccountStore::new();

    store
        .add_reference( // BARE-WRAPPER-WIRING
            Provider::Codex,
            "a@example.com".into(),
            AuthSource::CliProfile {
                profile_root: sandbox.root.join("profile-a"),
                ownership: ProfileOwnership::External,
                expected_identity: "acct-a".into(),
            },
        )
        .unwrap();
    let error = store
        .add_reference( // BARE-WRAPPER-WIRING
            Provider::Codex,
            "b@example.com".into(),
            AuthSource::CliProfile {
                profile_root: sandbox.root.join("profile-b"),
                ownership: ProfileOwnership::External,
                expected_identity: "acct-b".into(),
            },
        )
        .unwrap_err();

    assert_eq!(
        error,
        usage_core::edition::free_limit_reason(Provider::Codex)
    );
}

#[test]
fn bare_add_secret_reads_real_license_state() {
    let sandbox = TestSandbox::new();
    let _lock = crate::license::LICENSE_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let app_data_dir = sandbox.root.join("bare-add-secret");
    let _app_data = AppDataDirGuard::set(&app_data_dir);
    let store = AccountStore::new();

    store
        .add_secret( // BARE-WRAPPER-WIRING
            Provider::Codex,
            "a@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("acct-a"),
        )
        .unwrap();
    let error = store
        .add_secret( // BARE-WRAPPER-WIRING
            Provider::Codex,
            "b@example.com".into(),
            SecretSource::BrowserOAuth,
            credentials("acct-b"),
        )
        .unwrap_err();

    assert_eq!(
        error,
        usage_core::edition::free_limit_reason(Provider::Codex)
    );
}

#[test]
fn bare_add_reads_real_license_state() {
    let sandbox = TestSandbox::new();
    let _lock = crate::license::LICENSE_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let app_data_dir = sandbox.root.join("bare-add");
    let _app_data = AppDataDirGuard::set(&app_data_dir);
    let store = AccountStore::new();

    store
        .add( // BARE-WRAPPER-WIRING
            Provider::Codex,
            "a@example.com".into(),
            credentials("acct-a"),
        )
        .unwrap();
    let error = store
        .add( // BARE-WRAPPER-WIRING
            Provider::Codex,
            "b@example.com".into(),
            credentials("acct-b"),
        )
        .unwrap_err();

    assert_eq!(
        error,
        usage_core::edition::free_limit_reason(Provider::Codex)
    );
}
