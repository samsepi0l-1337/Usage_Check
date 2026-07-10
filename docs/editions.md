# UsageCheck Free vs Pro Editions

UsageCheck ships as **two separate binaries** (Free and Pro), not a single
binary unlocked at runtime. Edition choice is made at **compile time** via
Cargo features and Tauri bundle config. v0.1.4 introduced this split.

For the high-level product overview, see the [README](../README.md). For
cross-platform tray architecture and the original rewrite plan, see
[`docs/superpowers/specs/2026-07-08-usagecheck-crossplatform-design.md`](superpowers/specs/2026-07-08-usagecheck-crossplatform-design.md).

## Product split

| | **Free** | **Pro** |
| --- | --- | --- |
| Product name | `UsageCheck-Free` | `UsageCheck-Pro` |
| Bundle ID | `com.usagecheck.desktop.free` | `com.usagecheck.desktop.pro` |
| Cargo feature | `edition-free` (default) | `edition-pro` |
| Tauri config | `src-tauri/tauri.conf.json` | `src-tauri/tauri.pro.conf.json` (overrides name + ID) |
| Providers | Codex, Claude, Gemini (agy) | Free providers + Cursor, Grok, Higgsfield |

**Gemini** is not a separate `Provider` enum variant. It is implemented as
`Provider::Agy` (Antigravity), which polls the Antigravity **Gemini Models**
quota pool (and Claude+GPT pool) via `RetrieveUserQuotaSummary`.

**Licensing:** Pro is a separate artifact with additional provider modules
compiled in. There is **no online license server**; distribution is by
edition-specific installers from CI.

## Provider matrix

| Provider | Edition | Import path | Data source | Tray display |
| --- | --- | --- | --- | --- |
| **Codex** | Free | Browser OAuth or CLI import | `chatgpt.com` usage API + local logs | 5h / 7d used % |
| **Claude** | Free | Browser OAuth or CLI import | Anthropic OAuth usage API + local logs | 5h / 7d used % |
| **Gemini (agy)** | Free | Browser OAuth (Antigravity) | Antigravity `RetrieveUserQuotaSummary` | Gemini + Claude+GPT pools as used % |
| **Cursor** | Pro | **Import Cursor (local)** — reads `state.vscdb` | Undocumented Connect RPC `GetCurrentPeriodUsage` on `api2.cursor.sh` | Billing-period used % + optional `$ left` |
| **Grok (xAI)** | Pro | **Import Grok (clipboard)** — paste Management Key; optional **Import Grok (env vars)** | xAI Management API prepaid balance | Spend-since-top-up used % + `$ left` |
| **Higgsfield** | Pro | **Login Higgsfield (browser)** or **Import Higgsfield (CLI)** | `higgsfield account --json` subprocess | Credits used % + `N credits left` |

### Pro provider setup

#### Cursor

1. Sign in to the Cursor desktop app.
2. In UsageCheck Pro, tray menu → **Add account** → **Import Cursor (local)**.
3. The app reads (read-only) from Cursor's SQLite `state.vscdb`:

   - macOS: `~/Library/Application Support/Cursor/User/globalStorage/state.vscdb`
   - Windows: `%APPDATA%/Cursor/User/globalStorage/state.vscdb`

4. Keys read: `cursorAuth/accessToken`, `cursorAuth/refreshToken`,
   `cursorAuth/cachedEmail`, `cursorAuth/stripeMembershipType`.
5. Polling calls
   `POST https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage`
   with Connect protocol headers. Tokens refresh via
   `POST https://api2.cursor.sh/oauth/token` when a refresh token is present.
6. Local tokens are re-synced from `state.vscdb` on each poll when the
   identity matches.

#### Grok (xAI API credits)

1. Obtain an xAI **Management API key** from xAI Console → Settings → Management Keys.
2. Copy the key to your clipboard.
3. Tray menu → **Add account** → **Import Grok (clipboard)**.
4. UsageCheck validates the key via
   `GET https://management-api.x.ai/auth/management-keys/validation` and
   resolves your team ID from `scopeId` (no `XAI_TEAM_ID` required when
   validation succeeds).
5. Polling hits
   `GET https://management-api.x.ai/v1/billing/teams/{team_id}/prepaid/balance`.
6. Used % is computed from ledger `changes` (spend since last `PURCHASE` /
   `AUTO_PURCHASE`). Remaining balance appears as a detail suffix.

**Fallbacks** (when validation cannot resolve team ID):

- Paste the Management Key and team ID on **separate lines** in the clipboard
  before import, or set `XAI_TEAM_ID` and use clipboard import with key only.
- **Import Grok (env vars)** — set `XAI_MGMT_KEY` (or `XAI_MANAGEMENT_KEY`)
  and `XAI_TEAM_ID` in the environment before importing.

#### Higgsfield

