# Claude.md

## Project

Locagens is a local-first AI orchestration tool. The user picks one provider
and model, gives it a task, and the assistant works inside a selected project
folder. Runs are single-model: one provider/model drives the whole run, and the
main agent can fan work out to sub-agents via `spawn_agents` (`explorer`
read-only / `worker` full tools). The UI is a native Rust desktop app (Dioxus).
There are no agent presets and no coder/utility model tiers.

```txt
User Task -> Orchestrator creates a run -> main model responds (with tools)
-> tool calls gated by mode/permissions -> results fed back -> loop until
no tool calls -> run done; messages stream live and persist.
```

## Development & Agent Workflow (pi + Gemini)

How development work is executed in this repo:

- **Codebase exploration (read-only research & analysis)**: always use
  `gemini-4-hydrogen` (`Gemini 4 Hydrogen`), default effort `medium` (`low` for
  trivial lookups, `high`/`xhigh` for deep analysis), run as a read-only
  subagent or swarm worker pinned to a `gemini-4-hydrogen-*` model. Never burn a
  coding model on exploration.
- **All coding, development & file modifications**: always execute via the
  `pi` CLI using model `opencode-go/glm-5.3-flash`.

### Non-interactive `pi` CLI invocation (Windows PowerShell)

Always close stdin (`$null |`) and pass `--no-mcp --no-extensions --approve -p`.
For multi-line tasks on Windows, either pass a single-line prompt or pass a
`@prompt.md` file reference so Windows CLI argument parsing does not split on
newlines:

```powershell
$null | pi --model opencode-go/glm-5.3-flash --no-mcp --no-extensions --approve -p @path/to/prompt.md "Execute the instructions in the attached prompt file."
```

Or for short single-line prompts:

```powershell
$null | pi --model opencode-go/glm-5.3-flash --no-mcp --no-extensions --approve -p "Single line instruction"
```

### Locagens runtime architecture direction

Locagens is **100% pure Rust**: `apps/backend` (`locagens_core` library +
`lcgn` CLI) and `apps/desktop-rs` (native Dioxus 0.7 desktop app, binary
`locagens`), built directly on top of `pi` (in-process; `pi auth` /
`~/.pi/agent/auth.json`, `models.json`). Consumers call the typed in-process
facade `locagens_core::api` (see `docs/CORE_API.md`). There is no Node
backend, no Electron shell, no Vue web app, and no HTTP server.

## Tech Stack

```txt
Desktop: native Rust app (Dioxus 0.7, apps/desktop-rs, binary `locagens`)
Core:    Rust (Tokio) built directly on top of pi (apps/backend,
         `locagens_core` library + `lcgn` CLI binary)
API:     in-process typed facade (locagens_core::api, docs/CORE_API.md)
Storage: file-based JSON/JSONL under <app_support_dir>/data/ (no SQLite)
Realtime: in-process run event channels (no SSE, no HTTP)
Packages: cargo only (backend/desktop); no Node.js, pnpm, or Electron
```

## Project Map

Consult these before reading source:

- **`docs/CORE_API.md`** — typed in-process core API reference (`locagens_core::api`)
- **`docs/CLI.md`** — `lcgn` command-line reference
- **`docs/database-schema.md`** — file-based JSON/JSONL data layout

## Repository Structure

```txt
apps/
  backend/                Rust core (locagens_core library + lcgn CLI)
    src/
      api/                typed in-process facade: runs, projects, providers,
                          permissions, memory, plugins, processes, skills,
                          symbols, security, mcp (error.rs ApiError)
      cli/                lcgn CLI: args, repl, runner, render, auth, models
      bin/lcgn.rs         CLI entry point
      orchestrator/       Orchestrator (run lifecycle + drive loop), llm.rs,
                          tools.rs (workspace tools)
      services/           processes, plugins, skills, symbols, mcp
      db.rs               file-based JSON/JSONL store
      paths.rs            settings/data dir resolution
      pi_providers.rs     ~/.pi/agent auth.json + models.json engine
      state.rs            shared AppState
      util.rs             timestamps + canonical paths
  desktop-rs/             native Dioxus 0.7 desktop app (binary `locagens`)
    src/
      main.rs             bootstrap -> locagens_core::api -> UI
      components/         Dioxus UI components
      state_ui.rs  theme.rs  markdown.rs
docs/                     CORE_API.md, CLI.md, database-schema.md, RELEASING.md
```

## Core Principles

Keep the architecture simple. No heavy agent frameworks. Readable code over
clever abstractions. Small, purpose-driven functions. No premature abstraction.

