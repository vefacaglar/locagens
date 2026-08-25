# Claude.md

## Project

Locagens is a local-first AI orchestration tool. The user picks one provider
and model, gives it a task, and the assistant works inside a selected project
folder on the local machine.

The default implementation is a **single-model workspace assistant** with
filesystem tools and explicit permission modes. A run can also use an optional
agent preset: the selected main model acts as an architect, delegates
implementation to coder sub-agents, and may delegate tiny mechanical tasks to a
utility model.

```txt
User Task
  -> Orchestrator creates a run
  -> Main model generates a response (optionally calling workspace tools)
  -> Tool calls are gated by the active mode / permissions
  -> If a preset is active, architect can delegate to coder / utility sub-agents
  -> Tool results are fed back to the model
  -> Loop until the main model returns a final answer with no tool calls
  -> Run is marked done; messages are streamed live and saved
```

The current code supports the architect → coder preset path. It does not support
an additional reviewer loop or a separate planner/coder/reviewer acceptance
protocol.

---

## Tech Stack

```txt
Frontend: Vue 3 + Vite + TypeScript (Composition API, <script setup>)
Backend:  Node.js + TypeScript + Fastify
Database: SQLite via the built-in node:sqlite module
Realtime: SSE (Server-Sent Events)
Package manager: pnpm (workspaces)
Repository style: monorepo
```

## Project Map

For quick navigation and to minimize token usage, consult these reference documents before making changes:

- **`apps/api/src/routes/API_ENDPOINT_MAP.md`** — Complete HTTP API reference (30 endpoints with route handlers and repository calls)
- **`docs/database-schema.md`** — Full SQLite schema (7 tables, all columns, indexes, migrations, relationships)

These maps show file locations, dependencies, and data flow without requiring you to read the source code.

---

---

## Repository Structure

```txt
apps/
  api/                         Fastify backend
    src/
      server.ts                Bootstrap only: Fastify + CORS + listen
      context.ts               Shared dependencies (registry, repos, orchestrator)
      routes/
        index.ts               Registers all route groups + /ping
        providers.ts           Provider metadata/config/test/model-fetch routes
        projects.ts            CRUD + macOS folder picker
        runs.ts                Create/continue/cancel/permission/list/SSE
        permissions.ts         List/revoke standing tool permissions
      orchestrator/
        Orchestrator.ts        Run lifecycle + shared generation loop (drive)
        workspaceTools.ts      Tool schemas + executeWorkspaceTool (fs access)
        systemPrompt.ts        buildSystemPrompt (mode-aware)
        eventBus.ts            EventEmitter used for SSE fan-out
      providers/
        ModelProvider.ts       complete() interface
        OpenAICompatibleProvider.ts
        AnthropicProvider.ts
        ProviderFactory.ts     type -> adapter
        ProviderRegistry.ts    loads providers.local.json, provider configs, presets
      database/
        db.ts                  Connection + schema + migrations + startup cleanup
        repositories.ts        Barrel: re-exports every repository + interface
        repositories/          One file per repository (DI: db injected via ctor)
          interfaces.ts        I*Repository contracts + *Input types
          RunRepository.ts     Run rows
          MessageRepository.ts Message rows
          PlanRepository.ts    Plan rows
          ProjectRepository.ts Project rows
          PermissionRepository.ts  Standing permission grants
          MemoryRepository.ts  Global/project memory rows
          UsageLogRepository.ts    Token/cost usage logs (paginated)

  web/                         Vue frontend
    src/
      App.vue                  Thin shell: wires useAppShell to components
      main.ts
      api/client.ts            Typed fetch wrappers for every endpoint
      composables/
        useAppShell.ts         Composes the other composables + lifecycle
        useChatSession.ts      Runs/messages/SSE/send/continue/cancel/permission
        useProjects.ts         Project list + active project + add/remove flow
        usePermissions.ts      Standing-permissions list + settings modal
        useComposerSettings.ts Mode / model / preset settings (localStorage)
        useChatAutoScroll.ts   Scroll-to-bottom watchers
      lib/                     Pure helpers (no state)
        markdown.ts            Marked setup + renderMarkdown + cleanMessageContent
        format.ts              Time/status/json formatting, splitCombined, constants
        messageGroups.ts       Groups flat messages into renderable blocks
        confirmation.ts        Yes/no detection + permission path helpers
      components/
        AppSidebar.vue         Projects accordion + chat history
        MessageThread.vue      Message list (user / assistant / tool groups)
        ToolGroup.vue          Collapsible tool-call/response block
        CoderGroup.vue         Collapsible coder/utility sub-agent windows
        ChatComposer.vue       Input + mode menu + model menu + cards
        ConfirmationCard.vue   Inline yes/no quick reply
        PermissionCard.vue     Inline tool permission request (diff + options)
        AddProjectModal.vue
        PlanPanel.vue          Right-hand stable plan panel
        settings/              Provider, permission, and agent preset settings

packages/
  shared/src/index.ts          Shared TS contracts (Run, RunMessage, events, ...)

config/providers.json          Shared provider/model/pricing catalog (committed; NO secrets)
providers.local.json           Legacy local config (git-ignored; superseded by config/providers.json)
locagens.db                    Local SQLite file (git-ignored)
```

