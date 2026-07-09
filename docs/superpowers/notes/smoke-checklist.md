# Manual E2E Smoke Checklist — UsageCheck (Tauri app)

This checklist covers behavior that cannot be validated headlessly (GUI rendering,
system tray, OS keychain, and live browser OAuth against real provider endpoints).
A human must run through this on macOS and, when available, Windows before a
release is considered smoke-tested.

Prerequisites: `cargo build -p usage-app --release` (or `cargo tauri dev`) succeeds,
and `ui/dist` is built (`cd ui && npm install && npm run build`).

## Launch & tray

- [ ] Launch the app (`cargo tauri dev`, or run the release binary directly).
      No terminal window / dock icon should linger as the primary UI — the app
      lives in the tray.
- [ ] A tray icon appears (macOS menu bar icon, or Windows taskbar system-tray
      icon).
- [ ] Click the tray icon → the borderless popup window opens.
- [ ] Click the tray icon again → the popup closes (toggle behavior).

## Adding accounts

- [ ] With no accounts yet, the popup shows the empty-state message
      ("계정을 추가해 사용량을 확인하세요.").
- [ ] Click "계정 추가" → the provider picker opens showing Codex / Claude /
      Antigravity (agy) options.
- [ ] Pick **Codex → 브라우저 로그인** → system browser opens to the
      ChatGPT/OpenAI OAuth authorize page → complete login → browser tab shows
      "Login complete. You may close this tab and return to the app." → back in
      the popup, a new Codex card appears showing 5h and 7d % gauges populated
      with real data.
- [ ] Pick **Codex → CLI에서 가져오기** (with `~/.codex/auth.json` present from
      a prior `codex login`) → a Codex card appears without opening a browser.
- [ ] Pick **Claude → 브라우저 로그인** → system browser opens to the
      Anthropic/Claude OAuth authorize page → complete login → a new Claude
      card appears with 5h/7d gauges populated.
- [ ] Pick **Claude → CLI에서 가져오기** (with Claude `.credentials.json`
      present) → a Claude card appears without opening a browser.
- [ ] Pick **agy → 로컬 로그로 추가** → NO browser window opens; an agy card
      appears showing 5h/7d local token totals (may be zero if no
      `~/.gemini` transcripts exist yet).
- [ ] Remove an account (✕ button on a card) → the card disappears immediately
      and does not reappear after the next poll tick (wait >60s).
- [ ] Tray menu → **Quit UsageCheck** exits the app cleanly.

## Persistence

- [ ] Quit the app fully (not just close the popup) and relaunch.
- [ ] Previously-added Codex/Claude accounts still appear in the list with
      gauges populating again — confirms credentials round-tripped through the
      OS keychain (macOS Keychain / Windows Credential Manager), not just
      in-memory state.

## Token refresh / expiry handling

- [ ] Let an account's access token approach its `expires_at` (or force it by
      editing the stored expiry, if testing this proactively). Confirm one of:
  - the app proactively refreshes the token in the background (poll loop
    keeps succeeding, no user-visible interruption), or
  - if refresh fails (e.g. revoked refresh_token), the account card shows a
    "needs_login" / re-auth badge rather than silently going blank or crashing.

## CRITICAL: OAuth client_id validation (flagged from Task 13)

The Codex and Claude OAuth `client_id` / `auth_url` / `token_url` values in
`src-tauri/src/oauth.rs` are best-known **public** values, each marked
`// TODO: verify` in the source. They have not been confirmed against a live
login round-trip as of this task. This checklist item is **not optional** —
do not consider OAuth "done" until both of the following are confirmed:

- [ ] **Codex**: `client_id = "app_EMoamEEZ73f0CkXaXp7hrann"` against
      `https://auth.openai.com/oauth/authorize` /
      `https://auth.openai.com/oauth/token` — perform a real login. If OpenAI
      rejects the client_id (e.g. `invalid_client`, `unauthorized_client`),
      capture the correct value (e.g. via a fresh `codex login` CLI trace or
      HAR capture) and update `oauth.rs`, replacing the `TODO: verify` comment
      with a confirmation note (date + how it was verified).
- [ ] **Claude**: `client_id = "9d1c250a-e61b-44d9-88ed-5944d1962f5e"` against
      `https://console.anthropic.com/oauth/authorize` /
      `https://console.anthropic.com/v1/oauth/token` — perform a real login.
      If Anthropic rejects the client_id, capture the correct value the same
      way and update `oauth.rs` similarly.
- [ ] Record the outcome of both checks here (date, result, and the final
      client_id used) so the `TODO: verify` markers can eventually be removed
      once confirmed stable across a few login attempts.

### Result log

| Date | Provider | client_id used | Outcome | Notes |
|------|----------|-----------------|---------|-------|
|      | Codex    |                 |         |       |
|      | Claude   |                 |         |       |

## Platform notes

- [ ] macOS: tray icon renders correctly in both light and dark menu bar
      themes (icon is a template image — `icon_as_template(true)` in
      `main.rs`).
- [ ] Windows (when a Windows host is available): taskbar tray icon, popup
      window positioning, and OAuth browser callback (localhost redirect)
      all behave the same as macOS. Not yet verified on Windows as of this
      task — flag as outstanding if no Windows host was available.
