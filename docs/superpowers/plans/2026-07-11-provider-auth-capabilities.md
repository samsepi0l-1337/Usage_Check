# Provider Authentication Capabilities Implementation Plan

> **For Codex:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` to implement this plan task-by-task. Every behavior change must follow `superpowers:test-driven-development`.

**Goal:** Replace token-copy CLI imports with capability-driven provider authentication so only Codex, Claude, and Higgsfield expose CLI login, while Gemini, Cursor, and xAI retain their supported non-CLI sources.

**Architecture:** Persist an explicit `AuthSource` per account, generate tray actions from one capability registry, and route CLI setup through an injected terminal launcher plus provider-specific probes. Codex usage comes from `codex app-server`, Claude usage from a status-line bridge, Cursor credentials remain in memory for one poll, xAI keeps app-owned Management Keys, and Higgsfield reads `account status --json` without inventing percentages.

**Tech Stack:** Rust 2021, Tauri v2, Tokio, Serde/serde_json, rusqlite, reqwest, platform process APIs, Cargo feature editions.

**Approved design:** `docs/superpowers/specs/2026-07-10-provider-auth-capabilities-design.md`

## Global Constraints

- The tray exposes CLI authentication only for Codex, Claude, and Higgsfield.
- Codex login is `CODEX_HOME=<profile> codex login`; probe is `CODEX_HOME=<profile> codex login status` with `CODEX_ACCESS_TOKEN` and `OPENAI_API_KEY` removed.
- Claude login is `CLAUDE_CONFIG_DIR=<profile> claude auth login --claudeai`; probe is `CLAUDE_CONFIG_DIR=<profile> claude auth status --json` with `ANTHROPIC_API_KEY` and `CLAUDE_CODE_OAUTH_TOKEN` removed.
- Higgsfield login is `higgsfield auth login`; both setup and polling probe with `higgsfield account status --json`.
- Gemini/Antigravity is browser-only. Cursor is local-database-only and marked Experimental. Grok is human-labeled `xAI API credits` and uses Management Key clipboard/environment only.
- CLI profiles, Cursor databases, and Higgsfield never create UsageCheck token copies. Browser OAuth and xAI Management Keys remain app-owned secrets.
- CLI and Cursor identity mismatches report `identity_changed`; they never mutate the registered identity.
- Missing CLI/source reports `needs_setup`; unauthenticated sources report `needs_login`; a terminal launch failure reports `terminal_error`; Claude without a snapshot reports `waiting_for_usage`; Cursor private-RPC failure reports `experimental_error`; transient poll failures preserve the last successful in-memory value and report `stale`.
- Legacy UsageCheck account data may be reset. The reset may remove only the app-owned legacy index and copied-credential directory; provider-owned profiles, Keychain entries, Cursor databases, and browser data are never deleted.
- No token appears in process arguments, temporary scripts, logs, tray labels, local HTTP responses, or test fixtures. Claude snapshots contain only identity and rate-limit fields.
- Existing unrelated `AGENTS.md` changes are user-owned and must not be edited or staged.
- Existing in-scope Pro edits in `Cargo.lock`, `README.md`, `crates/usage-core/src/fetch/grok.rs`, `src-tauri/Cargo.toml`, `src-tauri/src/import.rs`, `src-tauri/src/main.rs`, `src-tauri/src/tray_menu.rs`, and `docs/editions.md` must be integrated rather than reverted.

## Baseline

Fresh baseline on 2026-07-11:

```text
cargo test -p usage-core
21 passed

cargo test -p usage-app --no-default-features --features custom-protocol,edition-pro
68 passed, 2 ignored
```

The app baseline has one existing warning for unused `higgsfield_cli_available`; the completed patch must be warning-free under the clippy commands in Task 9.

### Task 1: Introduce the provider capability and authentication-source model

**Files:**

- Modify: `crates/usage-core/src/account.rs`
- Create: `crates/usage-core/src/capabilities.rs`
- Modify: `crates/usage-core/src/lib.rs`
- Modify fixtures in: `src-tauri/src/store.rs`, `src-tauri/src/tray_menu.rs`, `src-tauri/src/api.rs`, `src-tauri/src/poller.rs`

**Step 1: Write failing core tests**

Add tests proving:

```rust
assert_eq!(auth_capability(Provider::Codex).methods,
           &[AuthMethod::Cli, AuthMethod::BrowserOAuth]);