---

## Core Principles

Keep the architecture simple. No heavy agent frameworks.

Prefer readable code over clever abstractions. Keep functions small and
purpose-driven. Avoid premature abstraction.

Backend layering: `routes` (HTTP) -> `orchestrator` / `repositories` /
`registry` (logic) -> `database` / `providers` (I/O). Routes never contain
provider-specific or SQL-heavy logic directly.

Frontend layering: `App.vue` (view) -> `useAppShell` (coordination) ->
domain composables -> `api/` + `lib/` (pure infrastructure). Components are
presentational and communicate via props/emits.

---

## Provider Configuration

Provider settings live in a **committed, version-controlled catalog** so every
clone shares the same providers, models, and pricing without re-entering them:

```txt
config/providers.json   (committed; providers + models + modelSettings + agentPresets — NO secrets)
```

`LOCAGENS_PROVIDER_CONFIG_PATH` can override this path (used by tests). API keys
are **never written to the JSON**: on macOS the key is stored in the Keychain and
the file keeps only a non-secret `apiKeyRef` pointer (`macos-keychain:<id>`),
which is deterministic from the provider id and therefore portable across
machines (each machine holds its own Keychain entry). Where secure storage is
unavailable (e.g. Windows — handled later) the key is simply not persisted.

**Two layers (predefined base + user overlay).** When
`LOCAGENS_PROVIDER_USER_CONFIG_PATH` is set, `ProviderRegistry` loads a read-only
predefined catalog (the committed/bundled `config/providers.json`) and merges a
writable **user overlay** on top (by provider/preset id). The overlay holds the
user's custom providers, their edits to predefined entries, and removal tombstones
(`removedProviders`/`removedPresets`); saves write ONLY the overlay, never the base.
This means an app update can refresh the predefined catalog while the user's own
providers survive. In the packaged desktop app the base is the bundled resource
and the overlay lives at `<userData>/providers.user.json`. With no overlay env set
(dev + tests) the registry uses the single base file and writes back to it.

So a fresh clone gets the full catalog from `config/providers.json`; each
developer only enters their own API keys once (saved to the Keychain, not the
file). Public provider metadata must never include secrets.

`GET /api/providers` returns only safe metadata:

```txt
id, displayName, type, models
```

Never expose `apiKey`, authorization headers, or raw secrets through public
provider metadata.

The local settings API returns editable config, but `GET /api/providers/config`
must not return raw API keys. It uses a preserve marker so the frontend can edit
other provider fields without receiving or re-posting the existing secret.

`config/providers.json` may also define an `agentPresets` block: named pairings
with an **architect** model, a **coder** model, `maxSubAgents`, and optionally a
**utility** model. `GET /api/agent-presets` returns this as safe metadata (only
provider ids + model names, already public). `PUT /api/agent-presets` saves the
full set back via `ProviderRegistry.saveAgentPresets`. See "Agent Presets" below.

