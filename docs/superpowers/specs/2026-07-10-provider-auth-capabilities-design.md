# Provider Authentication Capabilities Design

**Date:** 2026-07-10

**Status:** Approved

## Goal

Make authentication and usage collection explicit per provider, keep CLI-owned
credentials under the provider CLI, and open a real terminal login only when a
CLI authentication source can subsequently produce machine-readable usage.

The patch covers Codex, Claude, Gemini/Antigravity, Cursor, Grok/xAI, and
Higgsfield in both UsageCheck editions. Existing UsageCheck account records and
app-owned credential copies may be discarded; provider-owned files are never
deleted or rewritten except for the Claude status-line integration described
below.

## Product Decisions

1. The tray menu is generated from provider capabilities instead of hard-coded
   authentication labels.
2. A CLI login option is exposed only when that CLI can feed automated usage
   back to UsageCheck.
3. CLI-backed accounts store a profile reference, not copied access or refresh
   tokens.
4. Codex and Claude use isolated CLI profile roots for additional accounts.
5. Gemini/Antigravity, Cursor, and Grok do not expose CLI login actions because
   their CLI sessions do not expose a supported machine-readable quota source.
6. Cursor remains an explicitly experimental personal-usage integration.
7. Grok is presented as **xAI API credits** so it cannot be mistaken for
   SuperGrok consumer quota.
8. Higgsfield displays remaining credits and plan. It does not invent a used
   percentage when the CLI does not provide a credit total.
9. The previous UsageCheck account store is reset instead of migrated.

## Provider Capability Matrix

| Provider | Edition | Menu authentication actions | Usage source | Multi-account behavior |
| --- | --- | --- | --- | --- |
| Codex | Free | CLI, browser | `codex app-server` account and rate-limit methods | Default CLI profile plus isolated `CODEX_HOME` profiles |
| Claude | Free | CLI, browser | Claude status-line `rate_limits` snapshots | Default CLI profile plus isolated `CLAUDE_CONFIG_DIR` profiles |
| Gemini/Antigravity | Free | Browser only | Existing Antigravity quota integration | UsageCheck browser sessions remain independently stored |
| Cursor | Pro | Local desktop database only, marked Experimental | Read-only desktop `state.vscdb` plus existing private usage RPC | One row per distinct database identity |
| Grok/xAI | Pro | Management Key clipboard or environment | Official xAI Management prepaid-balance API | One row per xAI team ID |
| Higgsfield | Pro | CLI | `higgsfield account status --json` | One global CLI account only |

The internal API provider slug remains `grok` for compatibility, while all
human-facing labels say `xAI API credits`.

## Architecture

### Capability registry

A single provider capability registry owns menu exposure and setup behavior.
It returns an ordered list of `AuthMethod` values for each compiled provider:

```text
AuthMethod::Cli
AuthMethod::BrowserOAuth
AuthMethod::LocalDatabase
AuthMethod::ManagementKeyClipboard
AuthMethod::ManagementKeyEnvironment
```

The tray menu consumes this registry. Event handlers dispatch the same
`AuthMethod` values, avoiding a second hard-coded provider/method matrix.

### Account authentication sources

The persisted account record stores an `AuthSource` instead of assuming every
account has an app-owned token file:

```text
AuthSource::CliProfile {
    profile_root,
    ownership: External | Managed,
    expected_identity,
}
AuthSource::BrowserOAuth { credential_id }
AuthSource::CursorDatabase { database_path, expected_identity }
AuthSource::XaiManagement { credential_id, team_id }
AuthSource::HiggsfieldCli
```

`BrowserOAuth` and `XaiManagement` still require an app-owned secret because
there is no CLI profile that owns those credentials. CLI profiles, Cursor
database rows, and Higgsfield never copy tokens into UsageCheck.

`expected_identity` prevents a later CLI or Cursor desktop login from silently
turning an existing UsageCheck row into another account. An identity mismatch
changes the row status to `identity_changed`; it never overwrites the stored
identity or another account.

### CLI authentication coordinator

The shared coordinator exposes provider adapters with three operations:

```text
probe(profile) -> AuthProbe
login_command(profile) -> TerminalCommand
resolve_account(profile) -> ResolvedAccount
```

Only Codex, Claude, and Higgsfield implement this interface. The capability
registry cannot expose a CLI action for a provider without an adapter.

## Account-Addition Flow

### Codex and Claude

