# agy/Antigravity usage-source investigation (research spike, Task 9)

Date: 2026-07-08
Scope: read-only investigation only. No login, no interactive auth, no network calls, no credential
values recorded (structure/key-names only, redacted).

## 1. `~/.gemini` filesystem survey

Top-level layout:

```
~/.gemini/
  .gitignore
  HARNESS.md
  rules/
  antigravity/            # Antigravity IDE local state (older/lighter install)
  antigravity-cli/        # the `agy` CLI's state — this is the relevant one
  antigravity-ide/        # another IDE-side state dir
  config/
    .migrated
    config.json
    mcp_config.json
    plugins/
    projects/
    sidecars/
```

`config/config.json` (full, safe to show — only one key):
```json
{ "userSettings": { "remoteControlHostname": "<redacted-hostname-string>" } }
```
No auth token, no API key, no usage/quota field.

`config/mcp_config.json` — MCP server wiring for the local verify tooling (`score-verify` python
server + env vars for model IDs/fallbacks). Not related to agy's own usage accounting; this is
OMC-side tooling that happens to live under the same MCP config file, not an Antigravity usage API.

`antigravity-cli/` (this is where the `agy` CLI keeps its state) top-level entries:
```
bin/                      -> agentapi, webm_encoder binaries
brain/                    -> ~190 UUID-named directories (all EMPTY in the sample checked)
builtin/
cache/
  default_project_id.txt
  last_conversations.json   (1.6KB, binary-ish, mode 600)
  onboarding.json           (111 bytes, mode 600)
  projects.json
cli.log -> log/cli-<timestamp>.log
conversation_summaries.db  (SQLite)
conversations/             -> per-conversation SQLite DBs (<uuid>.db [+ -shm/-wal])
crashes/
installation_id            (36 bytes — just a UUID)
jetski_state.pbtxt         (protobuf text, mode 600)
knowledge/
last_check.timestamp       (empty)
log/                       -> ~80 rotated cli-*.log files
mcp/
scratch/
settings.json              -> literally `{}`
updater/
```

**No auth/usage JSON file exists anywhere under `~/.gemini`.** There is no `auth.json`,
`credentials.json`, `usage.json`, `quota.json`, or anything with `usage`/`quota`/`auth`/`credential`
in its filename (`find ~/.gemini -iname "*usage*" -o -iname "*quota*" -o -iname "*auth*" -o
-iname "*credential*"` returns nothing). A recursive grep for `access_token|Bearer|oauth|api_key|apiKey`
across all text-readable files under `~/.gemini` also returns nothing.

**Where the actual credential lives:** macOS Keychain, not the filesystem. `security dump-keychain`
(names only, no secret values retrieved) shows generic-password entries:
```
svce="Antigravity Safe Storage"     acct="Antigravity Key"
svce="Gemini Safe Storage"          acct="Gemini Keys"
svce="Antigravity IDE Safe Storage" acct="Antigravity IDE"
```
This is the standard Electron/Chromium `safeStorage` pattern (an app-specific encryption key sits in
Keychain; the actual OAuth/session tokens are AES-encrypted blobs on disk elsewhere, e.g. under
`~/Library/Application Support/Antigravity/`). Reading it requires a Keychain-unlock prompt (`security
find-generic-password` without `-w`/`-g` won't dump the secret, and doing so would need user consent) —
this is exactly the kind of interactive/credential-touching step the task explicitly forbids, and it's
not the local per-user data files a lightweight usage-checker should be parsing anyway.

`~/Library/Application Support/` also has `Antigravity/`, `Antigravity IDE/`, `com.google.GeminiMacOS/`
dirs (mode 700, not inspected further — out of scope, same keychain-backed encrypted-blob situation).

## 2. `agy` CLI surface

`agy --help` (non-interactive, safe):
```
Available subcommands:
  changelog       Show changelog and release notes
  help            Show help for subcommands
  install         Configure environment paths and shell settings
  models          List available models
  plugin          Manage plugins (install, uninstall, list, enable, disable)
  plugins         Alias for plugin
  update          Update CLI
```

`agy --version` → `1.1.0`.

`agy models` (safe, ran it) → prints the static model list (Gemini 3.5 Flash tiers, Gemini 3.1 Pro
tiers, Claude Sonnet/Opus 4.6, GPT-OSS 120B) — no usage/quota numbers, no per-model remaining-budget
info, just names.

**There is no `usage`, `quota`, `status`, `account`, or `whoami` subcommand.** Nothing in `--help`
output reports quota/usage/remaining-budget. Unlike Codex (`codex` CLI wraps
`chatgpt.com/backend-api/wham/usage`) or Claude (`api.anthropic.com/api/oauth/usage`), agy's CLI
surface exposes no analogous read.

## 3. Existing Swift Gemini handling (`Sources/UsageCheckCore/LogScanners.swift` +
`ScannerUtilities.swift`)