```txt
Core:      api facade (typed) -> orchestrator/services (logic)
           -> db/providers (I/O). Facade functions hold no storage logic.
Desktop:   main.rs -> api facade -> Dioxus components. Components are
           presentational; state flows through the shared AppState.
Contracts: Rust structs in locagens_core (db.rs, api payloads) shared by
           both the desktop app and the lcgn CLI.
Naming:    specific (PiProviderManager, Orchestrator, build_core);
           never helper/manager/processor/data.
```

## Providers

- Config lives in pi's agent directory (`~/.pi/agent/auth.json`, `models.json`,
  `models-store.json`; override via `PI_CODING_AGENT_DIR`), handled by
  `pi_providers.rs` (`PiProviderManager`). Never in the project.
- API keys live only in `auth.json` (literal, `!command`, `$ENV_VAR`, or
  OAuth) or environment variables. Never log or expose secrets. Metadata only
  returns safe fields; settings never return raw keys (preserve marker
  `__LOCAGENS_PRESERVE_API_KEY__`).
- `modelSettings[model]`: `reasoning`, `contextLimit`, `pricing` tiers
  (USD per 1M tokens). No built-in pricing table — unpriced models cost 0; cost
  is computed locally into `usage_logs`.
- Adapters implement `ModelProvider.complete(request)`. The orchestrator only
  sees the common `ToolCall` shape; `OpenAICompatibleProvider` and
  `AnthropicProvider` map to/from their wire formats. `reasoningWire.ts`
  converts the UI effort level to each model's reasoning parameter.

## Orchestrator

`orchestrator/mod.rs` owns the run driver. `run()` starts fresh,
`continue_run()` replays history; both share the same drive loop for the main
model AND every sub-agent — tool execution stays shared, never duplicated.

```txt
load run -> build mode-aware prompt
-> pick tools from mode -> call model
-> gate each tool call, execute, feed results back
-> finish when no tool calls, on cancel, or on error
```

Cancellation is cooperative (`checkCancelled`) and resolves any pending
permission/question so the loop unwinds.

Modes shape the prompt and tool gating (`chat`, `plan`, `accept_edits`,
`ask_permissions`, `auto`). The desktop UI offers Build (`accept_edits`), Plan,
and Full Access (`auto` — nothing is prompted). In plan mode, `update_plan` is
a stable document (right panel); live progress is the per-message
`<task_list>`.

## Tools

Defined in `orchestrator/tools.rs`. All paths must resolve inside the run's
project workspace; failures return error payloads the model can react to.

```txt
write_file  edit_file  delete_file  read_file  list_directory
create_directory  move_file  search_files
run_command(command)   fetch_url(url)          dangerous: permission-gated
set_chat_title  ask_user_question  remember  load_skill   orchestrator tools
update_plan                                  plan mode only
spawn_agents (explorer / worker)             single-model fan-out
```

- `run_command` (workspace cwd, env secrets stripped) and `fetch_url`
  (GET, truncated) always pause for approval unless a standing grant
  matches; `fetch_url` grants are per-host. The command sandbox is
  deny-by-default for network domains unless granted.
- Silent tools (no I/O, no prompt): `set_chat_title`, `update_plan`, `remember`.
  `ask_user_question` pauses the run (`awaiting_input`). `load_skill` returns a
  `SKILL.md` body from user/project skill roots (sub-agents get no skills).
- `spawn_agents` starts nested `runAgentLoop()` calls in the same run (messages
  tagged `agentRole` + `agentName`, rendered as sub-agent windows). Depth is
  capped at 1: sub-agents never receive `spawn_agents`. `explorer` sub-agents
  are read-only; `worker` sub-agents get full workspace tools.

## Permission Flow

```txt
gated tool call -> status awaiting_permission + permission_requested
(with PermissionPreview: action, path, old/new content)
-> allow_once | allow_project | allow_always | allow_run | deny
-> standing grants persisted in the permissions table (revocable in Settings)
```

`allow_run` also silences prompts for the rest of the run (in-memory).

## Run States

```txt
created  generating  awaiting_permission  awaiting_input  done  failed  cancelled
```

## Persistence

File-based JSON/JSONL under the OS app-support directory (`<app_support>/data/`):
`projects.json`, `runs/`, per-run `sessions/<run_id>/…` message + event
streams, `permissions.json`, `plans/`, `memory.json`, `usage_logs.jsonl` —
see `docs/database-schema.md`. All agent messages persist; a run must be
reopenable after restart. Startup marks stuck active runs `failed`. Provider
credentials never enter the app data.

## Realtime

In-process run event channels (no HTTP, no SSE) fanned out to the desktop UI
and the CLI:

```txt
run_started  status_changed  message_created  message_updated
model_snapshot_locked  permission_requested  plan_updated
question_requested  run_title_changed  run_completed  run_failed
```

Subscribers receive live events for active runs; late subscribers of
terminal runs get a replay of the final event.

