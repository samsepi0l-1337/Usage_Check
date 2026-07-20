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
- **UsageCheck Pro** adds **Cursor** (local, Experimental), **Grok** (xAI API
  management-key credits, not consumer SuperGrok), and **Higgsfield**
  (credits via CLI).
- A background poll refreshes the tray menu every 60 seconds (override with
  `USAGECHECK_POLL_SECS=<seconds>`, clamped to 15–3600).

## Free vs Pro editions

UsageCheck ships as **two compile-time editions** (v0.1.4+), not a single
binary unlocked at runtime. Deep reference:
[`docs/editions.md`](docs/editions.md).

| Edition | Product name | Bundle ID | Providers | Build |
| ------- | ------------ | --------- | --------- | ----- |
| **Free** (default) | `UsageCheck-Free` | `com.usagecheck.desktop.free` | Codex, Claude, agy (Gemini/Antigravity) | `./scripts/build-edition.sh free` |
| **Pro** | `UsageCheck-Pro` | `com.usagecheck.desktop.pro` | Free + Cursor, Grok, Higgsfield | `./scripts/build-edition.sh pro` |

**Gemini** is `Provider::Agy` (Antigravity Gemini Models quota), not a
separate enum.

Pro-only import paths (tray → Add Account), exact menu labels from
`auth_action_specs()`:

- **Import Cursor (local, Experimental)** — read-only reference to the local
  Cursor `state.vscdb` via an undocumented private RPC
  (`GetCurrentPeriodUsage` on `api2.cursor.sh`). Experimental: not an
  officially supported integration.
- **Import xAI API credits (clipboard)** — copy an xAI **Management Key** to
  the clipboard, then import (validates via xAI API; team ID resolved
  automatically). Fallback: **Import xAI API credits (env vars)** with
  `XAI_MGMT_KEY` + `XAI_TEAM_ID`. This is xAI API management-key credit
  usage, not consumer SuperGrok quota.
- **Add Higgsfield (CLI)** — pure CLI reference via `higgsfield account status --json`
  (no credential file read). Status is `needs_setup` when the CLI or its JSON
  output is unavailable.

Plain `cargo build` produces the **Free** edition. Pro builds use
`--no-default-features --features custom-protocol,edition-pro` and
`tauri.pro.conf.json` (see `scripts/build-edition.sh`).

**License note:** Pro is a separate binary with additional provider modules
compiled in. There is no online license server; distribution is by
edition-specific installers from CI.

**CI releases:** push a `v*` tag (e.g. `v0.1.4`) or run the Release workflow
manually. Artifacts: `UsageCheck-Free-macos`, `UsageCheck-Pro-macos`,
`UsageCheck-Free-windows`, `UsageCheck-Pro-windows`. See
[`docs/editions.md`](docs/editions.md#ci-release-matrix).

## Architecture

- `crates/usage-core` — pure Rust core: provider/account models, usage
  aggregation, provider fetchers (Codex/Claude API clients; Pro:
  Cursor/Grok/Higgsfield parsers), local log scanners (Codex/Claude/agy),
  edition helpers (`edition.rs`), all covered by unit tests.
- `src-tauri` (`usage-app`) — Tauri v2 tray shell: native menu bar menu,
  PKCE OAuth, file-backed account store under Application Support /
  `%APPDATA%`, background poller, CLI credential import (including Claude
  Keychain on macOS; Pro: Cursor `state.vscdb`, Grok clipboard/env, Higgsfield
  pure CLI reference).
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

The account store uses a file-backed **schema-v2** index (`accounts-v2.json`).
This is a fresh schema, not a migration: on first run under schema v2, any
legacy single-token account index and credentials file at the same
app-data root are reset (deleted), not converted. Accounts must be re-added
after upgrading from a pre-schema-v2 build.

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

Click the tray icon, then **Add Account** to open the provider submenu. Menu
labels below are the exact strings from `auth_action_specs()` in
`src-tauri/src/tray_menu.rs`.

- **Login Codex (browser)** / **Login Claude (browser)** — opens the system
  browser for that provider's OAuth login (PKCE via a local loopback
  callback, app-owned client credential). On success, a new account card
  appears and starts polling.
- **Add Codex (CLI)** / **Add Claude (CLI)** — CLI profile isolation, not a
  token copy. UsageCheck opens the system terminal and runs the provider's
  own login command (`codex login` / `claude auth login --claudeai`) into an
  app-managed, isolated profile directory (`CODEX_HOME` / `CLAUDE_CONFIG_DIR`)
  and registers a **reference** to that profile — no access/refresh token is
  extracted or stored by UsageCheck. Codex usage then comes from a live probe
  of the Codex app-server (`codex app-server --stdio`) against that profile.
  Claude usage comes from a status-line bridge UsageCheck installs into the
  profile's `settings.json`; a freshly added Claude CLI account shows
  **`waiting_for_usage`** until you run `claude` yourself and it renders its
  status line at least once, which is what emits the first quota sample.
- **Login Antigravity (browser)** — agy/Antigravity has no CLI import; this
  is the only way to add it. It registers `Provider::Agy`, whose quota model
  is the Antigravity Model Quota (Gemini and Claude+GPT pools) as used %, not
  a standalone Gemini API.