`ScannerUtilities.swift` `UsagePaths.geminiLogRoots`:
```swift
var geminiLogRoots: [URL] {
    [
        home.appendingPathComponent(".gemini"),
        home.appendingPathComponent(".config/gemini")
    ]
}
```
Just two root globs (`~/.gemini`, `~/.config/gemini`) — no credential file path is defined for Gemini
at all (contrast `codexAuthFile` and `claudeCredentialFiles`, which both exist as explicit properties).
This matches the finding above: there is no known Gemini/agy auth-file path to point at, because there
isn't one on disk.

`LogScanners.swift` `GeminiLogScanner.scan(now:)`:
- Enumerates `.jsonl` files under `geminiLogRoots`, filtered to files whose name starts with
  `"transcript"` or whose path contains `/logs/` — i.e. it's scanning for any JSONL transcript/log
  file, not a specific known Antigravity log schema.
- For each matching line, best-effort recursive search (`findTokenTotal`, `findModel`) walks the
  parsed JSON looking for `usageMetadata`/`usage_metadata`/`usage` sub-objects with keys like
  `totalTokenCount`, `promptTokenCount`, `candidatesTokenCount`, `cachedContentTokenCount`,
  `thoughtsTokenCount` (Gemini API's own usage-metadata field names), or generically any key
  containing `"token"` (and not `"limit"`) anywhere in the object graph as a fallback.
- Produces `ModelTokenEvent`s (timestamp + model + token count) — raw token counts, not %-of-quota.

This confirms the current/existing approach is **already local-log token aggregation only** — it was
built defensively (schema-sniffing rather than a fixed schema) precisely because there's no
authoritative endpoint or fixed log format to rely on. In practice, given the `antigravity-cli/`
survey above, the actual per-conversation data lives in SQLite (`conversations/*.db`,
`conversation_summaries.db`), not in `.jsonl` transcripts — so this scanner's glob (`transcript*.jsonl`
or `/logs/*.jsonl`) may not even be matching current-CLI-version log files. That's a separate, existing
gap outside this spike's scope (no code changes made here), but worth flagging for the next task that
touches Gemini scanning.

## 4. DECISION

**(b) Local-log aggregation is the only available source. No quota-% endpoint exists to call.**

Justification:
1. `agy --help` exposes no `usage`/`quota`/`status`/`account` subcommand — contrast Codex/Claude CLIs,
   which do wrap a discoverable usage endpoint.
2. No usage/quota JSON snapshot, no auth-token file, and no endpoint URL appear anywhere under
   `~/.gemini` (file-name search and content grep both empty).
3. The only credential material is inside macOS Keychain as an encrypted Electron-style
   `safeStorage` blob (`"Antigravity Safe Storage"` / `"Gemini Safe Storage"` service entries) — there
   is no plaintext bearer token to read, and extracting/decrypting it would require Keychain access
   consent, which is both out of scope for this read-only spike and not appropriate for a lightweight
   usage-checker to depend on.
4. The existing `GeminiLogScanner` already implements local-log/transcript token aggregation
   defensively (schema-sniffing multiple possible key names) — this is the same strategy this spike
   would have recommended, so no architecture change is needed, only note the gap above (transcript
   glob may be stale vs. current CLI's SQLite-based storage — worth a follow-up ticket, not this one).

**Consequence for the app:** agy/Gemini usage cards should continue to show **token totals from local
logs**, not a %-of-quota gauge — there is no `fetch/agy.rs` HTTP parser to build; there is no quota
endpoint to call. Codex and Claude remain the only sources with real quota-% endpoints
(`chatgpt.com/backend-api/wham/usage`, `api.anthropic.com/api/oauth/usage`).