## Error Handling

Provider config/model errors, invalid keys, timeouts (idle 300s), network
errors, rate limits, cancellations, and permission denials are stored in the
run's `error_message` — prior messages are always kept.

## Security Rules

```txt
API keys only in ~/.pi/agent/auth.json or env vars; never in app data/logs
Public provider metadata never exposes secrets
All file access constrained to the active project workspace
locagens.db stays out of git
No telemetry or external tracking unless explicitly requested
```

## Development Workflow

All work is strictly handled by:

```txt
gemini-4-hydrogen           exploration (read-only research & analysis)
pi CLI (glm-5.3-flash)      all coding & development via pi
```

See the "Development & Agent Workflow" section above for the exact
invocation pattern. The old `.claude/agents/` sub-agent definitions have
been removed.

## Future Features

Not implemented; do not build without an explicit request:

```txt
separate reviewer/acceptance loop after sub-agent fan-out
direct git integration
manual approval checkpoints beyond the permission flow
multi-user auth / cloud deployment
```

## Phase 9 — Codex Harness Architecture (implemented)

Locagens now ships the Codex tool harness (ported from `codex-rs`):

- **`apply_patch`** (`orchestrator/apply_patch.rs`): full `*** Begin Patch` …
  `*** End Patch` parser (add / delete / update / move, multi-chunk updates,
  `@@ context`, `*** End of File`), the fuzzy `seek_sequence` matcher (exact →
  `trim_end` → `trim` → Unicode-punctuation normalization), workspace-safe
  application, `preview_patch` and Codex's exact
  `Success. Updated the following files:` summary. Every target/move path is
  validated through `safe_path`, so patches cannot escape the workspace.
- **`exec_command` / `write_stdin`** (unified exec): commands run through the
  ProcessManager with a secret-stripped environment; they return
  `{ exit_code, output, wall_time_ms }` when they finish within
  `yield_time_ms` (default 10000 ms, clamped 500..60000) and otherwise
  register an interactive session the model can drive with `write_stdin`.
- **Command safety & policy** (`orchestrator/command_safety.rs`): Codex's
  `ApprovalPolicy` (`untrusted` / `on-request` / `on-failure` / `never`) and
  `SandboxMode` (`read-only` / `workspace-write` / `danger-full-access`)
  mapped from Locagens modes (`chat`/`plan` → read-only, `accept_edits` →
  workspace-write + on-request, `ask_permissions` → untrusted, `auto` →
  danger-full-access + never). Known-safe read-only commands (`git status`,
  `rg`, `cargo check`, …) run without prompting in `accept_edits`; dangerous
  commands (`rm -rf`, `git reset --hard`, `sudo`, …) always prompt.
- **Hierarchical `AGENTS.md` discovery** (`orchestrator/agents_md.rs`): walks
  from the git root down to the cwd picking at most one doc per level
  (`AGENTS.override.md` > `AGENTS.md` > `CLAUDE.md` > `.agents.md`), joined
  with `

--- project-doc ---

` under a 32 KiB budget and injected into
  the system prompt.
- **`<environment_context>`** (`orchestrator/env_context.rs`): Codex-format
  XML block (cwd / shell / approval_policy / sandbox_mode / network_access /
  workspace_roots / current_date) seeded as the first user message of a run.
- **`TurnDiffTracker`** (`orchestrator/turn_diff_tracker.rs`): snapshots file
  baselines before mutations (`apply_patch`, `write_file`, `edit_file`,
  `delete_file`, `move_file`) and streams `turn_diff_updated` events with the
  aggregate unified diff and changed-file count.
- **Context compaction** (`orchestrator/compact.rs`): automatic compaction
  when the estimated input tokens exceed 90k (also on demand via
  `compact_context`): older history is summarized with Codex's
  SUMMARIZATION_PROMPT and replaced by a `[Compacted context summary]` block
  while the last 8 messages stay intact.
- **Codex sub-agent tools**: `spawn_agent` (background agent with id),
  `wait_agent`, `send_input`, `close_agent`, `list_agents`, plus
  `request_user_input` (Codex question schema) and `view_image`.
- **Config & auth**: the primary agent directory is now `~/.locagens`
  (`$LOCAGENS_HOME`); `~/.codex` (`$CODEX_HOME`: `auth.json` OPENAI_API_KEY
  and `config.toml` `[model_providers.*]`) and the legacy `~/.pi/agent`
  directory are read as credential/config fallbacks.
- **`.vscode/tasks.json` + `.vscode/launch.json`**: w64devkit PATH, rustup-run
  GNU toolchain tasks (check/build/test/run) and cppdbg/gdb + CodeLLDB debug
  configurations for the desktop app and the `lcgn` CLI.
