# Agents.md

## Project

Locagens is a local-first AI coding agent written in pure Rust (migrated from
Electron + Vue + Node; ignore any leftover references to those, SQLite or an
HTTP server). The user picks one provider/model and a project folder; the
model works the task with workspace tools. Runs are single-model; the main
agent can fan out to `explorer` (read-only) / `worker` sub-agents on the same
model.

## Layout

```txt
crates/protocol      shared data types, ApprovalPolicy, SandboxMode
crates/apply-patch   Codex `*** Begin Patch` parser/applier
crates/execpolicy    command safety, rules, sandbox, ProcessManager
crates/config        paths + provider/auth/model catalog (PiProviderManager)
crates/context       AGENTS.md discovery, <environment_context>, TurnDiffTracker
crates/core          locagens_core: api/ facade, orchestrator/, services/,
                     cli/ (lcgn), db.rs (JSON/JSONL store)
apps/cli             binary `lcgn`
apps/desktop-rs      binary `locagens` (Dioxus 0.7 UI)
```

Docs: `docs/CORE_API.md`, `docs/CLI.md`, `docs/database-schema.md`.

```bash
cargo check --workspace
cargo test --workspace
```

## Rules

- Keep it simple: readable code, small functions, no agent frameworks, no
  premature abstraction. Facade (`locagens_core::api`) holds no storage logic;
  Dioxus components stay presentational.
- All file access stays inside the run's project workspace.
- API keys live only in `auth.json` (`~/.locagens`, fallbacks `~/.codex`,
  `~/.pi/agent`) or env vars. Never log, persist or expose them.
- Every run message persists; runs must reopen after restart. Errors go to
  the run's `error_message` without dropping prior messages.
- No telemetry. Don't build unrequested features (git integration, reviewer
  loop, cloud/multi-user).

## Development Workflow (Codex)

Pick the model by task type:

```txt
terra    coding & development
luna     codebase exploration and simple edits
```
