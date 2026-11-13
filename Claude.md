# Claude.md

## Project

Locagens is a local-first AI coding agent. The user picks one provider/model
and a project folder; the model works the task with workspace tools until it
returns a final answer. Runs are single-model; the main agent can fan out to
sub-agents (`explorer` read-only / `worker` full tools) on the same model.
There are no agent presets and no coder/utility model tiers.

Locagens is pure Rust (it was migrated from Electron + Vue + Node). There is no
Node.js, Electron, Vue, SQLite or HTTP server — ignore any leftover references
to them.

## Layout

```txt
crates/protocol      locagens-protocol: shared data types, ApprovalPolicy, SandboxMode
crates/apply-patch   locagens-apply-patch: Codex `*** Begin Patch` parser/applier
crates/execpolicy    locagens-execpolicy: command safety, rules, sandbox, ProcessManager
crates/config        locagens-config: paths + provider/auth/model catalog (PiProviderManager)
crates/context       locagens-context: AGENTS.md discovery, <environment_context>, TurnDiffTracker
crates/core          locagens-core (lib locagens_core):
  src/api/             typed in-process facade (runs, projects, providers, permissions, ...)
  src/orchestrator/    run driver (mod.rs), llm.rs streaming client, tools.rs, compact.rs
  src/services/        processes, plugins, hooks, skills, symbols, mcp
  src/cli/             lcgn implementation (args, repl, runner, render, auth, models)
  src/db.rs            file-based JSON/JSONL store
apps/cli             locagens-cli, binary `lcgn` (thin entry -> locagens_core::cli)
apps/desktop-rs      locagens-desktop, binary `locagens` (Dioxus 0.7 UI)
```

Docs: `docs/CORE_API.md` (api facade), `docs/CLI.md` (`lcgn`),
`docs/database-schema.md` (data layout), `docs/RELEASING.md`.

```bash
cargo check --workspace
cargo test --workspace
cargo run -p locagens-desktop
cargo run -p locagens-cli --bin lcgn -- -p "say OK"
```

## Principles

Keep it simple: no agent frameworks, readable code, small purpose-driven
functions, no premature abstraction.

```txt
Core:      api facade -> orchestrator/services (logic) -> db/providers (I/O).
           Facade functions hold no storage logic.
Desktop:   main.rs -> api facade -> presentational Dioxus components.
Contracts: Rust structs in locagens-protocol / locagens-core, shared by
           desktop and CLI.
Naming:    specific (PiProviderManager, Orchestrator, build_core);
           never helper/manager/processor/data.
```

## Providers & Secrets

- Agent config dir chain: `~/.locagens` (`$LOCAGENS_HOME`), then `~/.codex`
  (`$CODEX_HOME`) and `~/.pi/agent` (`$PI_CODING_AGENT_DIR`) as fallbacks
  (`auth.json`, `models.json`, `config.toml`). Never stored in the project or
  app data.
- API keys only in `auth.json` (literal, `!command`, `$ENV_VAR`, OAuth) or env
  vars. Never log or expose them; settings never return raw keys (preserve
  marker `__LOCAGENS_PRESERVE_API_KEY__`).
- `llm.rs` streams OpenAI-compatible, Anthropic Messages and OpenAI Responses
  APIs. No built-in pricing table: unpriced models cost 0; cost goes to
  `usage` lines of the run rollout.

## Runtime

- `run()` starts fresh, `continue_run()` replays history; both use the same
  drive loop for the main model and every sub-agent. Cancellation is
  cooperative and resolves any pending permission/question.
- Modes: `chat`, `plan`, `accept_edits`, `ask_permissions`, `auto`. The UI
  offers Build (`accept_edits`), Plan and Full Access (`auto`). Modes map to
  Codex `ApprovalPolicy` / `SandboxMode` in `locagens-protocol`.
- Tools live in `orchestrator/tools.rs`. All paths must stay inside the
  project workspace. Commands (`exec_command`, `run_command`) and `fetch_url`
  are permission-gated unless a standing grant matches or the command is
  known-safe; dangerous commands always prompt. Sub-agent depth is capped at 1.
- Permission answers: `allow_once | allow_project | allow_always | allow_run |
  deny`; standing grants persist in `permissions.json`.
- Run states: `created generating awaiting_permission awaiting_input done
  failed cancelled`.
- Context is auto-compacted above ~90k estimated input tokens (last 8
  messages kept).

## Persistence

Codex-style files under the Locagens home `~/.locagens` (`$LOCAGENS_HOME`;
`LOCAGENS_DATA_DIR` overrides the root for tests): one append-only JSONL
rollout per run at `sessions/YYYY/MM/DD/rollout-<ts>-<run_id>.jsonl`
(`session_meta`, `run_update`, `message`, `message_update`, `plan`, `usage`
lines; updates are appended, load replays, a corrupt last line is skipped),
plus `session_index.jsonl`, `history.jsonl`, `memories/*.md` and
`projects.json` / `permissions.json`. The legacy `<app_support>/Locagens/data/`
is imported once on first open and never modified. See
`docs/database-schema.md`. Every message persists and runs must reopen after
restart; startup marks stuck active runs `failed`. Errors go to the run's
`error_message` without dropping prior messages. No telemetry.

## Development Workflow (Claude Code)

Pick the sub-agent model by task type:

```txt
haiku    codebase exploration (read-only research & lookups)
sonnet   development with a clear, well-defined scope (start and end known)
opus     only for genuinely hard work (deep design, tricky debugging)
```

## Not Implemented

Do not build without an explicit request: reviewer/acceptance loop after
sub-agent fan-out, direct git integration, extra approval checkpoints beyond
the permission flow, multi-user auth / cloud deployment.
