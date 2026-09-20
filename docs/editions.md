# UsageCheck providers and the Pro license

> **Status: Pro activation is NOT available in the current release.** The
> licensing service is not live, and shipped builds embed the documented
> placeholder verification key, so every activation attempt fails and no
> license key unlocks Cursor, Grok, Higgsfield, Kimi, OpenCode Go, DeepSeek, OpenRouter, GitHub Copilot, Windsurf, MiniMax, Augment, Poe, Fireworks, Novita, Amp, Z.AI or Alibaba Token Plan — for anyone. Codex, Claude
> and agy remain free, with one active account per provider in the unlicensed
> Free state. The runtime gate preserves already-configured paid accounts and
> surplus free-provider accounts and renders them as `pro_required`. This
> document describes the licensing **design**;
> treat every "unlocks" statement below as what happens once a real
> production key is embedded and the activation endpoint exists.

UsageCheck ships as **one binary** for everyone. Codex, Claude, and agy
(Gemini/Antigravity) are free, with one active account each in Free and
unlimited accounts in Pro. A **Pro license key** is designed to unlock
Cursor, Grok, Higgsfield, Kimi, OpenCode Go, DeepSeek, OpenRouter, GitHub Copilot, Windsurf, MiniMax, Augment, Poe, Fireworks, Novita, Amp, Z.AI, and Alibaba Token Plan at **runtime** — there is no separate Free/Pro
binary, no compile-time edition Cargo feature, and no `tauri.pro.conf.json`
override.
This replaces the two-binary/compile-time-edition split UsageCheck used
before the runtime license gate landed.

For the high-level product overview, see the [README](../README.md). For the
signed-token activation wire contract, see
[`docs/LICENSE_API.md`](LICENSE_API.md). For cross-platform tray architecture
and the original rewrite plan, see
[`docs/superpowers/specs/2026-07-08-usagecheck-crossplatform-design.md`](superpowers/specs/2026-07-08-usagecheck-crossplatform-design.md).
For local development-only Pro verification, see [`docs/dev-pro.md`](dev-pro.md).

## Product identity