assert_eq!(auth_capability(Provider::Claude).methods,
           &[AuthMethod::Cli, AuthMethod::BrowserOAuth]);
assert_eq!(auth_capability(Provider::Agy).methods,
           &[AuthMethod::BrowserOAuth]);
#[cfg(feature = "edition-pro")]
assert_eq!(auth_capability(Provider::Cursor).methods,
           &[AuthMethod::LocalDatabase]);
#[cfg(feature = "edition-pro")]
assert_eq!(auth_capability(Provider::Grok).methods,
           &[AuthMethod::ManagementKeyClipboard,
             AuthMethod::ManagementKeyEnvironment]);
#[cfg(feature = "edition-pro")]
assert_eq!(auth_capability(Provider::Higgsfield).methods,
           &[AuthMethod::Cli]);
```

Add an `Account` JSON round-trip test for each `AuthSource` variant and a provider display-name test requiring `Provider::Grok.display_name() == "xAI API credits"`.

**Step 2: Verify RED**

Run:

```bash
cargo test -p usage-core --no-default-features --features edition-pro capabilities
```

Expected: compilation fails because `AuthMethod`, `AuthSource`, and `auth_capability` do not exist.

**Step 3: Implement the minimum model**

Add:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AuthMethod {
    Cli,
    BrowserOAuth,
    LocalDatabase,
    ManagementKeyClipboard,
    ManagementKeyEnvironment,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ProfileOwnership { External, Managed }

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthSource {
    CliProfile {
        profile_root: PathBuf,
        ownership: ProfileOwnership,
        expected_identity: String,
    },
    BrowserOAuth { credential_id: String },
    CursorDatabase { database_path: PathBuf, expected_identity: String },
    XaiManagement { credential_id: String, team_id: String },
    HiggsfieldCli { expected_identity: String },
}
```

Add `auth_source: AuthSource` to `Account`, implement the exact capability matrix, export it, and update existing test fixtures with semantically valid sources. Do not add a generic CLI variant for Gemini, Cursor, or xAI.

**Step 4: Verify GREEN**

Run both edition variants:

```bash
cargo test -p usage-core --no-default-features --features edition-free
cargo test -p usage-core --no-default-features --features edition-pro
cargo test -p usage-app --no-default-features --features custom-protocol,edition-pro --no-run
```

Expected: all tests pass and the app fixtures compile.

**Step 5: Commit**

```bash
git add crates/usage-core/src/account.rs crates/usage-core/src/capabilities.rs crates/usage-core/src/lib.rs src-tauri/src/store.rs src-tauri/src/tray_menu.rs src-tauri/src/api.rs src-tauri/src/poller.rs
git commit -m "refactor: model provider authentication sources"
```

### Task 2: Replace the legacy token-assuming store with schema v2

**Files:**

- Rewrite: `src-tauri/src/store.rs`
- Modify: `src-tauri/src/paths.rs`

**Step 1: Write failing filesystem tests**

Use UUID-named directories under `std::env::temp_dir()`. Tests must prove:

1. Missing v2 marker deletes only legacy `accounts.json` and `credentials/`.
2. A sibling sentinel and an external provider profile survive reset.
3. Re-running initialization is idempotent.
4. `CliProfile`, `CursorDatabase`, and `HiggsfieldCli` accounts create no secret file.
5. `BrowserOAuth` and `XaiManagement` accounts can resolve their app-owned `Credentials` by `credential_id`.
6. Removing an account deletes only its referenced app-owned secret.
7. Duplicate profile roots, Cursor identities, and xAI team IDs are rejected without overwriting another row.

**Step 2: Verify RED**

```bash
cargo test -p usage-app --no-default-features --features custom-protocol,edition-pro store::tests::v2_
```

