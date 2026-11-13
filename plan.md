> **Historical document.** This is the original two-model MVP plan (Electron + Vue +
> Node, SQLite, planner/coder split). It no longer describes Locagens: the product is a
> single-model, pure-Rust agent with no Node/SQLite/HTTP server. Read `README.md`,
> `Claude.md` and `docs/*.md` for the current design; `state.md` for project status.

# Locagens Plan

## 1. Project Summary

Locagens is a local-first AI orchestration tool that connects two AI models in one controlled workflow.

The main purpose is to remove the manual copy-paste bridge between different AI tools. One model acts as the planner, architect, or reviewer. The other model acts as the coder or implementation agent.

The first version should focus on this workflow:

```txt
User Task
  -> Planner Model
  -> Coder Model
  -> Planner Review
  -> Coder Fix
  -> Final Output
```

Locagens should not try to become a full autonomous coding agent in the MVP. The first milestone is a clean two-model orchestration system with a simple Vue interface, provider adapters, local configuration, run history, and live message streaming.

---

## 2. Core Product Idea

The user opens one screen, writes a task, selects two models, and starts a run.

Example:

```txt
Planner:
  Provider: Anthropic
  Model: claude-opus-4.5

Coder:
  Provider: OpenCode
  Model: qwen / glm / deepseek
```

The user should be able to see both models' messages in the same screen.

Locagens owns the message flow. The user should not manually copy messages from one model to another.

---

## 3. MVP Goals

The MVP should provide:

```txt
Vue-based frontend
Backend orchestrator API
Local provider configuration through providers.local.json
Provider adapter abstraction
OpenAI-compatible adapter
Anthropic adapter
OpenAI support
Anthropic support
OpenCode support
CommandCode support
Planner/coder model selection
Run creation
Planner -> Coder -> Planner Review loop
Max round control
SQLite persistence
Run/message history
Live updates through SSE or WebSocket
Final output panel
```

---

## 4. Out of Scope for MVP

Do not implement these in the first version:

```txt
Repository editing
Terminal execution
Automatic patching
Git integration
Cloud deployment
Multi-user auth
Long-term memory
Complex agent framework integration
File upload/context system
Token cost tracking
Agent marketplace/presets
```

These are valid future features, but they should not block the first usable version.

---

## 5. Recommended Tech Stack

```txt
Frontend: Vue 3 + Vite + TypeScript
Backend: Node.js + TypeScript + Fastify
Database: SQLite
Realtime: SSE first, WebSocket later if needed
Package manager: pnpm
Repository style: monorepo
```

SSE is enough for the first version because the backend mainly streams run events to the UI. WebSocket can be added later if the app needs bidirectional controls like pause, resume, manual approval, or live interventions.

---

## 6. Suggested Project Structure

```txt
locagens/
  apps/
    web/
      src/
        components/
        pages/
        stores/
        api/
        styles/

    api/
      src/
        config/
        database/
        providers/
        orchestrator/
        routes/
        prompts/
        shared/

  packages/
    shared/
      src/
        types/
        contracts/

  providers.example.json
  providers.local.json
  README.md
  PLAN.md
  Claude.md
  Agents.md
```

---

## 7. Provider Configuration

Provider credentials should be stored in a local ignored JSON file.

The repository should include:

```txt
providers.example.json
```

The local machine should use:

```txt
providers.local.json
```

`providers.local.json` must be ignored by Git.

Example:

```json
{
  "providers": {
    "openai": {
      "type": "openai-compatible",
      "displayName": "OpenAI",
      "baseUrl": "https://api.openai.com/v1",
      "apiKey": "YOUR_OPENAI_API_KEY",
      "models": [
        "gpt-5.5",
        "gpt-5.5-thinking"
      ]
    },
    "anthropic": {
      "type": "anthropic",
      "displayName": "Anthropic",
      "baseUrl": "https://api.anthropic.com",
      "apiKey": "YOUR_ANTHROPIC_API_KEY",
      "models": [
        "claude-opus-4.5",
        "claude-sonnet-4.5"
      ]
    },
    "opencode": {
      "type": "openai-compatible",
      "displayName": "OpenCode",
      "baseUrl": "http://localhost:4096/v1",
      "apiKey": "YOUR_OPENCODE_API_KEY",
      "models": [
        "qwen",
        "glm",
        "deepseek"
      ]
    },
    "commandcode": {
      "type": "openai-compatible",
      "displayName": "CommandCode",
      "baseUrl": "http://localhost:3000/v1",
      "apiKey": "YOUR_COMMANDCODE_API_KEY",
      "models": [
        "default"
      ]
    }
  }
}
```

