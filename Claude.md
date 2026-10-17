# Claude.md

## Project

Locagens is a local-first AI orchestration tool. The user picks one provider
and model, gives it a task, and the assistant works inside a selected project
folder. Runs are single-model by default. An optional agent preset turns the
main model into an architect that delegates to coder/utility sub-agents; runs
without a preset can fan work out via `spawn_agents` (explore/general).

```txt
User Task -> Orchestrator creates a run -> main model responds (with tools)
-> tool calls gated by mode/permissions -> results fed back -> loop until
no tool calls -> run done; messages stream live and persist.
```

## Tech Stack

```txt
Frontend: Vue 3 + Vite + TypeScript (Composition API, <script setup>)
Backend:  Node.js + TypeScript + Fastify
Database: SQLite via the built-in node:sqlite module
Realtime: SSE (Server-Sent Events)
Packages: pnpm workspaces, monorepo
```

## Project Map

Consult these before reading source:

- **`apps/api/src/routes/API_ENDPOINT_MAP.md`** — HTTP API reference (endpoints, handlers, repository calls)
- **`docs/database-schema.md`** — SQLite schema (tables, columns, indexes, migrations)

## Repository Structure

```txt
apps/
  api/                    Fastify backend
    src/
      server.ts           Bootstrap: Fastify + CORS + listen
      context.ts          Shared dependencies (registry, repos, orchestrator)
      routes/             providers, projects, runs, permissions, memory,
                          plugins, processes, skills, symbols, security, settings
      orchestrator/       Orchestrator.ts (run lifecycle + drive loop),
                          AgentLoop.ts, DelegationCoordinator.ts,
                          PermissionCoordinator.ts, QuestionCoordinator.ts,
                          RunMessageStream.ts, UserMessageInbox.ts,
                          rebuildHistory.ts, contextWindow.ts, pricing.ts,
                          workspaceTools.ts + workspace/ (tool exec, pathGuards,
                          networkTools, permissionPreview), prompts/ (system +
                          sub-agent prompts), processes/, plugins/, mcp/, skills/
      providers/          ModelProvider, OpenAICompatibleProvider,
                          AnthropicProvider, ProviderFactory, ProviderRegistry,
                          ProviderSecretStore, reasoningWire
      security/           CommandSandbox
      database/           db.ts (schema+migrations), repositories/ (one per table)
  web/                    Vue frontend
    src/
      App.vue             Thin shell -> useAppShell
      api/client.ts       Typed fetch wrappers
      composables/        useAppShell, useChatSession, useProjects,
                          usePermissions, useComposerSettings, ...
      lib/                markdown, format, messageGroups, confirmation
      components/         MessageThread, ToolGroup, CoderGroup, ChatComposer,
                          PermissionCard, ConfirmationCard, PlanPanel,
                          AppSidebar, settings/
  desktop/                Electron shell + electron-builder
  db-writer/              SQLite write worker
packages/
  shared/src/index.ts     Shared TS contracts (Run, RunMessage, RunEvent, ...)
docs/                     database-schema.md, RELEASING.md
```

## Core Principles

Keep the architecture simple. No heavy agent frameworks. Readable code over
clever abstractions. Small, purpose-driven functions. No premature abstraction.

```txt
Backend:  routes (HTTP) -> orchestrator/repositories/registry (logic)
          -> database/providers (I/O). Routes hold no provider/SQL logic.
Frontend: App.vue -> useAppShell -> domain composables -> api/ + lib/.
          Components are presentational (props/emits).
Contracts: shared types from packages/shared across all boundaries.
Naming:   specific (ProviderRegistry, RunRepository, buildSystemPrompt);
          never helper/manager/processor/data.
```

## Providers

- Config lives in OS app-support `providers.json` (override:
  `LOCAGENS_PROVIDER_CONFIG_PATH`; first-run seed:
  `LOCAGENS_PROVIDER_SEED_PATH`). Never in the project.
- API keys never touch JSON or the database: macOS Keychain behind a non-secret
  `apiKeyRef`. Never log or expose secrets. Public metadata endpoints return
  only safe fields; the settings API never returns raw keys (preserve marker).
- `agentPresets`: architect + coder model, `maxSubAgents`, optional utility
  model. `modelSettings[model]`: `reasoning`, `contextLimit`, `pricing` tiers
  (USD per 1M tokens). No built-in pricing table — unpriced models cost 0; cost
  is computed locally into `usage_logs`.
- Adapters implement `ModelProvider.complete(request)`. The orchestrator only
  sees the common `ToolCall` shape; `OpenAICompatibleProvider` and
  `AnthropicProvider` map to/from their wire formats. `reasoningWire.ts`
  converts the UI effort level to each model's reasoning parameter.

## Orchestrator

`Orchestrator.ts` owns one run driver. `run()` starts fresh, `continueRun()`
replays history; both share `drive()`, which uses `runAgentLoop()` for the main
model AND every sub-agent — tool execution stays shared, never duplicated.