Expected: tests fail because `AccountStore` has no injectable root, no v2 marker, and assumes every account owns credentials.

**Step 3: Implement schema v2**

Change the zero-sized store to:

```rust
#[derive(Clone, Debug)]
pub struct AccountStore { root: PathBuf }

impl AccountStore {
    pub fn new() -> Self;
    #[cfg(test)] pub fn new_at(root: PathBuf) -> Self;
    pub fn initialize_v2(&self) -> Result<(), String>;
    pub fn list(&self) -> Vec<Account>;
    pub fn account(&self, id: &str) -> Option<Account>;
    pub fn add_reference(&self, provider: Provider, label: String,
                         auth_source: AuthSource) -> Result<Account, String>;
    pub fn add_secret(&self, provider: Provider, label: String,
                      source: SecretSource, credentials: Credentials)
                      -> Result<Account, String>;
    pub fn credentials(&self, credential_id: &str) -> Option<Credentials>;
    pub fn update_credentials(&self, credential_id: &str,
                              credentials: &Credentials) -> Result<(), String>;
    pub fn remove(&self, account_id: &str) -> Result<Option<Account>, String>;
}
```

Use an explicit marker such as `schema-v2` and a v2 index file such as `accounts-v2.json`. `initialize_v2()` must never traverse or delete paths outside `root`. Remove token/account-ID dedupe and token-copy synchronization from the store contract.

**Step 4: Verify GREEN**

```bash
cargo test -p usage-app --no-default-features --features custom-protocol,edition-pro store::tests
```

Expected: all store tests pass.

**Step 5: Commit**

```bash
git add src-tauri/src/store.rs src-tauri/src/paths.rs
git commit -m "refactor: add source-aware account store v2"
```

### Task 3: Generate auth actions from capabilities and add terminal/coordinator foundations

**Files:**

- Modify: `src-tauri/src/tray_menu.rs`
- Create: `src-tauri/src/terminal.rs`
- Create: `src-tauri/src/cli_auth.rs`
- Modify: `src-tauri/src/main.rs` only to register modules and compile test helpers; defer runtime dispatch to Task 8

**Step 1: Write failing pure tests**

Add tests asserting the exact ordered Pro action labels:

```text
Add Codex (CLI)
Login Codex (browser)
Add Claude (CLI)
Login Claude (browser)
Login Antigravity (browser)
Import Cursor (local, Experimental)
Import xAI API credits (clipboard)
Import xAI API credits (env vars)
Add Higgsfield (CLI)
```

Assert no action contains `Gemini (CLI)`, `Antigravity (CLI)`, `Cursor (CLI)`, `Grok (CLI)`, `SuperGrok`, or `Higgsfield (browser)`.

Add renderer tests for paths containing spaces and apostrophes on macOS and Windows. Add coordinator tests with fake probes and a fake launcher for:

- authenticated unregistered default profile -> register without launch;
- unauthenticated default profile -> unique managed profile -> one launch -> successful re-probe;
- launch error -> `terminal_error`, no account;
- deadline -> no account;
- missing executable -> `needs_setup`, no launch.

Use an injected retry schedule in tests so the five-minute production deadline does not sleep.

**Step 2: Verify RED**

```bash
cargo test -p usage-app --no-default-features --features custom-protocol,edition-pro auth_action_specs
cargo test -p usage-app --no-default-features --features custom-protocol,edition-pro terminal::tests
cargo test -p usage-app --no-default-features --features custom-protocol,edition-pro cli_auth::tests
```

Expected: missing registry, terminal launcher, and coordinator APIs.

**Step 3: Implement minimal foundations**

Add a pure `AuthActionSpec { provider, method, event_id, label }` registry consumed by both menu construction and event parsing.

Add:

```rust
pub struct TerminalCommand {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub env_remove: Vec<OsString>,
}

pub trait TerminalLauncher {
    fn launch(&self, command: &TerminalCommand) -> Result<(), TerminalError>;
}
```

