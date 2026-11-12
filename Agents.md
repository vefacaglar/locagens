# Agents.md

## Purpose

How the agent works inside Locagens at runtime.

A run is a single workspace agent by default: the user picks a provider/model
and a project folder, then the model works the task with workspace tools until
it returns a final answer. Optional **presets** turn the main model into an
architect that delegates implementation to a coder model (`delegate_tasks`)
and small chores to a utility model (`delegate_to_utility`). Runs without a
preset can fan work out with `spawn_agents` (explore/general) using the run's
own model.

## Development & Agent Workflow (pi + Gemini)

How development work is executed in this repo:

- **Codebase exploration (read-only research & analysis)**: always use
  `gemini-3.8-flash` (`Gemini 3.8 Flash` / the `research` subagent with
  `Model: "flash"`). Never burn a coding model on exploration.
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

The backend is migrating 100% to **Rust (`Axum` + `Tokio` + `Rusqlite`)** built
directly on top of `pi` (`pi --mode rpc`, `pi auth` /
`~/.pi/agent/auth.json`, `models.json`). All new backend, provider, and auth
work targets the Rust + `pi` model; the Node/Fastify backend is legacy.

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
delegate_tasks / delegate_to_utility        preset build modes
spawn_agents                                single-model runs
```

All paths are relative to the project folder and confined to it; failures
return JSON for the model to react to. `run_command` and `fetch_url` pause for
approval unless a standing grant matches (commands: exact command + domain
set; fetches: per host). The bookkeeping tools (`update_plan`, `set_chat_title`,
`remember`) run silently. `load_skill` pulls a SKILL.md body from the user or
project skills directory (Settings → Skills). Plugins (Settings → Plugins) can
add MCP servers, custom tools, and lifecycle hooks.

In preset runs the architect cannot mutate files directly — mutations go
through coder sub-agents; utility sub-agents get read/list/search/move only.

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
never discards prior messages. Provider config lives in OS app-support
(keychain-backed secrets, `apiKeyRef` in JSON) and raw secrets never reach the
browser.

## Done Definition

```txt
one provider/model + one project folder is enough to run a task
presets and single-model delegation behave predictably per mode
all file access stays in the workspace; dangerous tools pause and resume
messages, status, permissions, and plans stream live and survive restarts
provider secrets stay local and masked
```
