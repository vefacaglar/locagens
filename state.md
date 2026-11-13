# Locagens — State (`state.md`)

Short handoff note: where the project stands. Update it when a phase lands.

## Current State

The Electron + Vue + Node.js (Fastify, SQLite) app has been fully replaced by
a pure Rust cargo workspace (see `Claude.md` / `Agents.md` for the layout):

- `crates/*`: `protocol`, `apply-patch`, `execpolicy`, `config`, `context`,
  `core` (`locagens_core`: api facade, orchestrator, services, CLI, rollout JSONL
  store).
- `apps/cli` (binary `lcgn`) and `apps/desktop-rs` (Dioxus 0.7, binary
  `locagens`); both call `locagens_core::api` in-process. No HTTP server.
- Last recorded test run: 107/107 passing across the workspace, release build
  clean (Phase 10).

## Completed Phases

```txt
0   agent rules setup, legacy .claude/agents removed
1-3 Rust core: JSON/JSONL storage, provider/auth engine, streaming LLM client,
    workspace tools, permission gating, run orchestrator
4   Node backend removed
5   lcgn CLI (merged 7615bc5)
6   locagens_core::api facade; model/agent presets removed (single-model runs)
7   native Dioxus desktop app with full settings/inspector parity
8   apps/web, apps/desktop (Electron), packages/shared, HTTP server removed (af5be81)
9   Codex harness port: apply_patch, exec_command/write_stdin, exec policy,
    AGENTS.md discovery, environment_context, turn diffs, compaction,
    Codex sub-agent tools, ~/.locagens + ~/.codex config
10  split into crates/* + apps/cli + apps/desktop-rs (T1-T7 merged)
```

## Known Limits

- Desktop UI has no automated end-to-end agent-run test (manual only).
- `lcgn` interactive REPL, Ctrl+C cancel and permission/question prompts have
  not been exercised on a real TTY.
- With a non-TTY stdin that never closes, `lcgn -p` waits for EOF; close it
  with `$null |` or `< NUL`.
- `ProcessManager` has no `clear_finished`; finished processes are pruned by
  its own lifecycle.

## Build Notes (Windows)

Prepend `$HOME\w64devkit\w64devkit\bin` to `PATH`, then:

```powershell
rustup run stable-x86_64-pc-windows-gnu cargo check --workspace
rustup run stable-x86_64-pc-windows-gnu cargo test --workspace
```

## Working Rules

- Commit and push to `origin/main` after each completed phase, and keep this
  file current.