1. Probe the provider's default CLI profile.
2. If it is authenticated and not registered, register it as an external
   profile reference.
3. If it is already registered or unauthenticated, create a UUID-named managed
   profile under the UsageCheck application-data directory.
4. Launch the provider login command in macOS Terminal or Windows PowerShell.
5. Probe the same profile every two seconds, for at most five minutes.
6. When the deterministic status check succeeds, resolve identity, register the
   account, configure its usage bridge, and refresh the tray.
7. If the deadline expires or login is cancelled, do not create an account.
   Selecting the action again starts a fresh attempt.

The commands and probes are:

| Provider | Login | Probe |
| --- | --- | --- |
| Codex | `CODEX_HOME=<profile> codex login` | `CODEX_HOME=<profile> codex login status` and exit code 0 |
| Claude | `CLAUDE_CONFIG_DIR=<profile> claude auth login --claudeai` | `CLAUDE_CONFIG_DIR=<profile> claude auth status --json`, exit code 0, `loggedIn: true` |

`CODEX_ACCESS_TOKEN`, `ANTHROPIC_API_KEY`, and `CLAUDE_CODE_OAUTH_TOKEN` are
removed from login and probe child environments so inherited process variables
cannot masquerade as the selected profile's saved login.

### Higgsfield

1. Run `higgsfield account status --json`.
2. If it returns valid JSON with a non-empty email and numeric credits, register
   or refresh the singleton Higgsfield row.
3. If it reports an unauthenticated session, launch `higgsfield auth login` in
   a terminal and apply the same two-second/five-minute probe loop.
4. Missing CLI produces `needs_setup`; an account is not created.

The primary executable name is `higgsfield`. Aliases are accepted only when
their version output identifies the Higgsfield CLI.

### Gemini/Antigravity, Cursor, and Grok/xAI

These providers never enter the CLI coordinator:

- Gemini/Antigravity retains browser login only.
- Cursor retains `Import Cursor (local, Experimental)` only.
- Grok exposes `Import xAI API credits (clipboard)` and the environment
  fallback only.

## Terminal Launching

Terminal launching is isolated behind a `TerminalLauncher` interface so tests
do not open real windows.

- macOS uses Terminal through `osascript`.
- Windows starts a visible PowerShell process.
- The launcher writes a short temporary script containing only executable
  names, profile paths, and login arguments. It never contains tokens.
- Profile paths are escaped by a platform-specific renderer, not interpolated
  with generic shell quoting.
- The script removes itself after the login process exits.
- Login stdout is informational. The coordinator trusts only the subsequent
  provider-specific probe.

If the terminal application itself cannot be launched, no account is created
and the attempt reports `terminal_error`.

## Provider Usage Adapters

### Codex

For each profile, polling starts a bounded `codex app-server --stdio` process,
initializes the protocol, reads account identity, calls
`account/rateLimits/read`, and terminates the child. The entire exchange has a
ten-second timeout. UsageCheck does not open `auth.json` or refresh tokens.

The adapter maps primary and secondary windows to the existing five-hour and
seven-day quota model. Unsupported or missing windows remain absent rather than
being fabricated.

### Claude

UsageCheck installs a status-line bridge for each registered Claude profile.
The bridge receives Claude's status-line JSON on stdin, atomically writes only
the account identity and `rate_limits` fields to an account-specific snapshot,
and never stores conversation text.

If the profile already has a status-line command, the bridge forwards the same
stdin to that command and prints its output unchanged. When an account is
removed, UsageCheck restores the previous command only if the profile still
points at the UsageCheck bridge; user changes made afterward win.

The tray reports `waiting_for_usage` after login until Claude emits a snapshot
from an API response. Snapshot reads validate the expected profile identity.

### Gemini/Antigravity

The current browser-backed Antigravity account and quota path remains. No
Gemini or `agy` CLI menu item, cached-token import, or interactive-TUI scraping
is added.

### Cursor

Cursor import stores the selected read-only `state.vscdb` path and stable
identity, not access or refresh tokens. Polling reopens the database read-only
and uses credentials only in memory for the existing experimental private RPC.

Identity is the access-token JWT subject when available, with normalized email
as the fallback. `stripeMembershipType` is plan metadata and is never an
account identifier. A different identity in the same database produces
`identity_changed` and does not mutate the registered row.

### Grok/xAI

The official Management Key validation and prepaid-balance APIs remain. Human
labels, documentation, tray actions, and detail text use `xAI API credits`.
No `grok login` action or SuperGrok quota claim is added.

