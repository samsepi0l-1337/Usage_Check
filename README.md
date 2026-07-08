# UsageCheck

Cross-platform (macOS menu bar / Windows taskbar) usage monitor for Codex,
Claude Code, and agy (Antigravity/Gemini-family) usage, built as a Tauri app
with a shared Rust core.

The app lives in the system tray. Click the tray icon to open a small
borderless popup listing every account you've added, each with live usage
gauges.

## What It Shows

- **Multi-account**: add any number of Codex, Claude, and agy accounts side
  by side.
- **Codex** and **Claude** accounts: per-account 5-hour and 7-day quota
  gauges, fetched from each provider's usage API using the account's stored
  OAuth credentials.
- **agy** accounts: agy has no quota/usage API, so cards show local token
  totals (best-effort, scanned from local agy/Gemini CLI logs) instead of a
  percentage gauge.
- A background poll loop refreshes all accounts every 60 seconds and pushes
  updates to the popup live.

## Architecture

- `crates/usage-core` — pure Rust core: provider/account models, usage
  aggregation, provider fetchers (Codex/Claude API clients), local log
  scanners (Codex/Claude/agy), all covered by unit tests.
- `src-tauri` (`usage-app`) — the Tauri v2 shell: system tray + borderless
  popup window, PKCE OAuth login flow, an OS-keychain-backed account store,
  a background poller, and the 4 Tauri commands the UI calls
  (`list_accounts`, `add_account`, `remove_account`, `get_usage`).
- `ui/` — the popup's web frontend (TypeScript + Vite), rendering per-account
  gauges and the account picker/add flow.
- `Sources/` — the original Swift/macOS-only menu bar app. **Reference only**;
  it is not built or maintained as part of this rewrite.

## Build & Run

Prerequisites:

- Rust (stable toolchain) with `cargo`
- Node.js + npm
- [`tauri-cli`](https://tauri.app/) (`cargo install tauri-cli --version "^2"`, or use `cargo tauri` if already available)

Build the frontend once (required before a release build; `cargo tauri dev`
also expects `ui/dist` to exist or a dev server configured):

```sh
cd ui
npm install
npm run build
cd ..
```

Run in development mode (hot-reloads the Tauri shell):

```sh
cargo tauri dev
```

Build a release binary:

```sh
cargo build -p usage-app --release
```

The release binary is at `target/release/usage-app` (or the platform-specific
Tauri bundle, if using `cargo tauri build`, under `src-tauri/target/release/bundle/`).

## Verify

```sh
cargo test -p usage-core   # 16 tests — core models/aggregate/fetch/scanners/account
cargo test -p usage-app    # 17 tests — oauth/poller/store
cargo build -p usage-app --release
```

GUI, tray, OAuth, and keychain persistence behavior can't be verified
headlessly — see
[`docs/superpowers/notes/smoke-checklist.md`](docs/superpowers/notes/smoke-checklist.md)
for the manual end-to-end checklist to run on a real machine before a release.

## Multi-Account Usage

Click the tray icon to open the popup, then click **"계정 추가"** ("Add
account") to open the provider picker:

- **Codex** and **Claude**: picking either opens your system browser to that
  provider's OAuth login page (PKCE flow via a local loopback callback
  server). On success, a new account card appears and starts polling.
- **agy**: agy/Antigravity has no discoverable public OAuth flow (see
  `src-tauri/src/oauth.rs`), so picking it shows a fallback message ("agy
  OAuth unavailable — use fallback import") instead of opening a browser — no
  account is added via this path today.

Accounts can be removed individually from their card (✕). Removing an account
deletes its credentials from the OS keychain.

## Data Sources

- **Codex**: `https://chatgpt.com/backend-api/wham/usage` (quota API, using
  stored OAuth credentials) plus local log scanning of `~/.codex/sessions`
  and `~/.codex/archived_sessions` as a fallback/supplement.
- **Claude**: `https://api.anthropic.com/api/oauth/usage` (quota API, using
  stored OAuth credentials) plus local log scanning of `~/.claude/projects`
  (or `~/.config/claude/projects` / `CLAUDE_CONFIG_DIR`) as a
  fallback/supplement.
- **agy**: no usage API exists, so agy accounts rely entirely on local log
  scanning of `~/.gemini/**/transcript*.jsonl` (including Antigravity CLI
  transcripts) for token totals.

## Credential Storage

All OAuth credentials (access token, refresh token, expiry) are stored in the
OS-native credential store — macOS Keychain or Windows Credential Manager —
via the `keyring` crate, keyed per account. Nothing is written to a plaintext
config file.

## Notes

- The Codex/Claude OAuth `client_id`/`auth_url`/`token_url` values in
  `src-tauri/src/oauth.rs` are best-known public values and are marked
  `// TODO: verify` pending a live login confirmation — see the smoke
  checklist for the required validation step.
- The old Swift app under `Sources/` (and its `Package.swift`/`Tests/`) is
  kept for reference only and is superseded by the Tauri app described above.