| | |
| --- | --- |
| Product name | `UsageCheck` |
| Bundle ID | `com.usagecheck.desktop` |
| Config | `src-tauri/tauri.conf.json` (the only Tauri config — no per-edition override file) |
| Providers | Codex, Claude, Gemini (agy) free with one active account each while unlicensed; unlimited accounts plus Cursor, Grok, Higgsfield, Kimi, OpenCode Go, DeepSeek, OpenRouter, GitHub Copilot, Windsurf, MiniMax, Augment, Poe, Fireworks, Novita, Amp, Z.AI, Alibaba Token Plan once Pro is active |

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
| **Kimi Code** | Yes | **Add Kimi (CLI)** — `~/.kimi-code/credentials` (or `$KIMI_CODE_HOME`) | `GET https://api.kimi.com/coding/v1/usages` (fallback `api.kimi.ai`) | 5h / 7d used % |
| **OpenCode Go** | Yes | **Add OpenCode Go (CLI)** — `auth.json` `opencode-go` key | `GET https://opencode.ai/zen/go/v1/usage` | rolling / weekly used % + monthly row |
| **DeepSeek** | Yes | **Add DeepSeek (dsh)** — `~/.dsh/.credentials.yaml` or `.env` | Official `GET https://api.deepseek.com/user/balance` | Remaining balance (`¥`/`$ left`), no invented used % |
| **OpenRouter** | Yes | **Add OpenRouter (CLI)** — `~/.ori/config.json` or OpenCode `auth.json` `openrouter` | Official `GET https://openrouter.ai/api/v1/key` | Period used % when capped; otherwise `$ left` / `$ used` |
| **GitHub Copilot** | Yes | **Import GitHub Copilot (local, Experimental)** — `~/.config/github-copilot` / `gh` hosts.yml | Undocumented `GET https://api.github.com/copilot_internal/user` | Premium used % (monthly) or `unlimited` |
| **Windsurf** | Yes | **Import Windsurf (local, Experimental)** — `state.vscdb` | Undocumented Connect RPC `GetUserStatus` | Daily → 5h used %; weekly used % + optional `$ left` |
| **MiniMax** | Yes | **Add MiniMax (CLI)** | `mmx quota show --output json` subprocess (probes `--json` aliases) | 5h + weekly used % from remaining-percent fields |
| **Augment** | Yes | **Add Augment (CLI)** | `auggie account status --json` subprocess | Credits used % when remaining+included; else `N credits remaining` |
| **Poe** | Yes | **Add Poe (CLI)** — `~/.poe-code/credentials.enc` (machine-derived decrypt) or plaintext `credentials.json` / `config.json` | Official `GET https://api.poe.com/usage/current_balance` | Remaining `N points left`; used % only when a limit is present |
| **Fireworks** | Yes | **Add Fireworks (CLI)** — `~/.fireworks/auth.ini` | Official `GET https://api.fireworks.ai/v1/accounts/{id}/billing/summary` | `$ billed` for the calendar month; used % only with spend+limit |
| **Novita** | Yes | **Add Novita (CLI)** — `~/.novita/config.json` | Official `GET https://api.novita.ai/openapi/v1/billing/balance/detail` | `$X.XX left` from `availableBalance` (1/10000 USD); no invented used % |
| **Amp** | Yes | **Add Amp (CLI)** — `~/.local/share/amp/secrets.json` | Undocumented JSON-RPC `userDisplayBalanceInfo` on `ampcode.com/api/internal` | Used % when remaining+total; else `$N remaining` |
| **Z.AI** | Yes | **Add Z.AI (CLI)** — OpenCode `auth.json` `zai`, then `~/.zcode/v2/config.json` / `~/.hermes/auth.json` | Reverse-engineered `GET https://api.z.ai/api/monitor/usage/quota/limit` (raw Authorization) | 5h + weekly used % |
| **Alibaba Token Plan** | Yes | **Add Alibaba Token Plan (CLI)** | Official `bl usage token-plan --output json` | 5h + weekly used fractions (`per5HourPercentage` / `per1WeekPercentage`) |

A Pro-gated provider is hidden from the **Add Account** menu (and its
account, if one somehow exists, renders `pro_required`) until a Pro license
is active — see `usage_core::edition::requires_pro` and the runtime gate in
`src-tauri/src/tray_menu/actions.rs` / `src-tauri/src/poller/mod.rs`.

### Free-state account limits

`usage_core::edition::FREE_ACCOUNTS_PER_PROVIDER = 1`: in the Free runtime
state, one account per provider remains active for Codex, Claude, and agy.
"First" means the earliest account in index/insertion order, and that account
remains fully polled. This limit does not delete additional accounts already
stored: they stay listed but report `pro_required` with no quota numbers.
Activating Pro restores every account on the next poll with no re-import.

For these free providers, the tray's **Add Account** entry stays visible but
is disabled at the cap, with the reason in its label. This differs from paid
provider entries, which are hidden in Free. The shared policy and its
add-time and poll-time gates live in `crates/usage-core/src/edition.rs`,
`src-tauri/src/store/validation.rs`, and `src-tauri/src/poller/mod.rs`.

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

#### Kimi Code

1. Sign in with the Kimi Code CLI so it writes
   `~/.kimi-code/credentials/*.json` (or `$KIMI_CODE_HOME/credentials`).
   `~/.kimi/credentials/kimi-code.json` is also tried.
2. Tray → **Add Account** → **Add Kimi (CLI)**. There is no in-app browser
   login; tokens are imported from those files and stored by UsageCheck.
3. Polling uses `GET https://api.kimi.com/coding/v1/usages` with the access
   token (`User-Agent: UsageCheck`). A 404 retries `api.kimi.ai`.