Each provider's `modelSettings[model]` block (edited per model row in the
Providers settings tab) may carry, besides `reasoning`, an optional
`contextLimit` (max context window in tokens) and `pricing`. `pricing` is a list
of `tiers` — `{ upToInputTokens?, inputRate, outputRate, cacheReadRate?,
cacheWriteRate? }` (USD per 1M tokens). Flat-priced models use one tier; tiered
models (e.g. a cheaper rate ≤250k prompt tokens, pricier above) list several and
the tier is chosen by the request's total prompt token count.
`ProviderRegistry.resolvePricing` feeds this to `calculateCost` (pricing.ts).
There is **no built-in pricing table and no name-based guessing**: a model with no
configured pricing simply costs 0. Cost is computed locally from these rates and
token usage (providers return token counts, never a dollar amount); it is stored
per call in `usage_logs` at run time and not recomputed for past rows.

---

## Provider Adapter Rules

Use the adapter pattern. The orchestrator must not know provider-specific
request formats — it only calls `complete(request)`.

```ts
export interface ModelProvider {
  complete(request: CompletionRequest): Promise<CompletionResponse>;
}
```

Adapters:

```txt
OpenAICompatibleProvider   /chat/completions, OpenAI tools/tool_calls shape
AnthropicProvider          /v1/messages, tool_use/tool_result blocks
```

OpenAI-compatible providers (OpenAI, OpenCode, CommandCode, etc.) share
`OpenAICompatibleProvider`. Anthropic uses its own adapter because the
Messages API differs.

Both adapters support workspace tools. The orchestrator always passes the
OpenAI-shaped `WORKSPACE_TOOLS`; each adapter maps to/from its own wire format
so the orchestrator only ever sees the common `ToolCall` shape. The Anthropic
adapter converts tool definitions to `input_schema`, assistant tool calls to
`tool_use` blocks, and tool results to `tool_result` blocks on a user turn.

**Reasoning conversion:** the UI only ever offers plain effort levels
(low/medium/high…). Each model names/shapes its reasoning control differently, so
`OpenAICompatibleProvider` converts the chosen effort into that model's official
parameter via `reasoningWire.ts` (`openAiReasoningParams`) — e.g.
`thinking_config:{mode:"on"}` (qwen-max), `thinking_preference:"enabled"`
(deepseek-v4), `reasoning_mode:"high_effort"` (kimi-k2.6),
`thinking_control:{enable:true}` (minimax-m3), `thinking_effort` (glm-5.1),
`reasoning_scope` (mimo-v2.5-pro). Unlisted models fall back to plain
`reasoning_effort`. Anthropic-typed endpoints keep `thinking:{budget_tokens}`.

---

## Orchestrator Rules

The orchestrator (`Orchestrator.ts`) owns one run driver and a shared
provider-facing generation loop.

It should:

```txt
load the run
build a mode-aware system prompt (systemPrompt.ts)
choose tools from mode + preset configuration
call the model with those tools available
if the model returns tool calls:
  gate each call by mode/permissions, execute it, feed the result back
otherwise:
  save the final assistant message and finish
stream every state change and message over SSE
```

`run()` starts a fresh run; `continueRun()` replays persisted history and
continues the same thread. Both share one private `drive()` method, which uses
`runAgentLoop()` for the main model and for delegated coder/utility sub-agents.
Do not re-introduce duplicated tool-execution code across those paths.

The run terminates when the main model responds with no tool calls, on
cancellation, or on error. Cancellation is cooperative (`checkCancelled`) and
also resolves any pending permission request so the loop can unwind.

---

## Operational Modes

The run's `mode` shapes the system prompt and tool gating:

```txt
chat             Lightweight conversation; no proactive workspace work.
plan             Discuss/plan only; do not write files.
accept_edits     May call tools; edits applied directly.
ask_permissions  Each tool call requires explicit user approval first.
auto             Autonomous build mode; still gates dangerous tools.
```

The web UI exposes three modes: **Build** (the `accept_edits` backend mode —
applies edits directly), **Plan**, and **Full Access** (autonomous). `chat`,
`ask_permissions` / `auto` remain valid backend modes (e.g. for older persisted
runs and the permission flow) but are no longer offered in the mode picker.

The picker defaults to whichever mode the user last SENT a message with
(persisted as `bm_last_used_current_mode` in `handleSendTask`); on a fresh start
with no prior message it defaults to **Build**.

