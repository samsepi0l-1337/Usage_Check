# UsageCheck providers and the Pro license

UsageCheck ships as **one binary** for everyone. Codex, Claude, and agy
(Gemini/Antigravity) are free. A **Pro license key** unlocks Cursor, Grok,
and Higgsfield at **runtime** — there is no separate Free/Pro binary, no
compile-time edition Cargo feature, and no `tauri.pro.conf.json` override.
This replaces the two-binary/compile-time-edition split UsageCheck used
before the runtime license gate landed.

For the high-level product overview, see the [README](../README.md). For the
signed-token activation wire contract, see
[`docs/LICENSE_API.md`](LICENSE_API.md). For cross-platform tray architecture
and the original rewrite plan, see
[`docs/superpowers/specs/2026-07-08-usagecheck-crossplatform-design.md`](superpowers/specs/2026-07-08-usagecheck-crossplatform-design.md).

## Product identity

| | |
| --- | --- |
| Product name | `UsageCheck` |
| Bundle ID | `com.usagecheck.desktop` |
| Config | `src-tauri/tauri.conf.json` (the only Tauri config — no per-edition override file) |
| Providers | Codex, Claude, Gemini (agy) always; Cursor, Grok, Higgsfield once a Pro license is active |

**Gemini** is not a separate `Provider` enum variant. It is implemented as
`Provider::Agy` (Antigravity), which polls the Antigravity **Gemini Models**
quota pool (and Claude+GPT pool) via `RetrieveUserQuotaSummary`.

## Licensing

A Pro license key is activated from the tray's license section (near **Add
Account** / **Remove**):

- **License status row** — `License: Free`, `License: Pro`,
  `License: Pro · expires <relative>`, `License: expired — reactivate`, or
  `License: verification needed` (offline grace period elapsed).
- **Last attempt row** — appears only after an activation attempt this run;
  shows a coarse classification (`activated`, `no network`, `invalid key`,
  `server not available yet`, …), never server-provided text, the key, or
  the token.
- **Activate from clipboard** — copy your license key, then click; reads the
  clipboard the same way **Import xAI API credits (clipboard)** does.
- **Deactivate license** — shown once Pro is active or a (even
  expired/broken) record is stored; removes the persisted record.
- **Get a license…** — opens `https://autoworkit.com/`.

Under the hood: activation POSTs the key to the license API, which returns
an **Ed25519-signed token** bound to this device. The token — never a
separate mutable "is Pro" flag — is persisted and its signature is
re-verified on every status check, so a hand-edited cache file fails closed
instead of silently granting Pro. Full wire contract, offline-grace and
clock-rollback handling: [`docs/LICENSE_API.md`](LICENSE_API.md).

## Provider matrix

| Provider | Requires Pro | Import path | Data source | Tray display |
| --- | --- | --- | --- | --- |
| **Codex** | No | **Login Codex (browser)** (OAuth) or **Add Codex (CLI)** (isolated CLI profile) | Browser: `chatgpt.com` usage API + local logs. CLI: live `codex app-server --stdio` probe of the managed profile | 5h / 7d used % |
| **Claude** | No | **Login Claude (browser)** (OAuth) or **Add Claude (CLI)** (isolated CLI profile) | Browser: Anthropic OAuth usage API + local logs. CLI: status-line bridge installed into the managed profile (`waiting_for_usage` until first sample) | 5h / 7d used % |
| **Gemini (agy)** | No | **Login Antigravity (browser)** — no CLI import | Antigravity Model Quota (`RetrieveUserQuotaSummary`) | Gemini + Claude+GPT pools as used % |
| **Cursor** | Yes | **Import Cursor (local, Experimental)** — reads `state.vscdb` | Undocumented Connect RPC `GetCurrentPeriodUsage` on `api2.cursor.sh` | Billing-period used % + optional `$ left` |
| **Grok (xAI)** | Yes | **Import xAI API credits (clipboard)** — paste Management Key; optional **Import xAI API credits (env vars)** | xAI Management API prepaid balance (not consumer SuperGrok) | Spend-since-top-up used % + `$ left` |
| **Higgsfield** | Yes | **Add Higgsfield (CLI)** | `higgsfield account status --json` subprocess | Credits used % + `N credits left` |

A Pro-gated provider is hidden from the **Add Account** menu (and its
account, if one somehow exists, renders `pro_required`) until a Pro license
is active — see `usage_core::edition::requires_pro` and the runtime gate in
`src-tauri/src/tray_menu/actions.rs` / `src-tauri/src/poller/mod.rs`.

### Pro provider setup

#### Cursor (Experimental)

This integration is **Experimental** — it depends on an undocumented private
RPC and Cursor's local SQLite layout, both of which Cursor can change without
notice. It is read-only and never writes to Cursor's database.

1. Sign in to the Cursor desktop app.
2. Activate a Pro license (tray → license section → **Activate from
   clipboard**), then tray → **Add Account** → **Import Cursor (local,
   Experimental)**.
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

#### Grok (xAI API management-key credits)

This is xAI **API** management-key credit usage — not consumer SuperGrok
subscription quota. There is no SuperGrok integration.

1. Obtain an xAI **Management API key** from xAI Console → Settings → Management Keys.
2. Copy the key to your clipboard.
3. Tray menu → **Add Account** → **Import xAI API credits (clipboard)**.
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
- **Import xAI API credits (env vars)** — set `XAI_MGMT_KEY` (or
  `XAI_MANAGEMENT_KEY`) and `XAI_TEAM_ID` in the environment before
  importing.

#### Higgsfield

