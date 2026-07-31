@AGENTS.md

# CLAUDE.md

`AGENTS.md` above is imported, so it loads every session and stays the single source of truth for this
repo's preferences and workspace facts. Keep new repo guidance there. This file adds only the harness
routing.

## Checkouts and Orca worktrees

- **This main checkout** (`~/Documents/Cursor/Usage_Check`) is an ordinary git checkout.
- **An Orca-managed worktree** is any checkout under `~/orca/workspaces/`. There, Orca owns the
  worktree, its terminals and the embedded browser: do not `git worktree add` and do not nest
  worktrees, because the repo sets `externalWorktreeVisibility: "hide"`, so a non-Orca worktree would
  hold the branch while staying invisible in the Orca UI. Create siblings with
  `orca worktree create --name <n>`.

Orca worktree names are EPHEMERAL — never hardcode one. **Uncommitted work in an Orca worktree is lost
when that worktree is removed**; commit or push before it goes away.

Startup race: this repo's setup script is `pnpm install`, but its Orca startup policy is
`setupAgentStartupPolicy: start-immediately`, so an agent auto-launched with a worktree can begin
before install finishes. `--setup run` does NOT fix this — that flag sets `setupRunPolicy` (already
`run-by-default`), a different field, and no `orca` command can write `setupAgentStartupPolicy` (it is
UI-only). The working mitigation is to not auto-launch an agent: create the worktree WITHOUT `--agent`,
wait for setup to finish, then `orca terminal create --worktree <selector> --command <agent>`.

## Harness role — LOGIC

Rust/Tauri workspace (`crates/usage-core`, `src-tauri`) plus a legacy `ui/`. TEST and LOGIC artifacts
are authored by the isolated `codex-author` lane (scratch root outside the repo, then safe-copy-in);
Claude and AGY verify. PLAN stays Claude-authored, dual-verified. Do not author TEST/LOGIC in the main
context, and do not route authoring here to the FDW `designer`/`writer` lane.

## Mechanical check

`cargo test -p usage-app` — the repo's own suite command, named in
`.github/workflows/release.yml:17`. Note line 59 runs only
`cargo test -p usage-app -- embedded_public_key_is_not_placeholder --ignored`, a single ignored test,
not the suite. The tree declares 510 test functions
(`grep -rnE '#\[(test|tokio::test)\]' crates/ src-tauri/ | wc -l`). A full run was not executed when
this file was written.