### Higgsfield

Polling runs the canonical command:

```text
higgsfield account status --json
```

The parser accepts fractional numeric `credits`, a non-empty `email`, and an
optional `subscription_plan_type`. It displays remaining credits and plan.
Because the official output does not guarantee a total-credit field, the
adapter emits no used-percent quota unless a future documented schema supplies
both a total and a remaining value.

## Store Reset

The new store uses schema version 2. On first startup without the version-2
marker, UsageCheck deletes its legacy account index and its app-owned copied
credential directory, then writes an empty version-2 store.

This reset may remove all previously configured UsageCheck accounts. It never
removes provider-owned CLI profiles, macOS Keychain entries owned by provider
CLIs, Cursor databases, or browser data. The reset is idempotent.

## Status and Failure Semantics

| Status | Meaning |
| --- | --- |
| `ok` | Fresh usage was read successfully |
| `needs_setup` | Required CLI executable or local source is unavailable |
| `needs_login` | Source exists but provider reports no authenticated session |
| `waiting_for_usage` | Claude is authenticated but has not emitted a quota snapshot |
| `identity_changed` | The referenced CLI profile or Cursor DB now belongs to another account |
| `experimental_error` | Cursor's private usage integration failed |
| `stale` | A transient network/process error occurred and the last successful in-memory value is shown |
| `terminal_error` | A visible terminal could not be launched |
| `error` | Non-transient malformed data or an unclassified provider failure |

Transient failures preserve the last successful in-memory provider value and
mark it `stale`. They do not silently report zero usage. Login cancellation or
the five-minute deadline creates no account row.

## Security Boundaries

- No token is placed in a process argument, temporary login script, log, tray
  label, local HTTP response, or test fixture.
- CLI adapter probes use profile-specific environments with conflicting auth
  variables removed.
- Provider-owned credential files are not parsed for Codex, Claude, or
  Higgsfield usage polling.
- Cursor credentials remain in memory only for the duration of one poll.
- Claude snapshots contain only identity and rate-limit fields.
- The local OpenAPI service continues to expose usage only.

## Testing Strategy

All behavior changes follow red-green-refactor. Tests use temporary directories
and fake executables; they never launch a real login or expose local secrets.

### Unit tests

- Capability registry produces exactly the approved menu methods in Free and
  Pro builds.
- Codex and Claude create unique managed profile roots.
- Codex exit-code status and Claude JSON status are parsed correctly.
- Missing authentication requests a terminal launch.
- Successful re-probe registers one account; timeout registers none.
- Version-2 initialization removes only app-owned legacy account and credential
  files.
- Cursor identity prefers JWT subject, falls back to normalized email, and
  never uses plan type.
- Grok/xAI labels and actions never mention CLI login or SuperGrok quota.
- Higgsfield parses fractional credits and `subscription_plan_type` from
  `account status --json`.
- Claude bridge captures rate limits and preserves an existing status-line
  command's output.

### Integration tests

- Fake `codex`, `claude`, and `higgsfield` programs exercise the complete
  probe-login-reprobe flow through an injected fake `TerminalLauncher`.
- Fake Codex app-server JSON-RPC responses validate window mapping and timeout
  behavior.
- Temporary Claude settings and snapshots validate install, chaining, atomic
  writes, and conditional restoration.
- Temporary Cursor SQLite databases validate read-only polling, identity
  mismatch handling, and absence of token persistence.

### Build and static verification

The final verification set is:

```text
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
```

macOS receives a real terminal-launch smoke check. Windows-specific command
rendering is unit-tested locally and the visible PowerShell flow is exercised
by Windows CI before release.

## Acceptance Criteria

1. The tray exposes CLI authentication only for Codex, Claude, and Higgsfield.
2. Selecting one of those methods with no usable login opens a visible terminal
   and registers the account only after a successful deterministic re-probe.
3. Codex and Claude additional accounts remain isolated by profile root.
4. CLI-backed providers do not create UsageCheck token copies.
5. Gemini remains browser-only, Cursor remains local-only and Experimental,
   and Grok is clearly identified as xAI API credits.
6. Higgsfield uses `account status --json`, preserves fractional credits, and
   does not fabricate used percent.
7. A CLI or Cursor identity change never overwrites another stored account.
8. Legacy UsageCheck accounts reset cleanly without touching provider-owned
   files.
9. Free and Pro tests, clippy checks, and release builds pass with fresh output.