### Status meanings

The tray shows one of these per account (see `src-tauri/src/poller.rs`):

- `ok` — live quota fetched successfully.
- `needs_login` — no usable credentials; re-add the account.
- `rate_limited` — provider returned HTTP 429.
- `error` — provider call failed for another reason.
- `stale` — a transient failure occurred, so the last known-good quota is
  shown instead of clearing it.
- `waiting_for_usage` — a CLI-profile Claude account whose profile hasn't
  emitted a status-line usage sample yet.
- `identity_changed` — the CLI profile's logged-in identity no longer matches
  the identity UsageCheck registered.
- `needs_setup` — the CLI or its JSON output is unavailable (e.g. Higgsfield
  CLI not installed).

When any account is at or above the alert threshold (default 90%, configurable
via `USAGECHECK_ALERT_THRESHOLD`), a **⚠ N account(s) near limit** banner
appears at the top of the tray menu.

The tray menu also shows an informational **`Updated HH:MM:SS`** row (the last
poll time, local clock), a **`<Product Name> v<version>`** row
(`UsageCheck-Free` or `UsageCheck-Pro` depending on edition) and, unless the
API is disabled, an **Open Usage API** item that opens the local API index
(`http://127.0.0.1:<port>/`) in your browser.

Right-click (or use the tray menu) → **Quit UsageCheck** to exit. Accounts can
be removed individually via the tray's **Remove** submenu; removing a CLI
Claude account also tears down the status-line bridge from that profile's
`settings.json`.

## Data Sources

Codex and Claude each have two account types with different data sources:

- **Codex, browser OAuth account**: `https://chatgpt.com/backend-api/wham/usage`
  (quota API, using the app-owned stored OAuth credentials) plus local log
  scanning of `~/.codex/sessions` and `~/.codex/archived_sessions` as a
  fallback/supplement.
- **Codex, CLI account** (`Add Codex (CLI)`): live probe of the Codex
  app-server (`codex app-server --stdio`) against the isolated `CODEX_HOME`
  profile UsageCheck manages — no HTTP usage API call, no token held by
  UsageCheck.
- **Claude, browser OAuth account**: `https://api.anthropic.com/api/oauth/usage`
  (quota API, using the app-owned stored OAuth credentials) plus local log
  scanning of `~/.claude/projects` (or `~/.config/claude/projects` /
  `CLAUDE_CONFIG_DIR`) as a fallback/supplement.
- **Claude, CLI account** (`Add Claude (CLI)`): a status-line bridge
  UsageCheck installs into the isolated profile's `settings.json`; usage
  appears once `claude` renders its status line in that profile (see
  `waiting_for_usage` above).
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
- Optional bearer-token auth: set `USAGECHECK_API_TOKEN=<token>` to require
  `Authorization: Bearer <token>` on every endpoint except the
  liveness/discovery paths (`/health`, `/`) — i.e. `/v1/*`, `/metrics`, and
  `/openapi.yaml`. Unset ⇒ open (localhost-only bind is the default protection).
- `GET /v1/alerts` near-limit threshold defaults to `90`%; override with
  `USAGECHECK_ALERT_THRESHOLD=<percent>` (clamped 0–100).

Endpoints:

| Method & path             | Description                                   |
| ------------------------- | --------------------------------------------- |
| `GET /health`             | Status, version, last-updated, snapshot age, per-status counts |
| `GET /v1/usage`           | Usage snapshot for all accounts               |
| `GET /v1/usage/{provider}`| Filtered to `codex` \| `claude` \| `agy`      |
| `GET /v1/accounts`        | Inventory: id/provider/name/status/`auth_kind` |
| `GET /v1/alerts`          | Windows at/above the alert threshold (near limit) |
| `GET /v1/usage.csv`       | Flat CSV: one row per account/window (+pools) |
| `GET /metrics`            | Prometheus text-format `used_percent` gauges  |
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
curl -s http://127.0.0.1:5178/v1/usage.csv    # CSV for spreadsheets
curl -s http://127.0.0.1:5178/metrics        # Prometheus scrape target
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

Browser-OAuth accounts (`Login … (browser)`) store their OAuth credentials
(access token, refresh token, expiry) in the OS-native credential store —
macOS Keychain or Windows Credential Manager — via the `keyring` crate, keyed
per account. Nothing is written to a plaintext config file.

CLI accounts (`Add Codex/Claude (CLI)`) do **not** have their tokens copied
into UsageCheck at all — UsageCheck stores only a reference to the isolated
CLI profile directory (`CODEX_HOME` / `CLAUDE_CONFIG_DIR`) and reads usage
from that profile in place.

## Notes

- The Codex/Claude OAuth `client_id`/`auth_url`/`token_url` values in
  `src-tauri/src/oauth.rs` are best-known public values and are marked
  `// TODO: verify` pending a live login confirmation — see the smoke
  checklist for the required validation step.
- The old Swift app under `Sources/` (and its `Package.swift`/`Tests/`) is
  kept for reference only and is superseded by the Tauri app described above.