4. Refresh uses `POST https://auth.kimi.com/api/oauth/token` with the
   imported refresh token.

#### OpenCode Go

1. Run `opencode auth login` and choose **OpenCode Go** (not Zen).
2. Tray → **Add Account** → **Add OpenCode Go (CLI)** reads
   `$OPENCODE_DATA_DIR/auth.json`, else `$XDG_DATA_HOME/opencode/auth.json`,
   else `~/.local/share/opencode/auth.json`.
3. Only the `opencode-go` entry is imported. Other keys in that file
   (including OpenRouter) are ignored by this provider.
4. Polling uses `GET https://opencode.ai/zen/go/v1/usage`. HTTP 403 means
   a Zen key or no Go subscription (`needs_setup`).

#### DeepSeek

1. Install DeepSeek Harness (`npx @deepseek-ai/dsh`) or put
   `DEEPSEEK_API_KEY` in `~/.dsh/.env` (or `$DSH_HOME`).
2. Tray → **Add Account** → **Add DeepSeek (dsh)** reads
   `$DSH_HOME/.credentials.yaml` then `.env`. Project `.env` files are not
   walked.
3. Polling uses official `GET https://api.deepseek.com/user/balance`.
   Remaining balance is shown as `¥110.00 left` / `$12.34 left`; used % is
   not invented from a prepaid wallet. `is_available: false` or a zero
   total is still `ok` (depleted).

#### OpenRouter

1. Tray → **Add Account** → **Add OpenRouter (CLI)** tries
   `~/.ori/config.json` (or `$ORI_HOME/config.json`) for an API key field,
   then the `openrouter` entry in OpenCode `auth.json`.
2. Polling uses official `GET https://openrouter.ai/api/v1/key`. When
   `limit` is set, used % is `(limit - limit_remaining) / limit`. When
   `limit` is null, remaining/usage is shown as `$ left` / `$ used`.

#### GitHub Copilot (Experimental)

This integration is **Experimental** — it depends on GitHub's undocumented
`/copilot_internal/user` endpoint, which can change without notice.

1. Sign in to GitHub Copilot in VS Code (or `gh auth login`).
2. Tray → **Add Account** → **Import GitHub Copilot (local, Experimental)**.
3. UsageCheck reads (read-only), first usable:
   - `~/.config/github-copilot/apps.json` (or `hosts.json`) `oauth_token`
   - `~/.config/gh/hosts.yml` github.com `oauth_token` / `token`
