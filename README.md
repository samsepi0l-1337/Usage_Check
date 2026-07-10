# UsageCheck

Cross-platform (macOS menu bar / Windows taskbar) usage monitor for Codex,
Claude Code, and agy (Antigravity/Gemini-family) usage, built as a Tauri app
with a shared Rust core.

The app lives in the system tray / menu bar only (no separate popup window).
Click the tray icon to open a native menu (Docker-style) with live usage
rows, Add/Remove account actions, Refresh, and Quit.

## What It Shows

- **Multi-account**: add any number of Codex, Claude, and agy accounts.
- **Codex** and **Claude**: 5-hour and 7-day quota percentages in the tray
  menu, fetched from each provider's usage API.
- **agy**: Antigravity Model Quota (Gemini / Claude+GPT pools) as used % in
  the tray menu.
- **UsageCheck Pro** adds **Cursor**, **Grok (xAI API credits)**, and
  **Higgsfield** (credits via CLI when available).
- A background poll refreshes the tray menu every 60 seconds.

## Free vs Pro editions

| Edition | Providers | Build |
| ------- | --------- | ----- |
| **Free** (default) | Codex, Claude, agy (Gemini/Antigravity) | `./scripts/build-edition.sh free` |
| **Pro** | Free providers + Cursor, Grok, Higgsfield | `./scripts/build-edition.sh pro` |

Pro-only import paths:

- **Cursor** — reads `cursorAuth/accessToken` from Cursor's local
  `state.vscdb` (read-only) and calls the undocumented
  `GetCurrentPeriodUsage` Connect RPC on `api2.cursor.sh`. This API may
  change without notice.
- **Grok** — xAI Management API prepaid balance (`XAI_MGMT_KEY` +
  `XAI_TEAM_ID` env vars at import time).
- **Higgsfield** — imports `~/.config/higgsfield/credentials.json` after
  `higgsfield auth login`; polls via `higgsfield account --json` when the
  CLI is installed. Shows `needs_setup` when the CLI or JSON shape is
  unavailable.

Plain `cargo build` produces the **Free** edition. Pro builds use
`--no-default-features --features custom-protocol,edition-pro` (see
`scripts/build-edition.sh`).

**License note:** Pro is a separate binary artifact (`UsageCheck-Pro`) with
additional provider modules compiled in. There is no online license server;
distribution is by edition-specific installers from CI.

## Architecture

- `crates/usage-core` — pure Rust core: provider/account models, usage
  aggregation, provider fetchers (Codex/Claude API clients), local log
  scanners (Codex/Claude/agy), all covered by unit tests.
- `src-tauri` (`usage-app`) — Tauri v2 tray shell: native menu bar menu,
  PKCE OAuth, file-backed account store under Application Support /
  `%APPDATA%`, background poller, CLI credential import (including Claude
  Keychain on macOS).
- `ui/` — legacy Vite frontend (unused by the tray-menu shell; kept for
  optional future UI work).
- `Sources/` — the original Swift/macOS-only menu bar app. **Reference only**;
  it is not built or maintained as part of this rewrite.

## Build & Run

Prerequisites:

- Rust (stable toolchain) with `cargo`
- Node.js + npm
- Optional: [`tauri-cli`](https://tauri.app/) for installer bundles
  (`cargo install tauri-cli --version "^2"`)

The tray-menu shell does not need the Vite UI. Build and run:

```sh
cargo build -p usage-app --release
./target/release/usage-app          # macOS / Linux
# target\release\usage-app.exe      # Windows
```

### Packaged installers (DMG / EXE)

Install the Tauri CLI once:

```sh
cargo install tauri-cli --version "^2"
```

**macOS DMG** (on a Mac):

```sh
mkdir -p ui/dist && printf '%s\n' '<!doctype html><html><body></body></html>' > ui/dist/index.html
cd src-tauri
cargo tauri build --bundles dmg,app
# → target/release/bundle/dmg/UsageCheck_*.dmg
# → target/release/bundle/macos/UsageCheck.app
```

macOS builds use an ad-hoc code signature (`bundle.macOS.signingIdentity = "-"`)
so Gatekeeper does not treat the downloaded app as damaged. Without an Apple
Developer ID / notarization, the first open may still show “Apple could not
verify…”. Use **System Settings → Privacy & Security → Open Anyway**, or:

```sh
xattr -cr /Applications/UsageCheck.app
```

**Windows EXE / MSI** must be built on Windows (or via CI). From this repo:

```sh
# GitHub Actions: Actions → Release → Run workflow
# Artifacts: UsageCheck-windows (NSIS .exe + .msi)
```

Local Windows build:

```sh
cd src-tauri
cargo tauri build --bundles nsis,msi
# → target/release/bundle/nsis/UsageCheck_*-setup.exe
# → target/release/bundle/msi/UsageCheck_*.msi
```

Accounts are stored under:

- macOS: `~/Library/Application Support/UsageCheck/`
- Windows: `%APPDATA%/UsageCheck/`

## Verify

```sh
cargo test -p usage-core   # core models/aggregate/fetch/scanners/account
cargo test -p usage-app    # oauth/poller/store/import/paths
cargo build -p usage-app --release

# Both editions (mutually exclusive features):
cargo test -p usage-core --no-default-features --features edition-free
cargo test -p usage-core --no-default-features --features edition-pro
cargo test -p usage-app --no-default-features --features custom-protocol,edition-pro
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

## Local API (for other agents / MCP / skills)

While the tray app is running it also serves a read-only **local HTTP API** so
other coding agents (Codex, Claude Code, agy, Cursor, ...) can consume the same
usage data — no tray-UI scraping. It is published from the same 60-second poll
snapshot the tray renders, so it never makes extra provider calls per request.

- Binds **`127.0.0.1` only** (never exposed off-host); read-only (GET only).
- **Never** returns access tokens, refresh tokens, or other credentials.
- Enabled by default. Disable with `USAGECHECK_API_DISABLE=1`.
- Default port `5178`; override with `USAGECHECK_API_PORT=<port>`.

Endpoints:

| Method & path             | Description                                   |
| ------------------------- | --------------------------------------------- |
| `GET /health`             | Service status, version, last-updated, count  |
| `GET /v1/usage`           | Usage snapshot for all accounts               |
| `GET /v1/usage/{provider}`| Filtered to `codex` \| `claude` \| `agy`      |
| `GET /openapi.yaml`       | The OpenAPI 3.1 spec for this API             |

All quota figures are **used percent** (`0` = unused, `100` = exhausted),
matching the tray (agy `remainingFraction` is already converted). The full
contract — schemas, examples, error shapes — lives in
[`docs/openapi.yaml`](docs/openapi.yaml) and is served live at `/openapi.yaml`.

Try it:

```sh
curl -s http://127.0.0.1:5178/health
curl -s http://127.0.0.1:5178/v1/usage | jq
curl -s http://127.0.0.1:5178/v1/usage/codex | jq
```

An MCP server or agent skill can wrap this by fetching `/v1/usage` (or a
per-provider path) and surfacing `accounts[].five_hour.used_percent`,
`week.used_percent`, and agy `pools[]`. Generate a typed client straight from
the served spec, e.g.:

```sh
curl -s http://127.0.0.1:5178/openapi.yaml -o usagecheck-openapi.yaml
# feed usagecheck-openapi.yaml to your OpenAPI client/codegen of choice
```

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
