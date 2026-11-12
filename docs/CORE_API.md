# `locagens_core::api` — Typed In-Process Core API

`locagens_core::api` is the **typed in-process facade** over the Locagens core
(`crates/core`, crate `locagens-core`; consumed by `apps/cli` and
`apps/desktop-rs`). It exposes every core operation as plain Rust functions
returning typed values — no HTTP involved.

## Who uses it

| Consumer | How |
| --- | --- |
| **Native Rust desktop app** (`apps/desktop-rs`, Dioxus) | calls `api` functions directly on the UI thread / from async tasks |
| **`lcgn` CLI** (`apps/cli`: `src/bin/lcgn.rs`, `src/cli/`) | creates runs, answers questions and streams events through the facade |

## Startup

```rust
pub fn bootstrap() -> anyhow::Result<AppState>
```

Opens the data store (file-based JSON/JSONL under the app-support dir), loads
provider auth/models (`crates/config`: `~/.locagens` + `~/.codex` + legacy
`~/.pi/agent`), runs startup tasks (fail stuck runs, project
sync), and returns the shared `AppState` (`db`, `orchestrator`, `pi`,
`processes`, `default_project_path`). The desktop UI calls this once at
startup — it is what the desktop app and `lcgn` do first.

## Error handling

Every facade function returns `ApiResult<T> = Result<T, ApiError>`.

```rust
pub enum ApiErrorKind { BadRequest, NotFound, Conflict, Internal }  // 400 / 404 / 409 / 500
pub struct ApiError  { pub kind: ApiErrorKind, pub message: String }
pub type ApiResult<T> = Result<T, ApiError>;
```

Constructors: `ApiError::bad_request`, `not_found`, `conflict`, `internal`.
`ApiError` implements `std::error::Error` + `Display`; the kinds map 1:1 onto
the HTTP status codes the former routes used (400/404/409/500), and messages
pass through unchanged.

## Modules

### `api::runs` — run lifecycle, messages, usage, events

```rust
pub async fn create_run(state, payload: &CreateRunPayload)  -> ApiResult<StartedRun>   // POST /api/runs
pub async fn continue_run(state, run_id, payload: &ContinueRunPayload) -> ApiResult<ContinuedRun>
pub async fn cancel_run(state, run_id)                      -> ApiResult<()>
pub async fn queue_message(state, run_id, content)          -> ApiResult<()>           // mid-run inbox (409 when idle)
pub async fn apply_permission_decision(state, run_id, call_id, decision, updated_args) -> ApiResult<()>
pub async fn permission_decision(state, run_id, decision)   -> ApiResult<()>
pub async fn answer_question(state, run_id, answer: &AnswerQuestionPayload) -> ApiResult<()>

pub async fn list_runs(state)      -> ApiResult<Vec<Run>>          // lastActiveAt DESC
pub async fn get_run(state, id)    -> ApiResult<Run>
pub async fn list_messages(state, id) -> ApiResult<Vec<RunMessage>>
pub async fn run_usage(state, id)  -> ApiResult<RunUsageSummary>   // tokens/cost, byRole
pub async fn run_plan(state, id)   -> ApiResult<Option<Plan>>
pub async fn run_pending(state, id) -> ApiResult<PendingRequests>  // pending permission/question
pub async fn usage_logs(state, query: &UsageLogQuery) -> ApiResult<UsageLogsResponse>

pub async fn subscribe_run_events(state, run_id) -> EventRx
```

### Event subscription

```rust
pub type RunEvent  = serde_json::Value;               // { "type": ..., ... } — same payloads as SSE
pub type EventRx   = tokio::sync::broadcast::Receiver<RunEvent>;
pub async fn subscribe_run_events(state: &AppState, run_id: &str) -> EventRx
```

Semantics:

* **terminal run** — replays the terminal event, then closes;
* **live handle** — replays pending permission/question, then forwards every
  event until the sender closes or all receivers drop;
* **no handle / not terminal** — polls until a handle appears, the run turns
  terminal, or the last receiver drops.

