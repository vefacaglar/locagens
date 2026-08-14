# Agents.md

## Purpose

This file describes how the agent currently works inside Locagens.

Locagens normally runs a **single workspace agent**. The user selects one
provider/model, chooses a local project folder, sends a task, and the
orchestrator lets that model respond, call workspace tools when appropriate,
read tool results, and continue until it returns a final answer.

Locagens also supports optional **agent presets**. A preset turns the main
model into an architect/planner and gives it a separate coder model for
implementation through `delegate_tasks`. Presets may also include a lightweight
utility model for small read/search/rename chores through `delegate_to_utility`.
When no preset is selected, the run stays plain single-model.

---

## Agent Role

The workspace agent should understand the user's request and act inside the
active project directory.

Responsibilities:

```txt
answer normal questions concisely
plan before implementation work
inspect files when context is needed
modify files through workspace tools in build modes
surface important decisions and uncertainty
stop when the task is done or blocked
```

The agent must not:

```txt
touch files outside the active project workspace
invent provider behavior or tool capabilities
expand scope beyond the user's request without saying so
silently perform destructive actions
hide tool failures
```

## Project Map

Before making changes, consult these reference documents to minimize token usage:

- **`apps/api/src/routes/API_ENDPOINT_MAP.md`** — HTTP API reference (30 endpoints, route handlers, repository calls)
- **`docs/database-schema.md`** — SQLite schema (7 tables, columns, indexes, migrations)

These maps show file locations and dependencies without reading source code.

---

---

## System Prompt

The system prompt is built per run by `buildSystemPrompt(projectName,
projectPath, mode, shouldReadProjectGuidance, delegation)` in
`apps/api/src/orchestrator/systemPrompt.ts`.

The prompt always includes the active project context when available. Non-chat
modes include the workspace tool catalog and planning/checklist instructions.
Chat mode intentionally stays lightweight and tells the model not to scan or
modify the workspace unless the user explicitly asks.

When a coder preset is active, the main prompt is architect-oriented and the
main model receives read-only workspace tools plus delegation tools. Coder and
utility sub-agents use their own prompts from the same file.

---

## Operational Modes

The run's `mode` controls the prompt and permission behavior:

```txt
chat             Lightweight conversation; no proactive workspace work.
plan             Planning only; no file mutation or commands.
accept_edits     Build mode; file edits are applied directly.
ask_permissions  Every tool call requires user approval unless already granted.
auto             Autonomous build mode; still gates dangerous tools.
```

The web UI currently exposes Chat, Build (`accept_edits`), and Plan. Older runs
or API callers may still use `ask_permissions` and `auto`.

---

## Workspace Tools

The agent is offered these tools from `apps/api/src/orchestrator/workspaceTools.ts`:

```txt
write_file(path, content)
edit_file(path, old_string, new_string, replace_all?)
delete_file(path)
read_file(path)
list_directory(path)
create_directory(path)
move_file(source_path, destination_path)
search_files(query, path?)
run_command(command, network_domains?)
fetch_url(url)
update_plan(title, tasks, body?, start_new?)   plan mode only
set_chat_title(title)
delegate_tasks(tasks, parallel?)               preset build modes only
delegate_to_utility(tasks, parallel?)          preset build modes only
```

Rules:

```txt
all paths are relative to the active project folder
paths that resolve outside the workspace are rejected
tool failures return JSON, and the model should react to them
prefer read/search/list before editing when context is missing
prefer edit_file for targeted changes to existing files
```

`run_command` and `fetch_url` are dangerous tools. They always route through the
permission flow unless a matching standing grant exists. `update_plan` is
internal bookkeeping only; it does not touch files or network and runs without a
permission prompt. `set_chat_title` also runs silently because it only renames
the run.

Commands run through the platform sandbox with workspace-only writes and no
network by default. A networked command must declare its domains in advance;
standing grants match the exact command and exact normalized domain set.

In preset runs, the architect cannot mutate files or run commands directly. It
delegates code-writing to coder sub-agents. Utility sub-agents receive read,
list, search, and move tools only.

---

## Permission Flow

When a gated tool call needs approval, the orchestrator pauses:

```txt
1. status becomes "awaiting_permission"
2. a permission_requested event is emitted with a preview
3. the user decides: allow_once | allow_project | allow_always | deny
4. project/global grants are saved in the permissions table
5. deny returns a JSON permission error to the model
```

Standing grants are scoped by tool. `run_command` grants are scoped to the exact
command string and exact normalized network-domain set; `fetch_url` grants are
scoped to the host. `allow_run` approves only the same permission identity for
the remainder of that run.

---

## Generation Loop

The orchestrator drives the run in `Orchestrator.ts`:

```txt
1. build the mode-aware system prompt
2. call the selected provider/model with the current history and tools
3. stream and save assistant output
4. if tool calls are returned, gate/execute/save each tool result
5. append tool results to model history and continue
6. if no tool calls are returned, mark the run done
```

`run()` starts a fresh thread. `continueRun()` replays stored history and
continues the same thread. Both use `drive()`, which delegates actual model
turns to the shared `runAgentLoop()`.

In a preset run, the main loop is the architect. `delegate_tasks` and
`delegate_to_utility` start additional `runAgentLoop()` calls inside the same
run and workspace, tagged as coder or utility messages. The loop ends when the
main model stops calling tools, the user cancels, or an error occurs.

---

## Safety

The orchestrator enforces:

```txt
workspace path containment
canonical registered-project validation and symlink boundary checks
permission gating for dangerous tools and ask_permissions mode
fail-closed command sandboxing with deny-by-default network access
SSRF-safe agent HTTP with redirect, DNS, timeout, and body limits
cooperative cancellation between steps
provider timeout per request
startup cleanup for runs left in active states
error persistence without discarding prior messages
```

Provider settings live in local configuration outside the project by default.
On macOS, provider secrets live in Keychain and the JSON config keeps only a
secret reference. Public provider lists expose only safe metadata; the local
settings API returns masked provider config and can preserve existing secrets
while saving edits.

---

## Done Definition

The current agent system is working when:

```txt
the user can choose one provider/model and one project folder
the user can optionally choose an architect/coder/utility preset
chat/build/plan modes behave differently and predictably
the agent can read/write workspace files through tools
the architect delegates mutations when a coder preset is active
all filesystem access stays inside the selected workspace
dangerous tools pause for approval and resume correctly
messages, status changes, permissions, and plans stream live and persist
runs can be reopened after restart
provider secrets stay in local config and raw values are not exposed to the browser
```
