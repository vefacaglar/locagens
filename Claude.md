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
  project workspace (dangling symlinks are refused). Commands (`exec_command`,
  `run_command`, `write_stdin`) are permission-gated outside Full Access unless a standing
  grant matches or, in Build only, the command is known-safe or `.rules`-allowed; `fetch_url` prompts in every
  mode except Full Access and only reaches public addresses (redirects are
  re-validated, DNS is pinned). Outside Full Access, dangerous commands and
  mutations of protected paths (`.git`, `.locagens`, `.codex`, `.agents`;
  `execpolicy::protected_paths`) always prompt; no grant or Allow-for-Run
  covers them. Calls to tools that were not offered to the agent are
  rejected. Unknown modes are rejected by the api facade and fail closed to
  read-only in `locagens-protocol`. Sub-agent depth is capped at 1.
- Permission answers: `allow_once | allow_project | allow_always | allow_run |
  deny`; anything else is `deny`. `allow_run` covers only that tool for the
  rest of the run. Standing grants persist in `permissions.json`; the project
  `.rules` files are never written.
- Project trust (Codex-style): `[projects."<path>"] trust_level` in
  `~/.locagens/config.toml` (`services/project_trust.rs`, nearest ancestor
  wins). Untrusted projects load no project hooks, MCP servers, plugins,
  `.rules` or agent roles. Hooks and stdio MCP servers get the stripped
  exec-session environment, not the app's env.
- Run states: `created generating awaiting_permission awaiting_input done
  failed cancelled`.
- Context is auto-compacted above ~90k estimated input tokens (last 8
  messages kept).
- Agents (Codex multi-agent v2, `orchestrator/agents.rs` + `agent_roles.rs`):
  `spawn_agent {task_name, message, agent_type}` starts a persistent named
  agent; `send_message` queues text (no turn), `followup_task` starts/queues a
  turn, `wait_agent {timeout_ms}` returns mailbox updates (turn outcomes with
  output), `interrupt_agent`, `close_agent`, `list_agents`. Max 4 live agents
  (completed ones hold a slot until closed). Roles: built-in `explorer`
  (read-only, default) and `worker`, plus `agents/*.toml` in
  `<project>/.locagens/` and the agent dirs; plan/chat force read-only.
  Children share the parent's `RunHandle`: parent cancel cancels them, their
  permission prompts queue through the parent's permission flow, and their
  messages carry `agentRole`/`agentName`. Children get no agent tools (depth
  1). `spawn_agents` is the one-shot parallel batch on the same hub: its
  agents take slots, it waits for each first turn, then closes them.
- Drive lifecycle: one drive (and one fresh `RunHandle`) per turn;
  `continue_run` is a conflict while a drive is live (queue instead).
  Cancellation is final for a handle (never reset); `cancel_run` waits up to
  5 s for the drive, then aborts it. A supervisor marks a panicked drive
  `failed` and always runs cleanup (close agents, drop handle, kill the run's
  exec sessions unless the turn ended `done`). Long awaits (LLM call, exec
  polling) race cancellation. LLM streams use one shared client, no total
  timeout, 300 s idle timeout, and split SSE on raw bytes before decoding.
- All permission prompts (any agent, network approvals) go through
  `RunHandle::ask_permission`: one queue, one slot owned by prompt id, a drop
  guard that clears it. Grants are persisted only by the asking side. Network
  "Allow for Run" hosts live on the handle (current turn only).
- Byte-budget string cuts use `locagens_protocol::text` (char-boundary safe).
- `/undo` leaves an `event` message telling the model; it reverts only paths the turn changed (pre- vs post-turn tree),
  skips paths changed since, and saves a `<run_id>#undo` backup snapshot.
- Other tools: `tool_search` (more than 8 MCP tools are deferred; matches are
  added to the next request), `get_context_remaining`, `host_executable`
  (PATH/PATHEXT lookup). `apply_patch` args are parsed while streaming and
  reported as `patch_progress` events.

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
