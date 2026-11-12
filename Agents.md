# Agents.md

## Purpose

How the agent works inside Locagens at runtime.

A run is a single workspace agent: the user picks a provider/model and a
project folder, then the model works the task with workspace tools until it
returns a final answer. The main agent can fan work out with `spawn_agents`
(`explorer` read-only research / `worker` full workspace tools) using the
run's own model. There are no agent presets and no coder/utility tiers.

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

## Agent Behavior

The agent understands the request and acts inside the active project. Plan
when it helps, inspect before changing, use judgment — that is all.

A few things it never does: leave the workspace, invent tool behavior, or hide
failures. It says so when it changes scope or takes a destructive action, and
stops when done or blocked.

## Modes

The run's `mode` shapes the prompt and gating:

```txt
chat             Conversation only; no proactive workspace work.
plan             Plan only; no mutation or commands.
accept_edits     Build; edits applied directly.
ask_permissions  Every tool call approved unless already granted.
auto             Autonomous build; dangerous tools still gated.
```

The UI offers Build (`accept_edits`), Plan, and Full Access (`auto` — prompts
nothing). The rest remain valid for older runs and API callers.

## Tools

From `workspaceTools.ts`:

```txt
write_file  edit_file  delete_file  read_file  list_directory
create_directory  move_file  search_files
run_command(command, network_domains?)  fetch_url(url)   dangerous
set_chat_title  ask_user_question  remember  load_skill
update_plan                                 plan mode
spawn_agents (explorer / worker)            single-model fan-out
```

All paths are relative to the project folder and confined to it; failures
return JSON for the model to react to. `run_command` and `fetch_url` pause for
approval unless a standing grant matches (commands: exact command + domain
set; fetches: per host). The bookkeeping tools (`update_plan`, `set_chat_title`,
`remember`) run silently. `load_skill` pulls a SKILL.md body from the user or
project skills directory (Settings → Skills). Plugins (Settings → Plugins) can
add MCP servers, custom tools, and lifecycle hooks.

## Permission Flow

```txt
gated call -> awaiting_permission + permission_requested (with preview)
-> allow_once | allow_project | allow_always | allow_run | deny
-> grants saved in the permissions table (revocable in Settings)
```

`deny` returns a permission error to the model; `allow_run` silences prompts
for the rest of the run.

## Generation Loop

```txt
build mode-aware prompt -> call model with history + tools -> stream output
-> gate/execute tool calls, feed results back -> repeat until no tool calls
```

`run()` starts fresh, `continueRun()` replays history; both share `drive()`
and the same `runAgentLoop()` for the main model and every sub-agent. The loop
ends on the final answer, cancellation, or error.

## Safety

The orchestrator enforces: workspace path containment (including symlink
checks), permission gating for dangerous tools, fail-closed command sandbox
with deny-by-default network, SSRF-safe agent HTTP, cooperative cancellation,
provider timeouts, startup cleanup of stuck runs, and error persistence that
never discards prior messages. Provider config lives in pi's agent directory
(`~/.pi/agent/auth.json`, `models.json`; keys are masked in metadata) and raw
secrets never reach the UI.

## Done Definition

```txt
one provider/model + one project folder is enough to run a task
single-model runs with `spawn_agents` (explorer / worker) behave
predictably per mode
all file access stays in the workspace; dangerous tools pause and resume
messages, status, permissions, and plans stream live and survive restarts
provider secrets stay local and masked
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