**Chat** is a retired UI mode (kept as a backend mode for old runs): a
lightweight conversational mode where `buildSystemPrompt` returns a short prompt
with no tool catalog or planning scaffolding, and the model is told not to
proactively scan/read/modify the workspace. Build/Plan keep the full prompt.

In **plan mode** the model records a structured, chat-specific plan via the
`update_plan` tool (see Workspace Tools / Plan Panel below). That plan is a
*stable document* shown in the right-hand panel — it is created once and only
changes on an explicit revision request. Live progress is tracked separately by
the text `<task_list>` block the model re-emits in each message (all modes),
which the web UI pins above the composer. So the panel = the fixed plan; the
pinned bar = the live, per-message checklist. They are independent.

`run_command` and `fetch_url` always pause for approval unless a matching
standing grant exists. In `ask_permissions` mode every tool call is gated the
same way. The approval flow emits `permission_requested`, sets status
`awaiting_permission`, and waits for a decision: `allow_once`, `allow_project`,
`allow_always`, `allow_run`, or `deny`. `allow_project` / `allow_always` are
persisted in the `permissions` table. `allow_run` is the user's mid-run escape
from prompt fatigue: it approves the current call AND silences every later prompt
for the rest of that run. It is run-scoped and in-memory (a `bypassRuns` set in
`PermissionCoordinator`, cleared when the run finishes) — not persisted.

The `permission_requested` event carries a `PermissionPreview` (built by
`buildPermissionPreview`): the tool, action
(create/edit/delete/read/list/mkdir/move/search/command/fetch), path, and any
available old/new content so the UI can render a preview before the tool runs.

Standing `allow_project` / `allow_always` grants are silent until revoked.
The Settings modal (sidebar → Settings) lists them and can revoke individual
rules or clear all, via `GET /api/permissions` and
`DELETE /api/permissions[/:id]`.

---

## Workspace Tools

Tools are defined and executed in `workspaceTools.ts`:

```txt
write_file(path, content)
edit_file(path, old_string, new_string, replace_all?)
delete_file(path)
read_file(path, offset?, limit?)   large files paged in windows of ≤2000 lines
list_directory(path)
create_directory(path)
move_file(source_path, destination_path)
search_files(query, path?)
run_command(command)
fetch_url(url)
  set_chat_title(title)
  ask_user_question(questions)                    main agent, every mode
  remember(scope, category, content, update_id?)  main agent, every mode
  load_skill(name)                                main agent, every mode
  update_plan(title, tasks, body?, start_new?)   plan mode only
  delegate_tasks(tasks, parallel?)               preset build modes only
  delegate_to_utility(tasks, parallel?)          preset build modes only
  ```

Safety: every path resolves against the run's project directory and must stay
inside it. Access outside the workspace is denied. Tool failures are returned
to the model as JSON `{ success: false, error }` rather than thrown.

`run_command` runs a shell command with the workspace as the working directory
(timeout 120s, output truncated). It is listed in `DANGEROUS_TOOLS` and is
therefore **always** routed through the permission flow before it executes,
regardless of the run's mode — the model can build/compile/test, but only after
the user approves. Standing `allow_project` / `allow_always` grants still apply.

`fetch_url` performs network I/O: it GETs an http(s) URL (timeout 30s, output
truncated) and returns the response body so the model can read online docs or
an API response. It is also in `DANGEROUS_TOOLS`, so it **always** asks before
running. Its standing grants are scoped per host (approving one site does not
approve the whole web). Because it is the only async tool, the orchestrator
executes tools through `executeWorkspaceToolAsync`.

`set_chat_title`, `ask_user_question`, `remember`, `load_skill`, `update_plan`,
`delegate_tasks`, and `delegate_to_utility` are orchestrator tools. They do not
execute through the filesystem helper directly: title and plan update local
run/plan state, `remember` writes a memory row, `load_skill` returns a SKILL.md
body from allowlisted skill roots, `ask_user_question` pauses the run for user
input, and delegation starts nested `runAgentLoop()` calls in the same run.

### Skills (`load_skill`)

Skills are Agent-Skills-compatible folders with a `SKILL.md` (YAML frontmatter
`name` + `description`, then markdown body). Discovered from:

```txt
user:    ~/Library/Application Support/Locagens/skills/<name>/SKILL.md   (macOS)
         ~/.config/locagens/skills/… or %APPDATA%/Locagens/skills/…
project: <workspace>/.locagens/skills/<name>/SKILL.md
```

On each drive(), the main agent gets an AVAILABLE SKILLS catalog (name +
description only). Matching work should call `load_skill(name)` to pull the full
body. Project skills override user skills with the same name. Managed in
**Settings → Skills** (list + Open folder; add skills by dropping SKILL.md on
disk). Sub-agents do not receive skills. Override user root with
`LOCAGENS_SKILLS_PATH`.

Do not add git integration or further network tools without an explicit request.

### Ask the User (`ask_user_question`)

The main agent gets an `ask_user_question(questions)` tool in every mode (coder/
utility sub-agents do not). It lets the model ask 1–4 multiple-choice questions
(`{ question, header, multiSelect, options[{ label, description? }] }`) when it is
blocked on a decision only the user can make. Like the permission flow it performs
no filesystem/network I/O: `Orchestrator.executeAskQuestion` validates/normalizes
the questions, then `requestUserAnswer` flips the run to status `awaiting_input`,
emits a `question_requested` SSE event, and waits. The web client
(`useChatSession.pendingQuestionRequest`, `QuestionCard.vue` above the composer)
renders the questions; submitting posts the chosen labels to
`POST /api/runs/:id/answer` (`{ selections: string[][] }`, aligned to the questions
order), which calls `Orchestrator.resolveQuestion`. The selections are fed back to
the model as the tool result (`{ success, answers[{ question, header, selected }] }`)
and the run resumes (`generating`). Cancellation resolves a pending question with
no selections so the loop unwinds.

### Memory (`remember`)

The agent has a persistent, two-tier memory so it learns the user and the project
over time. The main agent gets a `remember(scope, category, content, update_id?)`
tool in every mode (sub-agents do not). It records DURABLE facts only — user
preferences, recurring feedback about how to work, stable project facts, or
external references — never transient task detail or secrets.