4. Process env `GITHUB_TOKEN` is **not** used (the tray app does not inherit
   the user's shell environment).
5. Polling calls `GET https://api.github.com/copilot_internal/user` with
   `Authorization: token …` (Bearer retry on 401) and `User-Agent: UsageCheck`.
6. Premium interactions (fallback premium models, then chat) map to the
   monthly billing-period bar. Unlimited / entitlement `-1` shows `unlimited`
   with no percent. Completions are ignored as the primary bar.
7. HTTP 401/403 → `needs_login`; 404 → `needs_setup`; 429 → `throttled`.

#### Windsurf (Experimental)

This integration is **Experimental** — it depends on Windsurf's local SQLite
layout and an undocumented Connect RPC, both of which can change without
notice. It is read-only and never writes to Windsurf's database.

1. Sign in to the Windsurf desktop app.
2. Tray → **Add Account** → **Import Windsurf (local, Experimental)**.
3. The app reads (read-only) from Windsurf's SQLite `state.vscdb`:

   - macOS: `~/Library/Application Support/Windsurf/User/globalStorage/state.vscdb`
   - Windows: `%APPDATA%/Windsurf/User/globalStorage/state.vscdb`

4. Key read: `windsurfAuthStatus` JSON → `apiKey` (also `api_key`, nested).
5. Polling calls
   `POST https://server.self-serve.windsurf.com/exa.seat_management_pb.SeatManagementService/GetUserStatus`
   (fallback `server.codeium.com`) with Connect protocol headers.
6. Daily remaining % maps to the 5h slot as used %; weekly remaining % maps
   to week. Optional `$ left` from `overageBalanceMicros`.
7. Missing DB/key or RPC 401/403 → `needs_login`; other RPC failures →
   `experimental_error`. Local tokens are re-read from the DB on each poll
   when the identity matches.

#### MiniMax

1. Install the [MiniMax CLI](https://github.com/MiniMax-AI/cli) (`mmx`) and
   ensure it is on your `PATH` (Homebrew/user-local bins are also searched).
2. Run `mmx auth login` in a terminal yourself first — there is no in-app
   browser login for MiniMax.
3. Tray menu → **Add Account** → **Add MiniMax (CLI)** creates a pure CLI
   reference via `mmx quota show --output json` (no credential file read;
   `--json` / `mmx quota --output json` are probed if needed).
4. Each poll runs the same CLI JSON command and maps
   `current_interval_remaining_percent` / `current_weekly_remaining_percent`
   to used % (Codex-like 5h + weekly). Counts-only JSON is not converted
   into a percentage.
5. If the CLI is missing, import fails with a clear install/`mmx auth login`
   message; polling status is **`needs_setup`** when the CLI is unavailable
   or JSON has no remaining-percent windows.

#### Augment

1. Install the [Auggie CLI](https://www.npmjs.com/package/@augmentcode/auggie)
   (`npm i -g @augmentcode/auggie`) and ensure `auggie` is on your `PATH`.
2. Run `auggie login` in a terminal yourself first — there is no in-app
   browser login for Augment. `--json` on `auggie account status` needs
   Auggie 0.24.0+.
3. Tray menu → **Add Account** → **Add Augment (CLI)** creates a pure CLI
   reference via `auggie account status --json` (no credential file read).
4. Each poll runs the same command. Remaining+included credits become used %
   plus `N/M credits`; a bare remaining number is shown as
   `N credits remaining` and is never converted into a percentage.
5. If the CLI is missing, import fails with a clear install/`auggie login`
   message; polling status is **`needs_setup`** when the CLI is unavailable
   or JSON has no recognizable credit fields.

#### Poe

1. Run `npx poe-code login` (or `npx poe-code@latest login`). Credentials
   live in `~/.poe-code/` (`$POE_CODE_HOME` overrides).
2. Tray → **Add Account** → **Add Poe (CLI)** reads, first usable:
   - `credentials.enc` decrypted with the official machine-derived key
     (`hostname:username` via scrypt, AES-256-GCM; no user password)
   - plaintext `credentials.json` `apiKey`
   - `config.json` `apiKey` / `core.apiKey`
3. Polling uses official `GET https://api.poe.com/usage/current_balance`
   (`User-Agent: UsageCheck`). Remaining points are `N points left`. Used %
   is never invented from a remaining-only balance.

#### Fireworks

1. Run `firectl signin` and/or `firectl set-api-key` so
   `~/.fireworks/auth.ini` (`$FIREWORKS_HOME`) has `account_id` and an API
   key.
2. Tray → **Add Account** → **Add Fireworks (CLI)** imports both fields.
3. Polling uses official
   `GET https://api.fireworks.ai/v1/accounts/{account_id}/billing/summary`
   for the current UTC calendar month. Line-item spend is `$12.34 billed`.
   Used % only when the JSON also has a positive spend limit.

#### Novita

1. Run `novita auth login` so `~/.novita/config.json` (`$NOVITA_HOME`) holds
   a token / team API key.
2. Tray → **Add Account** → **Add Novita (CLI)** prefers the selected team's
   API key, then a top-level token.
3. Polling uses official
   `GET https://api.novita.ai/openapi/v1/billing/balance/detail`.
   `availableBalance` is 1/10000 USD and is shown as `$X.XX left`.
   `creditLimit` is not treated as a usage cap.

#### Amp (Experimental)

This integration is **Experimental** — it depends on Amp's undocumented
JSON-RPC `userDisplayBalanceInfo` method, which can change without notice.

1. Install [Amp](https://ampcode.com/) and run `amp login`.
2. Tray → **Add Account** → **Add Amp (CLI)** reads
   `$AMP_HOME/secrets.json`, else `$XDG_DATA_HOME/amp/secrets.json`, else
   `~/.local/share/amp/secrets.json` (`apiKey@https://ampcode.com/`).
3. Polling `POST`s `{"method":"userDisplayBalanceInfo","params":{}}` to
   `https://ampcode.com/api/internal` with `Authorization: Bearer`.
4. Amp Free `$remaining/$total remaining` becomes used %. Credits-only
   `$N remaining` is never converted into a percentage.
5. HTTP 401/403 → `needs_login`; other RPC failures → `experimental_error`.

#### Z.AI (Experimental)

This integration is **Experimental** — the monitor quota API is
community reverse-engineered and is not an official public contract.

1. Tray → **Add Account** → **Add Z.AI (CLI)** reads, first usable:
   - OpenCode `auth.json` `zai` (or `zai-coding-plan`) `{type:api, key:…}`
     — other keys in that file are ignored
   - `~/.zcode/v2/config.json` API key field
   - `~/.hermes/auth.json` `zai` entry
2. Polling uses `GET https://api.z.ai/api/monitor/usage/quota/limit` with
   raw `Authorization: <key>` (Bearer retry on 401).
3. 5h / weekly used % come from `percentage`, remaining percents, or
   `used`/`limit`. `nextResetTime` maps to `resets_at`.
4. HTTP 401 → `needs_login`; 403/404 → `needs_setup`; 429 → `throttled`.

#### Alibaba Token Plan

1. Install the [Bailian / Model Studio CLI](https://github.com/modelstudioai/cli)
   (`bl`) and run `bl auth login`.
2. Tray → **Add Account** → **Add Alibaba Token Plan (CLI)** runs
   `bl usage token-plan --output json` (probes `--console-site international`
   if needed).
3. `per5HourPercentage` / `per1WeekPercentage` are used fractions: `<= 1`
   is treated as a ratio, otherwise a 0–100 percent. Reset timestamps are
   used when present.
4. Missing CLI → install/`bl auth login`; polling is **`needs_setup`** when
   the CLI is unavailable or JSON has no windows.

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
    kimi.rs / opencode.rs / deepseek.rs / openrouter.rs
    copilot.rs / windsurf.rs
    minimax.rs / augment.rs  # CLI quota / account-status JSON
    amp.rs / zai.rs / bailian.rs

src-tauri/
  src/edition.rs          # product_name(), re-exports all_providers()
  src/license/            # signed-token activation, status, offline grace (docs/LICENSE_API.md)
  src/cursor_local.rs     # read-only state.vscdb import
  src/windsurf_local.rs   # read-only Windsurf state.vscdb import
  src/import/             # load_grok_env_auth(), import_grok_from_clipboard(),
                          # load_higgsfield_cli_auth(), load_kimi_cli_auth(),
                          # load_opencode_cli_auth(), load_deepseek_cli_auth(),
                          # load_openrouter_cli_auth(), load_copilot_cli_auth(),
                          # load_minimax_cli_auth(), load_augment_cli_auth(),
                          # load_poe_cli_auth(), load_fireworks_cli_auth(),
                          # load_novita_cli_auth(), load_amp_cli_auth(),
                          # load_zai_cli_auth(), load_bailian_cli_auth()
  src/poller/             # poll_* for paid providers; requires_pro()-gated dispatch
  src/tray_menu/          # auth_action_specs() (is_pro()-filtered); license tray section
  src/menu_actions.rs     # handle_menu_event(): add-cursor-local / add-grok-clipboard /
                          # add-grok-env / add-higgsfield-cli / add-kimi-cli /
                          # add-opencode-cli / add-deepseek-cli / add-openrouter-cli /
                          # add-copilot-local / add-windsurf-local /
                          # add-minimax-cli / add-augment-cli /
                          # add-poe-cli / add-fireworks-cli / add-novita-cli /
                          # add-amp-cli / add-zai-cli / add-bailian-cli /
                          # license-activate-clipboard / license-deactivate / license-get
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
| **Kimi Code** | CLI file import only (no in-app browser login). Empty/404 usage payload → `needs_setup`. |
| **OpenCode Go** | Imports only `opencode-go` from `auth.json`. A Zen key returns HTTP 403 (`needs_setup`). |
| **DeepSeek** | Prepaid wallet: remaining balance only, never a fabricated used %. |
| **OpenRouter** | Unlimited keys (`limit: null`) show `$ left` / `$ used` instead of used %. |
| **GitHub Copilot** | **Experimental.** Undocumented `copilot_internal/user`. 404 means no Copilot subscription (`needs_setup`). |
| **Windsurf** | **Experimental.** Undocumented `GetUserStatus` Connect RPC and local `state.vscdb`. No separate Codeium provider. |
| **MiniMax** | **Pure CLI reference** via `mmx quota show --output json` (no credential file read). Remaining-percent fields only; counts are not converted to %. Unrecognized JSON → `needs_setup`. |
| **Augment** | **Pure CLI reference** via `auggie account status --json` (Auggie 0.24.0+). Bare remaining credits never become a used %. Unrecognized JSON → `needs_setup`. |
| **Poe** | Remaining points only unless the balance payload includes a limit. Encrypted `credentials.enc` uses a machine-derived key (not a user password). |
| **Fireworks** | Calendar-month billed spend as `$ billed`. Used % only with spend vs limit in the same JSON. |
| **Novita** | `availableBalance` converted from 1/10000 USD. No invented used %. |
| **Amp** | **Experimental.** Undocumented `userDisplayBalanceInfo` JSON-RPC. Remaining-only never becomes used %. |
| **Z.AI** | **Experimental.** Reverse-engineered coding-plan monitor API. Raw Authorization first. |
| **Alibaba Token Plan** | **Pure CLI reference** via `bl usage token-plan --output json`. Fractions `<= 1` are ratios. |
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
- Push of tags matching `v*` (e.g. `v0.2.0`)

A `guard` job runs first and checks the embedded license Ed25519 public key
(`src-tauri/src/license/pubkey.rs`) — see that file and `pubkey_tests.rs` for
the mechanism. As of 2026-07-30 this check is advisory: a still-placeholder
key emits a workflow warning and a job-summary note instead of failing the
release, and the resulting build ships permanently Free — no customer key
can unlock Pro (Codex/Claude/agy remain free but are subject to Free's
one-account-per-provider limit). Dropping the
`continue-on-error` line on that step in the workflow makes it blocking
again. The `build` job then runs regardless:

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
- Pro 라이선스 키를 활성화하면 런타임에 Cursor, Grok, Higgsfield, Kimi, OpenCode Go, DeepSeek, OpenRouter, GitHub Copilot, Windsurf, MiniMax, Augment, Poe, Fireworks, Novita, Amp, Z.AI, Alibaba Token Plan이 열립니다 (별도 바이너리 없음).
- 트레이 메뉴 → 라이선스 섹션 → **Activate from clipboard**로 키 등록,
  **Deactivate license**로 해제, **Get a license…**로 구매 페이지 열기.
- 활성화는 Ed25519 서명 토큰(디바이스 바인딩, 오프라인 유예 14일)으로 검증됩니다 — 자세한 내용은 `docs/LICENSE_API.md`.