The macOS implementation uses a UUID temporary shell script and Terminal through `osascript`; Windows uses a UUID temporary PowerShell script and `CREATE_NEW_CONSOLE`. The scripts contain only executable/profile/argument data, quote by platform, self-delete after the login command exits, and never contain credentials.

Add a provider-adapter trait/enum boundary implementing `probe`, `login_command`, and `resolve_account`. Production retry interval is two seconds and deadline is five minutes.

**Step 4: Verify GREEN**

Run the three targeted commands above. Expected: all pass without opening a real terminal.

**Step 5: Commit**

```bash
git add src-tauri/src/tray_menu.rs src-tauri/src/terminal.rs src-tauri/src/cli_auth.rs src-tauri/src/main.rs
git commit -m "feat: add capability-driven CLI setup coordinator"
```

### Task 4: Implement Codex profile probing and app-server usage

**Files:**

- Create: `src-tauri/src/codex_cli.rs`
- Modify: `crates/usage-core/src/fetch/codex.rs`
- Modify: `crates/usage-core/src/fetch/mod.rs`
- Modify: `src-tauri/src/cli_auth.rs`
- Modify: `src-tauri/src/paths.rs`

**Step 1: Write failing tests**

Tests must cover:

- default `CODEX_HOME`, and unique app-managed roots under `profiles/codex/<uuid>`;
- probe exit 0 vs exit 1 while clearing `CODEX_ACCESS_TOKEN` and `OPENAI_API_KEY`;
- JSONL notifications interleaved before matching response IDs;
- `account/read` chatgpt identity and rejection of null/API-key accounts;
- `account/rateLimits/read` mapping `usedPercent`, `windowDurationMins * 60`, and `resetsAt` into primary/secondary quota windows;
- missing windows remain `None`;
- child timeout/EOF/non-zero exits become process errors and never fabricate zero quota.

Use a fake executable or deterministic in-memory JSONL harness. Do not read a real `~/.codex/auth.json`.

**Step 2: Verify RED**

```bash
cargo test -p usage-app --no-default-features --features custom-protocol,edition-free codex_cli::tests
cargo test -p usage-core --no-default-features --features edition-free fetch::codex::tests::app_server
```

Expected: missing adapter and parser behavior.

**Step 3: Implement app-server exchange**

Launch `codex app-server --stdio` with the account's `CODEX_HOME`, cleared conflicting variables, `kill_on_drop(true)`, and a ten-second total timeout. Write newline-delimited requests in this order:

```json
{"method":"initialize","id":1,"params":{"clientInfo":{"name":"usagecheck","title":"UsageCheck","version":"0.1.4"},"capabilities":null}}
{"method":"initialized"}
{"method":"account/read","id":2,"params":{"refreshToken":true}}
{"method":"account/rateLimits/read","id":3}
```

Read by matching `id`, ignore notifications and unknown fields, validate the expected identity on every poll, then terminate the child. Do not parse or refresh copied tokens.

**Step 4: Verify GREEN**

Run the two targeted commands above. Expected: all pass.

**Step 5: Commit**

```bash
git add src-tauri/src/codex_cli.rs src-tauri/src/cli_auth.rs src-tauri/src/paths.rs crates/usage-core/src/fetch/codex.rs crates/usage-core/src/fetch/mod.rs
git commit -m "feat: read Codex usage from isolated CLI profiles"
```

### Task 5: Implement Claude profile probing and the status-line bridge

**Files:**

- Create: `src-tauri/src/claude_cli.rs`
- Create: `src-tauri/src/claude_statusline.rs`
- Modify: `src-tauri/src/cli_auth.rs`
- Modify: `src-tauri/src/paths.rs`
- Modify: `src-tauri/src/main.rs`

**Step 1: Write failing tests**

Tests must cover:

- default `CLAUDE_CONFIG_DIR`, and unique managed roots under `profiles/claude/<uuid>`;
- `claude auth status --json` exit 0 plus `loggedIn: true` parsing;
- identity uses non-empty `orgId`, then normalized email fallback; plan uses `subscriptionType`;
- probe/login environments clear both conflicting variables;
- bridge installation preserves the complete previous `statusLine` object;
- bridge snapshot stores only expected identity and `rate_limits.five_hour`/`seven_day`;
- an existing command receives byte-identical stdin and its stdout is returned unchanged;
- restore occurs only while the current command is still the UsageCheck bridge;
- no snapshot maps to `waiting_for_usage`; identity mismatch maps to `identity_changed`.