Event types: `run_started`, `status_changed`, `message_created`,
`message_updated`, `plan_updated`, `title_changed`, `run_completed`,
`run_failed`, `run_cancelled` (sub-agent messages carry
`agentRole`/`agentName`).

### `api::projects` — projects, files, git

```rust
pub async fn list_projects(state) -> ApiResult<Vec<Project>>
pub async fn create_project(state, input: CreateProjectInput) -> ApiResult<Project>
pub async fn delete_project(state, path) -> ApiResult<()>
pub async fn list_files(state, path, query) -> ApiResult<Vec<String>>
pub async fn git_status(state, path) -> ApiResult<GitStatus>
pub async fn git_diff_details(state, path) -> ApiResult<Vec<GitFileDiff>>
pub async fn git_generate_message(state, run_id) -> ApiResult<String>   // LLM commit message
pub async fn git_commit(state, input: GitCommitInput) -> ApiResult<()>
pub fn require_registered_project(state, raw) -> ApiResult<PathBuf>     // workspace containment
```

### `api::providers` — `pi` providers, models & auth

```rust
pub async fn list_providers(state) -> ApiResult<Vec<ProviderMetadata>>
pub async fn get_provider_configs(state) -> ApiResult<Value>            // keys masked
pub async fn save_provider_configs(state, input: &SaveProviderConfigInput) -> ApiResult<()>
pub async fn fetch_models(state, input: &FetchModelsInput) -> ApiResult<Vec<String>>
pub async fn test_provider(state, input: &TestProviderInput) -> ApiResult<TestProviderResult>
pub async fn get_pi_auth(state) -> ApiResult<Vec<Value>>                // ~/.pi/agent/auth.json status
pub async fn set_pi_auth(state, input: &SetPiAuthInput) -> ApiResult<()>
```

### `api::permissions` — standing grants

```rust
pub async fn list_permissions(state) -> ApiResult<Vec<PermissionRule>>
pub async fn delete_permission(state, id: i64) -> ApiResult<()>
pub async fn clear_permissions(state) -> ApiResult<()>
```

### `api::memory` — durable long-term memory

```rust
pub async fn list_memories(state) -> ApiResult<Vec<Memory>>
pub async fn create_memory(state, input: CreateMemoryInput) -> ApiResult<Memory>   // scope: project|global
pub async fn update_memory(state, id, content) -> ApiResult<Memory>
pub async fn delete_memory(state, id) -> ApiResult<()>
pub async fn clear_memories(state) -> ApiResult<()>
```

### `api::mcp` — MCP servers

```rust
pub async fn list_mcp_servers(state, project_path) -> ApiResult<McpServersResponse>
pub async fn upsert_mcp_server(state, input: McpServerInput) -> ApiResult<McpServerInfo>
pub async fn delete_mcp_server(state, name, project_path) -> ApiResult<bool>
pub async fn restart_mcp_server(state, name, project_path) -> ApiResult<McpServerInfo>
pub async fn toggle_mcp_server(state, name, enabled, project_path) -> ApiResult<McpServerInfo>
```

### `api::plugins` — plugin registry (MCP servers, custom tools, hooks)

```rust
pub async fn list_plugins(state, project_path) -> ApiResult<PluginsResponse>
pub fn plugin_templates() -> ApiResult<Vec<Value>>
pub async fn install_plugin(state, payload: InstallPayload) -> ApiResult<Value>
pub async fn toggle_plugin(state, id, input: ToggleInput) -> ApiResult<Value>
pub async fn delete_plugin(state, id, input: ToggleInput) -> ApiResult<()>
```

### `api::processes` — managed terminal processes

```rust
pub async fn list_processes(state, project_path) -> ApiResult<Vec<ProcessInfo>>
pub async fn spawn_process(state, input: SpawnProcessInput) -> ApiResult<ProcessInfo>
pub async fn kill_process(state, id) -> ApiResult<bool>
pub async fn restart_process(state, id) -> ApiResult<ProcessInfo>
pub async fn process_logs(state, id) -> ApiResult<ProcessLogs>
```

### `api::security` — sandbox status & setup

