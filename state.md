# Locagens — State (`state.md`)

Short handoff note: where the project stands. Update it when a phase lands.

## Current State

Locagens is a pure Rust cargo workspace (see `Claude.md` / `README.md` for the
layout). The earlier Electron + Vue + Node.js (Fastify, SQLite) app is gone.

- `crates/*`: `protocol`, `apply-patch`, `execpolicy`, `config`, `context`,
  `core` (`locagens_core`: api facade, orchestrator, services, CLI/TUI, rollout
  JSONL store).
- `apps/cli` (binary `lcgn`) and `apps/desktop-rs` (Dioxus 0.7, binary
  `locagens`); both call `locagens_core::api` in-process. No HTTP server.
- Last recorded test run (2026-11-13, Windows GNU toolchain,
  `cargo test --workspace --no-fail-fast`): 599 passed, 0 failed, 1 ignored.
- The QA review in `docs/QA_REVIEW.md` (QA-001..073) drove a large hardening
  pass; the remaining items there are tests (QA-071..073) and refactors.

## Completed Work

```txt
0   agent rules setup, legacy .claude/agents removed
1-3 Rust core: JSON/JSONL storage, provider/auth engine, streaming LLM client,
    workspace tools, permission gating, run orchestrator
4   Node backend removed
5   lcgn CLI
6   locagens_core::api facade; model/agent presets removed (single-model runs)
7   native Dioxus desktop app with settings/inspector parity
8   apps/web, Electron app, packages/shared, HTTP server removed
9   Codex harness port: apply_patch, exec_command/write_stdin, exec policy,
    AGENTS.md discovery, environment_context, turn diffs, compaction,
    Codex multi-agent v2 tools, ~/.locagens + ~/.codex config
10  split into crates/* + apps/cli + apps/desktop-rs
```

Since then:

- **Logins:** OAuth for ChatGPT (`openai`), GitHub Copilot, xAI/Grok, OpenRouter
  and Kimi (device-code and authorization-URL flows, auto refresh); OpenCode
  `auth.json` read as an API-key fallback. Anthropic stays API key only.
- **`lcgn`:** ratatui full-screen TUI, shared slash-command table
  (`cli/commands.rs`), sessions, `lcgn review`, `lcgn trust`, `lcgn mcp`,
  `lcgn config`, `--profile`, `--trust`.
- **Code review:** read-only `chat`-mode review runs with structured findings
  (`api::review`, `lcgn review`, `/review`).
- **Sandbox:** Windows restricted token + Job Object + per-domain network proxy,
  macOS Seatbelt, Linux Bubblewrap (fail closed when missing); shared
  protected-path list.
- **MCP:** stdio and streamable HTTP (`http` / `streamable-http` / `sse`
  entries), bearer/header auth and OAuth (discovery, dynamic registration,
  PKCE), resource tools, deferred tools via `tool_search`.
- **Hooks and config:** Codex-style lifecycle hooks (`docs/HOOKS.md`),
  `config.toml` subset with profiles (`docs/CONFIG.md`), Starlark `.rules`
  execpolicy.
- **Project trust:** `<project>/.locagens/` config loads only for trusted
  folders (`lcgn trust`, desktop banner).
- **Storage:** Codex-style rollout files under `~/.locagens`, multi-process
  safe (advisory lock, run owner locks); legacy data imported once.
- **Desktop extras:** git panel (stage/unstage/commit, generated messages),
  turn snapshots and undo, worktrees, processes, skills, plugins, memory.

## Known Limits

- Desktop UI has no automated end-to-end agent-run test (manual only).
- `lcgn` TUI/REPL Ctrl+C cancel and permission prompts have had little testing
  on real terminals beyond Windows.
- With a non-TTY stdin that never closes, `lcgn -p` waits for EOF; close it
  with `$null |` or `< NUL`.
- On macOS/Linux there is no per-domain network proxy: approving
  `network_domains` opens the whole network for that command.
- Tests that touch the developer's real `~/.locagens` / `~/.codex`
  (QA-071) and several regression-test gaps (QA-072) are still open.

## Build Notes (Windows)

Prepend `$HOME\w64devkit\w64devkit\bin` to `PATH`, then:

```powershell
rustup run stable-x86_64-pc-windows-gnu cargo check --workspace
rustup run stable-x86_64-pc-windows-gnu cargo test --workspace
```

## Working Rules

- Keep this file current when a phase lands.
- Commit finished work; see `Claude.md` for the workflow.