Use temporary `settings.json`, snapshot files, and fake child commands. No local Claude credential source may be read.

**Step 2: Verify RED**

```bash
cargo test -p usage-app --no-default-features --features custom-protocol,edition-free claude_cli::tests
cargo test -p usage-app --no-default-features --features custom-protocol,edition-free claude_statusline::tests
```

Expected: bridge and CLI parser APIs do not exist.

**Step 3: Implement the bridge**

Before Tauri startup, recognize:

```text
usage-app --claude-statusline-bridge <account-id>
```

The helper reads stdin once, extracts only identity and rate-limit fields, writes the snapshot atomically, chains the preserved command with identical stdin, writes only the chained stdout, and exits without starting Tauri. Installation writes a UsageCheck `statusLine` command to the selected profile's `settings.json`; sidecar metadata stores the complete prior object. Removal uses conditional restoration so later user edits win.

**Step 4: Verify GREEN**

Run the two targeted commands above. Expected: all pass.

**Step 5: Commit**

```bash
git add src-tauri/src/claude_cli.rs src-tauri/src/claude_statusline.rs src-tauri/src/cli_auth.rs src-tauri/src/paths.rs src-tauri/src/main.rs
git commit -m "feat: collect Claude quota through CLI status line"
```

### Task 6: Make Cursor a read-only Experimental database reference

**Files:**

- Modify: `src-tauri/src/cursor_local.rs`
- Modify: `src-tauri/src/import.rs`
- Modify: `src-tauri/src/poller.rs`
- Modify: `src-tauri/src/store.rs`

**Step 1: Write failing tests**

Create temporary SQLite databases and synthetic non-secret JWT payloads. Prove:

- identity prefers JWT `sub`;
- absent `sub` falls back to trimmed lowercase email;
- `stripeMembershipType` is plan metadata, never identity;
- import persists `CursorDatabase { database_path, expected_identity }` and creates no secret file;
- poll reopens the database read-only and keeps tokens only in a local session value;
- changed identity reports `identity_changed` without modifying the account;
- private RPC failure reports `experimental_error`;
- an existing last-success value becomes `stale` for transient failures, not zero.

**Step 2: Verify RED**

```bash
cargo test -p usage-app --no-default-features --features custom-protocol,edition-pro cursor_local::tests
cargo test -p usage-app --no-default-features --features custom-protocol,edition-pro poller::tests::cursor_
```

Expected: current import stores tokens and plan-as-account-ID, so persistence/identity assertions fail.

**Step 3: Implement the reference adapter**

Expose:

```rust
pub struct CursorSession {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub identity: String,
}

pub fn read_cursor_session(path: &Path) -> Result<CursorSession, CursorLocalError>;
```

Open SQLite read-only. Store only path and expected identity. Polling may use credentials in memory for the existing RPC but must not call `update_credentials` for Cursor.

**Step 4: Verify GREEN**

Run the two targeted commands above. Expected: all pass.

**Step 5: Commit**

```bash
git add src-tauri/src/cursor_local.rs src-tauri/src/import.rs src-tauri/src/poller.rs src-tauri/src/store.rs
git commit -m "refactor: keep Cursor credentials in its local database"
```

### Task 7: Finalize xAI Management Keys and canonical Higgsfield CLI usage

**Files:**

- Modify: `crates/usage-core/src/fetch/grok.rs`
- Modify: `crates/usage-core/src/fetch/higgsfield.rs`
- Modify: `src-tauri/src/import.rs`
- Modify: `src-tauri/src/poller.rs`
- Modify: `src-tauri/src/cli_auth.rs`
- Modify: `src-tauri/src/tray_menu.rs`
- Modify: `src-tauri/Cargo.toml`
- Modify: `Cargo.lock`

**Step 1: Write failing tests**