```rust
pub async fn security_status() -> ApiResult<SecurityStatus>
pub async fn setup_sandbox(confirmed: bool) -> ApiResult<SecurityStatus>
```

### `api::skills` — SKILL.md packs

```rust
pub async fn list_skills(state, project_path) -> ApiResult<SkillsListResponse>
pub async fn get_skill(state, name, project_path) -> ApiResult<SkillSummary>
pub async fn install_skill(state, input: InstallSkillInput) -> ApiResult<SkillSummary>
pub async fn skills_folder(state, input: InstallSkillInput) -> ApiResult<SkillsFolder>
pub async fn delete_skill(state, name, location) -> ApiResult<()>
```

### `api::symbols` — code symbol search

```rust
pub async fn search_symbols(state, path, query, kind) -> ApiResult<Vec<CodeSymbol>>
```

Registered projects search directly; unregistered-but-valid directories fall
back to a canonicalized search so the Symbols tab works before registration.

## Run model

Runs are **single-model**: one provider/model drives the whole run. The main
agent can fan work out to sub-agents with the `spawn_agents` tool —
`explorer` (read-only) or `worker` (full workspace tools) — each running its
own `AgentLoop` with the run's model. There are no agent presets, no
coder/utility tiers and no `delegate_tasks`/`delegate_to_utility` tools.

## Conventions

* All path arguments are workspace-relative or absolute and validated against
  the project root (symlink-aware containment) before touching disk.
* Facade functions are async where they touch the orchestrator/LLM and
  synchronous-safe otherwise; all are callable from any Tokio runtime.
* `AppState` is cheap to clone (internally `Arc`'d) and shared across the
  desktop UI and the CLI.

## Phase 9 — Codex Harness Tools (crates/core orchestrator + crates/* support)

The agent loop exposes the full Codex toolset in addition to the classic
workspace tools:

| Tool | Schema | Behaviour |
| --- | --- | --- |
| `apply_patch` | `{ input: string }` (`patch` alias accepted) | Applies a `*** Begin Patch` … `*** End Patch` patch inside the workspace (add / delete / update / move, fuzzy context matching) and returns Codex's summary + unified diff. All paths validated with `safe_path`. |
| `exec_command` | `{ cmd, workdir?, yield_time_ms?, max_output_tokens?, shell?, login?, justification? }` | Runs via the ProcessManager with a secret-stripped env. Finishes → `{ exit_code, output, wall_time_ms }`; otherwise → `{ session_id, status: "running", output }`. |
| `write_stdin` | `{ session_id, chars?, yield_time_ms?, max_output_tokens? }` | Feeds a running exec session and polls its output (`running` / `exited`). |
| `view_image` | `{ path }` | Returns `{ ok, path, mime_type, bytes, width, height }` for a workspace image. |
| `update_plan` | Codex `{ explanation?, plan: [{step, status}] }` **or** Locagens `{ title, body?, tasks }` | Normalized into the same `db::Plan` + `plan_updated` event. |
| `request_user_input` | Codex `{ questions: [{ id?, header?, question, options? }] }` | Routed through the QuestionCard flow (`question_requested`). |
| `compact_context` | `{}` | Summarizes older history into a `[Compacted context summary]` block. |
| `spawn_agent` / `send_input` / `wait_agent` / `close_agent` / `list_agents` | Codex interactive sub-agents | Background sub-agents with ids, input delivery, completion waiting and termination. |

Approval/sandbox policy (Codex-compatible):

```txt
chat / plan            → on-request + read-only
accept_edits (Build)   → on-request + workspace-write
                         (known-safe read-only commands run without prompting)
ask_permissions        → untrusted + workspace-write (everything prompts)
auto (Full Access)     → never + danger-full-access
```

Events added: `turn_diff_updated` (`{ diff, filesChanged }`),
`context_compacted`, `agent_spawned`.

Auth & provider config: primary directory `~/.locagens` (`$LOCAGENS_HOME`);
fallback reads from `~/.codex` (`$CODEX_HOME`: `auth.json` `OPENAI_API_KEY`,
`config.toml` `[model_providers.*]`) and `~/.pi/agent`.