1. Install the [Higgsfield CLI](https://higgsfield.ai) and ensure `higgsfield`
   is on your `PATH`.
2. Tray menu → **Add account** → **Login Higgsfield (browser)**.
   This runs `higgsfield auth login` (system browser device flow) and, on
   success, imports `~/.config/higgsfield/credentials.json` automatically.
3. Alternatively, run `higgsfield auth login` in a terminal yourself, then
   choose **Import Higgsfield (CLI)** to re-import credentials.
4. Each poll runs `higgsfield account --json` and parses flexible JSON
   shapes for `credits` / `credits_total`.
5. If the CLI is missing, login/import fails with a clear message; polling
   status is **`needs_setup`** when the CLI is unavailable or JSON has no
   recognizable credit fields.

## Architecture

Edition gating is **compile-time** (`#[cfg(feature = "edition-pro")]`).
Free builds omit Pro provider variants, fetch modules, and tray menu items
entirely.

```
crates/usage-core/
  src/edition.rs          # edition_id(), free_providers(), paid_providers(), all_providers()
  src/account.rs          # Provider enum (Cursor/Grok/Higgsfield behind edition-pro)
  src/paid.rs             # Pro-only re-exports (edition-pro only)
  src/fetch/
    cursor.rs             # parse GetCurrentPeriodUsage JSON
    grok.rs               # parse prepaid balance JSON
    higgsfield.rs         # parse account --json credits

src-tauri/
  src/edition.rs          # product_name(), re-exports all_providers()
  src/cursor_local.rs     # read-only state.vscdb import (edition-pro)
  src/import.rs           # load_grok_env_auth(), import_grok_from_clipboard(),
                          # run_higgsfield_browser_login(), load_higgsfield_cli_auth()
  src/poller.rs           # poll_cursor, poll_grok, poll_higgsfield
  src/tray_menu.rs        # Pro import menu items
  src/main.rs             # add-cursor-local / add-grok-clipboard / add-grok-env /
                          # add-higgsfield-login / add-higgsfield-cli handlers
  tauri.conf.json         # UsageCheck-Free defaults
  tauri.pro.conf.json     # UsageCheck-Pro productName + identifier override
```

### Cargo features

`edition-free` and `edition-pro` are **mutually exclusive**. Enabling both
triggers a `compile_error!` in `crates/usage-core/src/edition.rs`.

| Crate | Default features | Edition flags |
| --- | --- | --- |
| `usage-core` | `edition-free` | `edition-free`, `edition-pro` |
| `usage-app` (`src-tauri`) | `custom-protocol`, `edition-free` | `edition-free` → `usage-core/edition-free`; `edition-pro` → `usage-core/edition-pro` |

Plain `cargo build -p usage-app --release` produces the **Free** edition.

## Known limitations

| Area | Limitation |
| --- | --- |
| **Cursor** | Uses an **undocumented** internal API (`GetCurrentPeriodUsage`). Cursor may change or break it without notice. No official public quota API. |
| **Grok** | Shows **prepaid API credit** balance and spend-since-top-up %. There is **no SuperGrok weekly quota %** — that subscription tier is not modeled. |
| **Higgsfield** | **CLI-dependent** for live polling (`higgsfield` must be on `PATH`). Import only needs `credentials.json`; polling needs the CLI. Unrecognized JSON → `needs_setup`. |
| **Runtime unlock** | No feature flag or license server toggles Pro at runtime — you must install the Pro binary. |
| **Local API** | `GET /v1/usage/{provider}` documents `codex` \| `claude` \| `agy` only; Pro providers appear in the full `/v1/usage` snapshot when running Pro. |

## Build and release

### Local builds

Use the edition build script (creates `ui/dist` placeholder, then runs
`cargo tauri build` with the correct features):

```sh
./scripts/build-edition.sh free --bundles dmg,app    # macOS Free
./scripts/build-edition.sh pro  --bundles dmg,app    # macOS Pro
./scripts/build-edition.sh free --bundles nsis,msi     # Windows Free
./scripts/build-edition.sh pro  --bundles nsis,msi     # Windows Pro
```

Equivalent manual invocations:

```sh
cd src-tauri
cargo tauri build --no-default-features --features custom-protocol,edition-free
cargo tauri build --no-default-features --features custom-protocol,edition-pro \
  --config tauri.pro.conf.json
```

### CI release matrix

GitHub Actions workflow: [`.github/workflows/release.yml`](../.github/workflows/release.yml)

Triggered by:

- `workflow_dispatch` (manual)
- Push of tags matching `v*` (e.g. `v0.1.4`)

| Matrix job | Platform | Edition | Upload artifact name |
| --- | --- | --- | --- |
| `macos-free` | `macos-15` | Free | `UsageCheck-Free-macos` (`.dmg` + `.app`) |
| `macos-pro` | `macos-15` | Pro | `UsageCheck-Pro-macos` (`.dmg` + `.app`) |
| `windows-free` | `windows-latest` | Free | `UsageCheck-Free-windows` (`.exe` + `.msi`) |
| `windows-pro` | `windows-latest` | Pro | `UsageCheck-Pro-windows` (`.exe` + `.msi`) |

On tag pushes, the `release` job publishes all four artifact sets to a
GitHub Release (`softprops/action-gh-release`).

macOS jobs verify ad-hoc code signature (`signingIdentity: "-"`) and fail if
the bundle is linker-signed only.

### Verify both editions

```sh
cargo test -p usage-core --no-default-features --features edition-free
cargo test -p usage-core --no-default-features --features edition-pro
cargo test -p usage-app --no-default-features --features custom-protocol,edition-pro
```

## 한국어 요약

- **Free**: Codex, Claude, Gemini(agy) — 기본 빌드.
- **Pro**: Free 제공자 + Cursor, Grok, Higgsfield — 별도 바이너리(`UsageCheck-Pro`).
- 런타임 라이선스 없음; CI에서 에디션별 설치 파일을 배포.
- Pro 계정 추가: Cursor 로컬 DB, Grok 클립보드(또는 환경 변수), Higgsfield 브라우저 로그인/CLI.
- 자세한 빌드: `./scripts/build-edition.sh free|pro`.