For xAI, test Management Key paste/environment parsing, team-ID dedupe, `AuthSource::XaiManagement`, and exact human label `xAI API credits`. Assert there is no CLI action or `SuperGrok` claim.

For Higgsfield, test:

```json
{"email":"person@example.com","credits":12.75,"subscription_plan_type":"Creator"}
```

Expected parser result preserves `12.75`, plan `Creator`, and emits `five_hour == None`, `week == None`, with detail text `12.75 credits remaining`. Test invalid/missing email, nonnumeric credits, unauthenticated probe, login/re-probe, singleton registration, and no app-owned secret file.

**Step 2: Verify RED**

```bash
cargo test -p usage-core --no-default-features --features edition-pro fetch::higgsfield::tests
cargo test -p usage-app --no-default-features --features custom-protocol,edition-pro import::tests::xai_
cargo test -p usage-app --no-default-features --features custom-protocol,edition-pro cli_auth::tests::higgsfield_
```

Expected: current parser truncates credits/derives a percentage, and current Higgsfield import copies credential files or exposes a browser path.

**Step 3: Implement approved Pro sources**

Preserve the existing official xAI Management Key validation and prepaid-balance fetcher, but store it through `XaiManagement { credential_id, team_id }`. Keep internal provider slug `grok`.

Replace all Higgsfield credential-file/browser import code with canonical CLI probe/login. Parse `credits` as `f64` from number or numeric string and optional `subscription_plan_type`. Remove total-credit and used-percent fabrication. Make `rusqlite` and `arboard` optional dependencies included by `edition-pro` so Free builds do not pull Pro-only integrations.

**Step 4: Verify GREEN**

Run the three targeted commands above plus:

```bash
cargo test -p usage-app --no-default-features --features custom-protocol,edition-free --no-run
```

Expected: all pass and Free compiles without Pro dependencies.

**Step 5: Commit**

```bash
git add Cargo.lock src-tauri/Cargo.toml crates/usage-core/src/fetch/grok.rs crates/usage-core/src/fetch/higgsfield.rs src-tauri/src/import.rs src-tauri/src/poller.rs src-tauri/src/cli_auth.rs src-tauri/src/tray_menu.rs
git commit -m "feat: align Pro providers with supported auth sources"
```

### Task 8: Wire setup, removal, polling cache, and API status end to end

**Files:**

- Modify: `src-tauri/src/main.rs`
- Modify: `src-tauri/src/poller.rs`
- Modify: `src-tauri/src/store.rs`
- Modify: `src-tauri/src/tray_menu.rs`
- Modify: `src-tauri/src/api.rs`
- Modify: `docs/openapi.yaml`

**Step 1: Write failing integration tests**

Test the common action dispatcher rather than provider-specific duplicated event IDs. Prove:

- only registry actions are accepted;
- successful CLI setup registers once and refreshes the menu;
- login timeout/cancel registers nothing;
- transient setup notices expose `needs_setup` or `terminal_error` as a disabled tray row;
- Claude account removal conditionally restores its bridge before deleting the row;
- `poll_all` routes by `(provider, auth_source)` and never invokes token sync for CLI/Cursor/Higgsfield;
- a last-success cache returns old quota with `status == "stale"` after a transient error;
- API DTO includes `detail_suffix` but serializes none of `auth_source`, `profile_root`, `credential_id`, Management Key, or Cursor token;
- provider filtering accepts `cursor`, `grok`, and `higgsfield` in Pro builds.

**Step 2: Verify RED**

```bash
cargo test -p usage-app --no-default-features --features custom-protocol,edition-pro main::tests
cargo test -p usage-app --no-default-features --features custom-protocol,edition-pro poller::tests::auth_source_
cargo test -p usage-app --no-default-features --features custom-protocol,edition-pro api::tests
```

Expected: runtime still dispatches a hard-coded matrix, startup still dedupes/imports ambient CLI tokens, and DTO lacks detail/status coverage.

**Step 3: Complete runtime wiring**