1. Install the [Higgsfield CLI](https://higgsfield.ai) and ensure `higgsfield`
   is on your `PATH`.
2. Run `higgsfield auth login` in a terminal yourself first — there is no
   in-app browser login for Higgsfield.
3. Tray menu → **Add Account** → **Add Higgsfield (CLI)** creates a pure CLI
   reference via `higgsfield account status --json` (no credential file read).
4. Each poll runs `higgsfield account status --json` and parses flexible
   JSON shapes for `credits` / `credits_total`.
5. If the CLI is missing, import fails with a clear message; polling status
   is **`needs_setup`** when the CLI is unavailable or JSON has no
   recognizable credit fields.

## Architecture

Provider gating is a **runtime check**, not a compile-time feature — every
binary contains every provider; `usage_core::edition::requires_pro` and
`crate::license::is_pro()` decide what's visible/dispatchable.

```
crates/usage-core/
  src/edition.rs          # all_providers(), requires_pro()
  src/account.rs          # Provider enum (all variants always compiled in)
  src/fetch/
    cursor.rs             # parse GetCurrentPeriodUsage JSON
    grok.rs               # parse prepaid balance JSON
    higgsfield.rs         # parse account --json credits

src-tauri/
  src/edition.rs          # product_name(), re-exports all_providers()
  src/license/            # signed-token activation, status, offline grace (docs/LICENSE_API.md)
  src/cursor_local.rs     # read-only state.vscdb import
  src/import/             # load_grok_env_auth(), import_grok_from_clipboard(),
                          # load_higgsfield_cli_auth()
  src/poller/             # poll_cursor, poll_grok, poll_higgsfield; is_pro()-gated dispatch
  src/tray_menu/          # auth_action_specs() (is_pro()-filtered); license tray section
  src/menu_actions.rs     # handle_menu_event(): add-cursor-local / add-grok-clipboard /
                          # add-grok-env / add-higgsfield-cli / license-activate-clipboard /
                          # license-deactivate / license-get
  tauri.conf.json         # the single UsageCheck config
```

### Cargo features

`usage-app`'s only feature is `custom-protocol` (Tauri's embedded-frontend
mode), on by default. There is no edition feature and no
`--no-default-features` build variant to choose between.

## Known limitations

| Area | Limitation |
| --- | --- |
| **Cursor** | **Experimental.** Uses an **undocumented** private RPC (`GetCurrentPeriodUsage`). Cursor may change or break it without notice. No official public quota API. |
| **Grok** | Shows **xAI API management-key prepaid credit** balance and spend-since-top-up %. This is **not** consumer SuperGrok — there is no SuperGrok weekly quota % and that subscription tier is not modeled. |
| **Higgsfield** | **Pure CLI reference** via `higgsfield account status --json` (no credential file read). Login happens via the CLI, not in-app. Unrecognized JSON → `needs_setup`. |
| **Claude CLI accounts** | Usage depends on a status-line bridge installed into the isolated profile; a newly added Claude CLI account shows `waiting_for_usage` until `claude` is run in that profile and renders its status line at least once. |
| **Offline grace** | A Pro license verified once keeps working offline for 14 days (`license::OFFLINE_GRACE`); beyond that (or on a detected clock rollback) the tray shows `License: verification needed` until the next successful online refresh. |
| **Local API** | `GET /v1/usage/{provider}` documents `codex` \| `claude` \| `agy` only; Pro providers appear in the full `/v1/usage` snapshot once a Pro license is active. |

## Build and release

### Local builds

```sh
./scripts/build-edition.sh --bundles dmg,app    # macOS
./scripts/build-edition.sh --bundles nsis,msi   # Windows
```

Equivalent manual invocation:

```sh
cd src-tauri
cargo tauri build --features custom-protocol --bundles dmg,app   # or nsis,msi
```

### CI release matrix

GitHub Actions workflow: [`.github/workflows/release.yml`](../.github/workflows/release.yml)

Triggered by:

- `workflow_dispatch` (manual)
- Push of tags matching `v*` (e.g. `v0.1.34`)

A `guard` job runs first and fails the whole workflow if the embedded
license Ed25519 public key (`src-tauri/src/license/pubkey.rs`) is still the
placeholder — see that file and `pubkey_tests.rs` for the mechanism. Once it
passes:

| Matrix job | Platform | Upload artifact name |
| --- | --- | --- |
| `macos` | `macos-15` | `UsageCheck-macos` (`.dmg` + `.app`) |
| `windows` | `windows-latest` | `UsageCheck-windows` (`.exe` + `.msi`) |

On tag pushes, the `release` job publishes the **3 unified installers**
(macOS `.dmg`, Windows `.exe` + `.msi`) to a GitHub Release
(`softprops/action-gh-release`).

macOS jobs verify ad-hoc code signature (`signingIdentity: "-"`) and fail if
the bundle is linker-signed only.

### Verify

```sh
cargo test -p usage-core
cargo test -p usage-app
cargo build -p usage-app --release
```

## 한국어 요약

- UsageCheck는 **단일 바이너리**입니다. Codex, Claude, Gemini(agy)는 무료.
- Pro 라이선스 키를 활성화하면 런타임에 Cursor, Grok, Higgsfield가 열립니다 (별도 바이너리 없음).
- 트레이 메뉴 → 라이선스 섹션 → **Activate from clipboard**로 키 등록,
  **Deactivate license**로 해제, **Get a license…**로 구매 페이지 열기.
- 활성화는 Ed25519 서명 토큰(디바이스 바인딩, 오프라인 유예 14일)으로 검증됩니다 — 자세한 내용은 `docs/LICENSE_API.md`.