- `scope` is `"global"` (applies to every project) or `"project"` (this codebase
  only; the orchestrator scopes it to the run's `projectPath`).
- `category` is `user` / `feedback` / `project` / `reference`.
- `update_id` revises an existing memory in place instead of duplicating it.

Like `set_chat_title`/`update_plan` it performs no filesystem/network I/O and runs
**silently** (no permission prompt, no SSE): `Orchestrator.executeRemember`
validates the args and writes via `MemoryRepository` (the `memory` table). On every
run start, `drive()` loads the relevant memories (`MemoryRepository.listForContext`
= all global + this project's) and `formatMemoryContext` injects them into the
system prompt under a "REMEMBERED CONTEXT" section, so the model honors them from
the first turn. There is no live SSE event — the user simply sees current memories
when they open Settings.

Memories are managed by the user in **Settings → Memory** (`MemoryTab.vue`,
`useMemories`), backed by `GET/POST/PUT/DELETE /api/memories` (`routes/memory.ts`).
The list is fetched when the settings screen opens (`useAppShell.openSettings`
loads permissions + memories together). The user can add, edit, delete, or clear
memories; global and project memories are shown in separate groups.

### Session Title (`set_chat_title`)

New runs are created titled `"New session…"` (see `routes/runs.ts`). The main
agent is given a `set_chat_title(title)` tool in every mode (coder sub-agents are
not); the system prompt asks it to call this once, early, with a short title in
the user's language. Like `update_plan` it performs no filesystem/network I/O, so
it runs silently (no permission prompt): `Orchestrator.executeSetTitle` updates
the run via `RunRepository.update` and emits a `run_title_changed` SSE event,
which the web client applies to the active run and the sidebar list live.

### Plan Panel (`update_plan`)

In plan mode the orchestrator additionally advertises an `update_plan` tool:

```txt
update_plan(title, tasks[{ text, status }], body?, start_new?)
```

It performs no filesystem/network I/O, so it runs silently (no permission
prompt). The orchestrator persists the plan to the `plans` table via
`PlanRepository` and emits a `plan_updated` SSE event. The plan is meant to be
stable: the model creates it once and only calls `update_plan` again to revise
on request (updating the same plan in place); `start_new: true` supersedes a
finished plan with a new versioned one. Progress is *not* tracked here — that
lives in the per-message `<task_list>` (see Operational Modes).

The web client (`useChatSession.currentPlan`, `PlanPanel.vue`) renders the
active plan in a right-hand side panel. The panel opens automatically once a
plan exists and then stays open across mode switches until the user closes it
(an in-thread "Plan: …" link re-opens it once collapsed). The active plan is
loaded on run select via `GET /api/runs/:id/plan`.

---

## Agent Presets (`delegate_tasks`)

A run can optionally use **two models**: an **architect** (the run's normal
`providerId`/`model`) that plans and coordinates, and a **coder** (the run's
`coderProviderId`/`coderModel`) that does the actual code-writing as 1–3 sub-agents.
This pairing is a named **preset** ("opusplan"-style) defined server-side in
`providers.local.json` under `agentPresets` and selectable, optionally, next to
the model picker in the composer (`useComposerSettings.selectedPresetId`). When no
preset is selected the run is plain single-model and behaves exactly as before.

When (and only when) a coder model is configured, the architect loop is given an
extra `delegate_tasks` tool. The architect is still **advertised** the full
workspace toolset (including `write_file`/`edit_file`/`delete_file`/`run_command`),
but every mutating call is a **trap**: `AgentLoop.runToolCall` rejects it with a
`success:false` error that redirects the model to `delegate_tasks`. So the
architect cannot actually change the project itself — the only working path is to
delegate — but instead of hitting a dead-end (no tool to call) it gets an in-loop
correction. This replaced the earlier "strip the tools entirely" approach, which
left weak models with no way to express intent so they gave up and told the user
to run commands manually. The trap keeps them moving toward delegation. (Tool
set-up lives in `Orchestrator.drive()` via `buildModeBaseTools`; the trap/redirect
lives in `AgentLoop.runToolCall`.)

```txt
delegate_tasks(tasks[{ title, instructions, files? }], parallel?)
```

The architect writes self-contained `instructions` per sub-task (the coder does
NOT see the conversation) and decides concurrency: `parallel: true` only when the
tasks touch disjoint files, otherwise they run sequentially. The orchestrator caps
the list at the preset's `maxSubAgents` (1–3) and runs each sub-task through the
SAME generation loop (`runAgentLoop`) on the coder model, in the same workspace,
tagging those messages `agentRole: "coder"`. Each sub-agent's final text is
returned to the architect as the tool result.

Implementation notes:
- `Orchestrator.drive()` sets up the architect loop (adding `DELEGATE_TASKS_TOOL`
  and architect instructions when a coder is configured); `runAgentLoop` is the
  shared, model-agnostic loop used by both the architect and every coder sub-agent.
- Coder sub-agents get `WORKSPACE_TOOLS` only — never `delegate_tasks` or
  `update_plan` — so delegation depth is hard-capped at 1 (no recursion).
- Sub-agents share the run's `runId`, so cancellation and the permission flow just
  work; because parallel sub-agents share the single pending-permission slot, their
  approval prompts are serialized via `permissionChain` (one prompt at a time).
- Each sub-agent's messages are tagged with `agentName` (the delegated task
  title) in addition to `agentRole: "coder"`. The web UI uses `agentName` to
  render EACH coder sub-agent in its own collapsible window — `groupMessages`
  (`messageGroups.ts`) partitions a contiguous coder block by `agentName`, and
  `CoderGroup.vue` shows the title as the window header. Parallel sub-agents
  interleave by timestamp but still split into separate windows. Presets are
  managed in Settings → Agents (`AgentPresetsTab.vue`).

### Optional Utility Tier (`delegate_to_utility`)

A preset may additionally define an **optional** `utility` endpoint — a cheap,
lightweight model the architect offloads tiny mechanical chores to (locating
files/symbols, summarizing a file, simple renames) so its expensive context stays
lean. It is purely additive: presets without a `utility` block, and single-model
runs, behave exactly as before.

When (and only when) a `utility` model is configured *and* the run can delegate
(coder present, build-type mode), the architect also gets a `delegate_to_utility`
tool:

```txt
delegate_to_utility(tasks[{ title, instructions }], parallel?)
```

It mirrors `delegate_tasks` but runs each task (capped at 3) through `runAgentLoop`
on the run's `utilityProviderId`/`utilityModel`, tagged `agentRole: "utility"` +
`agentName`. Crucially the utility sub-agent gets a **restricted toolset**
(`UTILITY_TOOLS` = read-only `read_file`/`list_directory`/`search_files` **plus
`move_file`** for renames — no write/edit/delete/create/run_command) and the
`buildUtilitySystemPrompt` prompt instructing it to return a SHORT answer. Handled
by `Orchestrator.executeUtilityTasks`. In the web UI utility sub-agents reuse the
coder window machinery (`foldCoderGroups` folds both `coder` and `utility` roles;
`CoderGroup.vue` badges the box "Utility" vs "Coder"). Run fields:
`utility_provider_id` / `utility_model` columns; preset metadata carries an
optional `utility` endpoint.

---

## Run States

Use explicit run states (`RunStatus` in shared):

```ts
export type RunStatus =
  | "created"
  | "generating"
  | "awaiting_permission"
  | "awaiting_input"
  | "done"
  | "failed"
  | "cancelled";
```

`awaiting_input` is the non-terminal state used while an `ask_user_question` call
waits for the user's answer (sibling to `awaiting_permission`).

---

## Persistence

SQLite via `node:sqlite`. Tables (created/migrated in `db.ts`):

```txt
runs         id, title, task, project_path/name, status, provider, model, mode,
             coder_provider_id?, coder_model?, agent_preset?, ...
messages     id, run_id, role, agent_role?, provider/model?, content, raw_response?, ...
projects     path (pk), name, created_at
permissions  scope, project_path, tool, command, status; UNIQUE(scope, project_path, tool, command)
plans        id, run_id, title, body?, tasks (JSON), status ('active'|'completed'), version
memory       id, scope ('global'|'project'), project_path, category, content, created_at, updated_at
usage_logs   id, run_id, agent_role?, provider_id, model, input/output/cache tokens,
             cache_hit_rate, cost, duration_ms?, created_at
```

All agent messages are stored. A run must be reopenable after restart. On
startup, runs stuck in active states are marked `failed`. Provider credentials
stay in JSON, never in the database.

---

## Realtime Rules

Use SSE (`GET /api/runs/:id/events`). The backend fans events out through
`eventBus`. Structured event types (see `RunEvent` in shared):

```txt
run_started
status_changed
message_created
message_updated
model_snapshot_locked
permission_requested
plan_updated
question_requested
run_title_changed
run_completed
run_failed
```

The stream closes on `run_completed`, `run_failed`, or a terminal
`status_changed` (done/failed/cancelled). The frontend updates live.

---

## Error Handling

Handle clearly and save to the run record without losing prior messages:

```txt
missing provider config        invalid/empty model response
invalid API key                invalid model name
provider timeout (idle 300s)   network error
rate limit                     cancelled run
permission denied
```

Failed runs keep their messages; the error is stored in `error_message`.

---

## Security Rules

```txt
API keys NEVER touch any JSON file. config/providers.json is committed and keyless;
  keys live only in the OS keychain (referenced by a non-secret apiKeyRef pointer).
Never return provider secrets from public metadata endpoints.
Only the trusted local settings API should read or save full provider config.
Never log secrets.
Keep the local SQLite file out of git.
Constrain all file access to the active project workspace.
No telemetry or external tracking unless explicitly requested.
```

---

## Coding Style

Use shared TypeScript types for cross-boundary contracts (`packages/shared`).

Prefer specific names:

```txt
ProviderRegistry  Orchestrator  RunRepository  OpenAICompatibleProvider
useChatSession    useProjects   buildSystemPrompt  executeWorkspaceTool
```

Avoid vague names like `helper`, `manager`, `processor`, `data`, `stuff`.

---

## Future Features

Not implemented today; do not build without an explicit request:

```txt
separate reviewer/acceptance loop after coder delegation
direct git integration
manual approval checkpoints beyond the existing permission flow
token cost tracking
multi-user auth / cloud deployment
```