At startup, run `initialize_v2()` and manage a cloneable store plus last-success cache and transient setup-notice state. Dispatch all menu authentication through `(Provider, AuthMethod)` from the registry. Browser OAuth and xAI continue through app-owned secret records; Cursor uses a DB reference; CLI actions use the coordinator.

Remove ambient Codex/Claude import/sync and any Higgsfield credential parser. Route poll by `AuthSource`, validate expected identity before usage, and preserve only in-memory last success on transient failures. Add `detail_suffix` and the full provider/status enums to the OpenAPI DTO/spec while keeping authentication metadata private.

**Step 4: Verify GREEN**

Run the three targeted commands above. Expected: all pass.

**Step 5: Commit**

```bash
git add src-tauri/src/main.rs src-tauri/src/poller.rs src-tauri/src/store.rs src-tauri/src/tray_menu.rs src-tauri/src/api.rs docs/openapi.yaml
git commit -m "feat: route provider auth by capability"
```

### Task 9: Synchronize documentation and perform full verification

**Files:**

- Modify: `README.md`
- Modify: `docs/editions.md`
- Modify if necessary: `docs/openapi.yaml`
- Modify only for discovered regressions: nearest source/test files

**Step 1: Add documentation assertions before prose changes**

Add or extend a lightweight Rust/doc-contract test that reads `README.md` and `docs/editions.md` and asserts:

- CLI is listed only for Codex, Claude, and Higgsfield;
- Gemini says browser only;
- Cursor says local and Experimental;
- xAI says Management Key/API credits and never says Grok CLI or SuperGrok quota;
- Higgsfield command is `higgsfield account status --json`.

**Step 2: Verify RED**

Run the new focused test. Expected: current docs still describe the previous Pro/Higgsfield paths.

**Step 3: Update docs minimally**

Document source ownership, destructive v2 account reset, CLI profile isolation, exact menu labels, status meanings, and the limitation that Claude shows `waiting_for_usage` until its configured profile emits status-line quota data. Do not claim official support for private Cursor RPC behavior.

**Step 4: Run the complete fresh verification matrix**

Run independent edition commands in parallel where Cargo locks permit:

```bash
cargo test -p usage-core --no-default-features --features edition-free
cargo test -p usage-core --no-default-features --features edition-pro
cargo test -p usage-app --no-default-features --features custom-protocol,edition-free
cargo test -p usage-app --no-default-features --features custom-protocol,edition-pro
cargo clippy -p usage-core --no-default-features --features edition-free -- -D warnings
cargo clippy -p usage-core --no-default-features --features edition-pro -- -D warnings
cargo clippy -p usage-app --no-default-features --features custom-protocol,edition-free -- -D warnings
cargo clippy -p usage-app --no-default-features --features custom-protocol,edition-pro -- -D warnings
cargo build -p usage-app --release --no-default-features --features custom-protocol,edition-free
cargo build -p usage-app --release --no-default-features --features custom-protocol,edition-pro
git diff --check
```

Then perform a macOS terminal-render/launch smoke using a harmless temporary command; do not invoke a real provider login. Inspect the generated script to confirm it contains no ambient token value. Windows visible-PowerShell behavior remains CI-gated, but its exact renderer tests must pass locally.

**Step 5: Broad review and fix loop**

Generate a whole-branch review package from the branch base, request spec and code-quality review, fix all Critical/Important findings with focused regression tests, and re-run affected checks. Confirm `AGENTS.md` remains unstaged and unchanged by this implementation.

**Step 6: Commit**

```bash
git add README.md docs/editions.md docs/openapi.yaml <nearest-test-files>
git commit -m "docs: explain provider-owned authentication"
```

## Completion Evidence

The final handoff must include:

- branch and commit list;
- changed provider-auth behavior by provider;
- fresh test, clippy, and release-build results for Free and Pro;
- macOS harmless terminal smoke result and Windows CI limitation;
- confirmation that no provider-owned credential/profile file was modified or deleted;
- confirmation that `AGENTS.md` was not staged or committed;
- any remaining risk, especially Codex app-server schema drift, Claude status-line emission prerequisites, and Cursor private API instability.