The backend can read API keys. The frontend must never receive API keys.

`GET /api/providers` should return only safe metadata:

```txt
provider id
display name
provider type
available models
```

It must not return:

```txt
apiKey
authorization headers
raw provider secrets
```

---

## 8. Provider Adapter Pattern

Different providers use different request/response formats. Locagens should hide those differences behind adapters.

The orchestrator should depend only on this interface:

```ts
export interface ModelProvider {
  complete(request: CompletionRequest): Promise<CompletionResponse>;
}
```

Common request type:

```ts
export type CompletionRequest = {
  model: string;
  systemPrompt?: string;
  messages: ChatMessage[];
  temperature?: number;
  maxTokens?: number;
};
```

Common message type:

```ts
export type ChatMessage = {
  role: "system" | "user" | "assistant";
  content: string;
};
```

Common response type:

```ts
export type CompletionResponse = {
  content: string;
  raw?: unknown;
  usage?: {
    inputTokens?: number;
    outputTokens?: number;
    totalTokens?: number;
  };
};
```

Required MVP adapters:

```txt
OpenAICompatibleProvider
AnthropicProvider
```

OpenAI, OpenCode, and CommandCode should use `OpenAICompatibleProvider` if they expose an OpenAI-compatible `/chat/completions` API.

Anthropic should use a separate adapter because its Messages API uses a different format.

Suggested provider folder:

```txt
apps/api/src/providers/
  ModelProvider.ts
  OpenAICompatibleProvider.ts
  AnthropicProvider.ts
  ProviderFactory.ts
  ProviderRegistry.ts
```

---

## 9. Agent Roles

Locagens has two main agent roles.

## Planner Agent

The Planner Agent is responsible for:

```txt
understanding the task
creating a concrete implementation plan
giving instructions to the coder
reviewing coder output
accepting the result or requesting changes
```

The Planner Agent should not:

```txt
write the full implementation unless explicitly asked
take over the coder role
expand the scope unnecessarily
give vague review comments
```

## Coder Agent

The Coder Agent is responsible for:

```txt
following planner instructions
producing the implementation output
keeping the solution scoped
making reasonable assumptions when needed
returning complete and usable results
```

The Coder Agent should not:

```txt
ignore planner constraints
redesign the whole solution without being asked
debate architecture unnecessarily
ask avoidable follow-up questions
```

---

## 10. Default Workflow

```txt
1. User writes a task.
2. User selects planner provider/model.
3. User selects coder provider/model.
4. User starts the run.
5. Planner creates an implementation plan.
6. Coder implements the plan.
7. Planner reviews the coder output.
8. If accepted, the run ends.
9. If changes are required, coder revises.
10. The loop stops when accepted, cancelled, failed, or maxRounds is reached.
```

Default max rounds:

```txt
3
```

Allowed max rounds:

```txt
1-10
```

---

## 11. Model Selection and Run Locking

Model selection is part of the run setup.

Before a run starts, the user can freely change:

```txt
planner provider
planner model
coder provider
coder model
max rounds
```

After a run starts, the selected planner/coder configuration must be locked for that run.

Each run must store an immutable snapshot of the selected model configuration:

```txt
planner_provider_id
planner_provider_display_name
planner_model
coder_provider_id
coder_provider_display_name
coder_model
max_rounds
```

Changing global provider/model selections must not modify previous runs.

Active runs should not allow direct model switching because it makes the context and output quality hard to track.

The UI should make this clear:

```txt
Run setup: editable
Active run: locked
Finished run: can be duplicated or retried with different models
```

---

## 12. Retry With Different Model

After a run finishes, the user should be able to retry or continue with a different model.

Recommended actions:

```txt
Retry run with different planner
Retry run with different coder
Retry coder from same planner plan
Continue from this message with different model
Duplicate run with new model selection
```

MVP can include only:

```txt
Duplicate run with new model selection
Retry coder with different model
```

The copied/retried run should become a new run with its own model snapshot. The original run must stay unchanged.

This allows workflows like:

```txt
Planner: Claude Opus
Coder: OpenCode Qwen

If the output is weak:
Retry Coder With Different Model
Coder: CommandCode GLM
```

---

## 13. Review Markers

The planner review must include one of these markers:

```txt
FINAL_ACCEPTED
CHANGES_REQUIRED
```

Accepted format:

```txt
FINAL_ACCEPTED

Reason:
<short reason>
```

Change request format:

```txt
CHANGES_REQUIRED

Required fixes:
1. <specific fix>
2. <specific fix>
3. <specific fix>
```

If the marker is missing, the orchestrator should treat it as `CHANGES_REQUIRED` until max rounds is reached.

If max rounds is reached without `FINAL_ACCEPTED`, the latest coder output should be shown as the final candidate.

---

## 14. Run States

Use explicit run states:

```ts
export type RunStatus =
  | "created"
  | "planning"
  | "implementing"
  | "reviewing"
  | "fixing"
  | "done"
  | "failed"
  | "cancelled"
  | "max_rounds_reached";
```

Avoid vague states like:

```txt
processing
working
pending
```

---

## 15. Orchestrator Responsibilities

The orchestrator is the core of Locagens.

It should:

```txt
create a run
store the selected model snapshot
load selected providers
build role-specific prompts
call planner model
save planner message
call coder model with planner output
save coder message
call planner review
save review message
parse review marker
continue or stop according to the result
stream all events to the UI
save final output
handle errors
```

The orchestrator should not contain provider-specific HTTP request logic. That belongs inside provider adapters.

---

## 16. Prompt Design

## Planner System Prompt

```txt
You are the planner and reviewer agent.

Your responsibilities:
- Understand the user's task.
- Create a clear implementation plan.
- Give concrete instructions to the coder model.
- Review the coder model's output.
- Decide whether the output is acceptable.

Rules:
- Do not implement the task yourself unless explicitly asked.
- Do not rewrite the coder's output.
- Give specific, actionable feedback.
- Keep the scope limited to the user's original request.
- If the coder output is acceptable, respond with: FINAL_ACCEPTED.
- If changes are needed, respond with: CHANGES_REQUIRED and list the required fixes.
```

## Coder System Prompt

```txt
You are the coder and implementation agent.

Your responsibilities:
- Follow the planner's instructions.
- Produce concrete implementation output.
- Keep the solution scoped.
- Avoid redesigning the architecture unless the planner explicitly asks for it.
- Do not ignore planner constraints.
- If something is ambiguous, make a reasonable assumption and state it briefly.
```

---

## 17. Database Schema

Use SQLite for MVP.

## runs

```sql
CREATE TABLE runs (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  task TEXT NOT NULL,
  status TEXT NOT NULL,

  planner_provider_id TEXT NOT NULL,
  planner_provider_display_name TEXT NOT NULL,
  planner_model TEXT NOT NULL,

  coder_provider_id TEXT NOT NULL,
  coder_provider_display_name TEXT NOT NULL,
  coder_model TEXT NOT NULL,

  max_rounds INTEGER NOT NULL,
  current_round INTEGER NOT NULL DEFAULT 0,

  source_run_id TEXT,
  retry_type TEXT,

  final_output TEXT,
  error_message TEXT,

  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
```

## messages

```sql
CREATE TABLE messages (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  role TEXT NOT NULL,
  agent_role TEXT,
  provider_id TEXT,
  provider_display_name TEXT,
  model TEXT,
  content TEXT NOT NULL,
  raw_response TEXT,
  created_at TEXT NOT NULL,
  FOREIGN KEY (run_id) REFERENCES runs(id)
);
```

Provider config can stay in JSON for MVP. A provider database table is not required in the first version.

---

## 18. API Endpoints

## Providers

```txt
GET /api/providers
```

Returns safe provider metadata and model lists.

Must not expose API keys.

## Runs

```txt
POST /api/runs
GET /api/runs
GET /api/runs/:id
GET /api/runs/:id/events
POST /api/runs/:id/cancel
POST /api/runs/:id/duplicate
POST /api/runs/:id/retry-coder
```

## POST /api/runs

Creates and starts a new run.

## POST /api/runs/:id/duplicate

Creates a new run from an existing run with selectable planner/coder configuration.

## POST /api/runs/:id/retry-coder

Creates a new run using the same task and planner plan, but with a different coder model.

This can be implemented after the basic run flow works.

---

## 19. Realtime Events

The backend should stream structured events.

