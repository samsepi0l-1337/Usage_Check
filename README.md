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
- Optional: [`tauri-cli`](https://tauri.app/) for installer bundles
  (`cargo install tauri-cli --version "^2"`)

Build the frontend once (required before a release build; `cargo tauri dev`
also expects `ui/dist` to exist or a dev server configured):

```sh
cd ui
npm install
npm run build
cd ..
```

Run the release binary directly (macOS / Windows, after a native build):

```sh
cargo build -p usage-app --release
./target/release/usage-app          # macOS / Linux
# target\release\usage-app.exe      # Windows
```

Important: build with the default features (includes `custom-protocol`) so the
UI is embedded from `ui/dist`. A release binary built *without*
`custom-protocol` loads `http://localhost:5173` and shows a blank white popup
when the Vite dev server is not running. Always run `npm run build` in `ui/`
before `cargo build --release` (or use `cargo tauri build`, which runs
`beforeBuildCommand` for you).

Also ensure Vite uses relative asset paths (`base: "./"` in `ui/vite.config.ts`)
so CSS/JS resolve under Tauri's custom protocol.

Or use the Tauri CLI for a packaged app (`.app` / `.msi` / `.exe` installer):

```sh
cargo tauri build
```

Development mode (hot-reloads the Tauri shell when `tauri-cli` is installed):

```sh
cargo tauri dev
```

**Windows:** build on a Windows host. The tray/WebView shell is not
cross-compiled from macOS. The same Cargo workspace and `ui/` frontend are
used on both platforms.

## Verify

```sh
cargo test -p usage-core   # core models/aggregate/fetch/scanners/account
cargo test -p usage-app    # oauth/poller/store/import/paths
cargo build -p usage-app --release
```

On macOS the release binary is `target/release/usage-app`. On Windows, build
on a Windows host the same way (or use `cargo tauri build` for an installer
bundle). Cross-compiling the tray/WebView shell from macOS to Windows is not
supported out of the box.

GUI, tray, OAuth, and keychain persistence behavior can't be verified
headlessly — see
[`docs/superpowers/notes/smoke-checklist.md`](docs/superpowers/notes/smoke-checklist.md)
for the manual end-to-end checklist to run on a real machine before a release.

## Multi-Account Usage

Click the tray icon to open the popup, then click **"계정 추가"** ("Add
account") to open the provider picker:

- **Codex** / **Claude** — **브라우저 로그인**: opens the system browser for
  that provider's OAuth login (PKCE via a local loopback callback). On
  success, a new account card appears and starts polling.
- **Codex** / **Claude** — **CLI에서 가져오기**: imports tokens already stored
  by the CLI (`~/.codex/auth.json`, or `$CODEX_HOME/auth.json`; Claude's
  `.credentials.json` under `~/.claude` / `$CLAUDE_CONFIG_DIR`). Useful when
  you are already logged in via `codex login` / `claude` and do not want a
  second browser flow.
- **agy** — **로컬 로그로 추가**: agy has no public OAuth or quota API, so this
  registers a local-log-only account that shows 5h/7d token totals scanned
  from `~/.gemini` (and `~/.config/gemini`).

Right-click (or use the tray menu) → **Quit UsageCheck** to exit. Accounts can
be removed individually from their card (✕); that also deletes credentials
from the OS keychain.

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