```txt
load run -> build mode-aware prompt (systemPrompt.ts)
-> pick tools from mode + preset -> call model
-> gate each tool call, execute, feed results back
-> finish when no tool calls, on cancel, or on error
```

Cancellation is cooperative (`checkCancelled`) and resolves any pending
permission/question so the loop unwinds.

Modes shape the prompt and tool gating (`chat`, `plan`, `accept_edits`,
`ask_permissions`, `auto`). The web UI offers Build (`accept_edits`), Plan, and
Full Access (`auto`, `bypassDangerousGating` — nothing is prompted). In plan
mode, `update_plan` is a stable document (right panel); live progress is the
per-message `<task_list>`.

## Tools

Defined in `workspaceTools.ts`. All paths must resolve inside the run's project
workspace; failures return `{ success: false, error }` to the model.

```txt
write_file  edit_file  delete_file  read_file  list_directory
create_directory  move_file  search_files
run_command(command)   fetch_url(url)          dangerous: permission-gated
set_chat_title  ask_user_question  remember  load_skill   orchestrator tools
update_plan                                  plan mode only
delegate_tasks / delegate_to_utility         preset build modes only
spawn_agents                                 single-model runs
```

- `run_command` (workspace cwd, 120s, env secrets stripped) and `fetch_url`
  (GET, 30s, truncated) always pause for approval unless a standing grant
  matches; `fetch_url` grants are per-host. OS command sandbox only with
  `LOCAGENS_STRICT_COMMAND_SANDBOX=1`.
- Silent tools (no I/O, no prompt): `set_chat_title`, `update_plan`, `remember`.
  `ask_user_question` pauses the run (`awaiting_input`). `load_skill` returns a
  `SKILL.md` body from user/project skill roots (sub-agents get no skills).
- Delegation tools start nested `runAgentLoop()` calls in the same run (messages
  tagged `agentRole` + `agentName`, rendered as sub-agent windows). Depth is
  capped at 1: sub-agents never receive delegation tools. In preset runs,
  architect mutation calls are trapped and redirected to `delegate_tasks`.

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

SQLite via `node:sqlite` (`db.ts` migrations). Tables: `runs`, `messages`,
`projects`, `permissions`, `plans`, `memory`, `usage_logs` — see
`docs/database-schema.md`. All agent messages persist; a run must be reopenable
after restart. Startup marks stuck active runs `failed`. Provider credentials
never enter the database.

## Realtime

SSE at `GET /api/runs/:id/events` fanned out via `eventBus`:

```txt
run_started  status_changed  message_created  message_updated
model_snapshot_locked  permission_requested  plan_updated
question_requested  run_title_changed  run_completed  run_failed
```

Stream closes on `run_completed` / `run_failed` / terminal `status_changed`.

## Error Handling

Provider config/model errors, invalid keys, timeouts (idle 300s), network
errors, rate limits, cancellations, and permission denials are stored in the
run's `error_message` — prior messages are always kept.

## Security Rules

```txt
API keys NEVER in JSON/DB/logs; keychain via apiKeyRef only
Public provider metadata never exposes secrets
All file access constrained to the active project workspace
locagens.db stays out of git
No telemetry or external tracking unless explicitly requested
```

## Sub-Agents

Project development uses Claude Code sub-agents defined in `.claude/agents/`:

```txt
architect           Plan-only before 2+ module changes. No code.
coder               Implements a scoped task; verifies with typecheck/tests.
reviewer            Reviews diffs against this file. Read-only.
searcher            Read-only exploration; paths + short answers.
tester              Runs/writes tests; may only edit test files.
provider-specialist Provider adapters, registry, secrets, pricing.
security-auditor    Permission flow, sandbox, SSRF, path containment.
prompt-engineer     systemPrompt.ts + prompts/ (agent behavior = product).
docs-writer         Keeps API_ENDPOINT_MAP.md, database-schema.md, Claude.md in sync.
debugger            Run triage: errors, states, SSE, SQLite forensics.
release-engineer    Electron packaging, dist pipeline, signing, updates.
```

Delegation etiquette for the main agent:

```txt
wide "where/what/how" question          -> searcher
multi-module or non-trivial design      -> architect, then coder
small single-file fix                   -> just do it yourself
prompt/agent-behavior change            -> prompt-engineer
provider/config/secret work             -> provider-specialist
after a non-trivial diff                -> reviewer (security-sensitive: security-auditor)
after implementation / before done      -> tester
failing run or weird state              -> debugger
docs out of sync with code              -> docs-writer
packaging / release work                -> release-engineer
```

Sub-agent reports return as tool results — summarize them for the user.
Sub-agents share the workspace and must respect the rules above.

## Future Features

Not implemented; do not build without an explicit request:

```txt
separate reviewer/acceptance loop after coder delegation
direct git integration
manual approval checkpoints beyond the permission flow
multi-user auth / cloud deployment
```