```ts
export type RunEvent =
  | {
      type: "run_started";
      runId: string;
    }
  | {
      type: "status_changed";
      status: RunStatus;
    }
  | {
      type: "round_started";
      round: number;
    }
  | {
      type: "message_created";
      message: RunMessage;
    }
  | {
      type: "model_snapshot_locked";
      snapshot: RunModelSnapshot;
    }
  | {
      type: "run_completed";
      finalOutput: string;
    }
  | {
      type: "run_failed";
      errorMessage: string;
    };
```

---

## 20. Frontend UI

The UI should be one main screen.

Suggested layout:

```txt
Left Panel:
- task input
- planner provider select
- planner model select
- coder provider select
- coder model select
- max rounds
- start button
- run status

Center Panel:
- planner messages

Right Panel:
- coder messages

Bottom Panel:
- final output
```

Run history sidebar:

```txt
run title
status
created date
planner model
coder model
retry/duplicate indicator
```

During an active run, model selectors should be disabled.

For completed runs, show actions:

```txt
Duplicate with different models
Retry coder with different model
Copy final output
```

---

## 21. Error Handling

Handle these clearly:

```txt
missing providers.local.json
missing provider
missing model
invalid API key
provider timeout
provider rate limit
empty model response
invalid model name
network error
cancelled run
max rounds reached
unexpected adapter response
```

Errors should be saved into the run record.

The frontend should show the error without losing previous messages.

---

## 22. Security Rules

```txt
Never commit providers.local.json
Never commit real API keys
Never expose API keys to the frontend
Never log secrets
Never commit local SQLite databases
Never add telemetry without explicit approval
```

The frontend should only receive safe provider metadata.

---

## 23. Recommended Development Phases

## Phase 1 — Project Setup

```txt
Create monorepo
Create Vue app
Create backend app
Create shared package
Add pnpm workspace
Add TypeScript configs
Add .gitignore
Add README.md, PLAN.md, Claude.md, Agents.md
```

## Phase 2 — Provider Config and Adapters

```txt
Load providers.local.json
Expose safe providers endpoint
Define ModelProvider interface
Implement OpenAICompatibleProvider
Implement AnthropicProvider
Implement ProviderFactory
Implement ProviderRegistry
Test provider calls from backend
```

## Phase 3 — SQLite Persistence

```txt
Create SQLite database
Create runs table
Create messages table
Add run repository
Add message repository
Save and read runs
Save and read messages
```

## Phase 4 — Orchestrator Loop

```txt
Create run state machine
Lock model snapshot at run start
Call planner
Call coder
Call planner review
Parse FINAL_ACCEPTED / CHANGES_REQUIRED
Enforce maxRounds
Save final output
Handle failures
```

## Phase 5 — Vue Interface

```txt
Create single-screen layout
Add task input
Add planner provider/model selectors
Add coder provider/model selectors
Add max round input
Add start button
Add planner panel
Add coder panel
Add final output panel
Add run history sidebar
Disable model selectors during active run
```

## Phase 6 — Realtime Streaming

```txt
Add SSE endpoint
Stream run events
Update UI live
Show run status changes
Show new messages live
Show final output live
```

## Phase 7 — Retry and Duplicate

```txt
Duplicate run with different models
Retry coder with different model
Store source_run_id
Store retry_type
Keep original run immutable
```

## Phase 8 — Stabilization

```txt
Improve error messages
Add cancellation
Add provider timeouts
Add empty response handling
Add basic tests
Clean up UI states
Improve local setup docs
```

---

## 24. MVP Acceptance Criteria

The MVP is complete when:

```txt
The app starts locally
The backend loads providers.local.json
The UI lists configured providers and models
The user can write a task
The user can select planner and coder models
The user can start a run
The selected model config is locked for the run
Planner output is sent to coder
Coder output is sent back to planner
Planner review marker is parsed
The loop stops correctly
Messages are streamed live
Runs and messages are saved in SQLite
Previous runs can be reopened
Active run model selectors are disabled
Completed runs can be duplicated with different models
API keys are not exposed to the frontend
```

---

## 25. Future Features

After MVP:

```txt
Manual approval checkpoints
File/context upload
Repository workspace
Patch generation
Diff viewer
Terminal command requests
Explicit user approval for terminal execution
Token usage and cost tracking
Prompt template editor
Agent presets
Local model support
Cloud sync
Multi-user mode
```

---

## 26. Design Principle

Locagens should stay focused.

The first version should solve one problem well:

```txt
Stop manually copying messages between two AI models.
```

Everything else should be added after the core two-model orchestration flow works reliably.
