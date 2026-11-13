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

Run lifecycle rules:

* **One drive per run.** Every turn gets a fresh `RunHandle`; `continue_run`
  returns `Conflict` while a drive of the run is live in this process (queue a
  mid-run message instead), and also while the run shows an active status
  updated in the last 2 minutes without a live handle here (another Locagens
  process may be driving it).
* **Cancel is final and awaited.** `cancel_run` cancels the handle (never
  reset), resolves pending prompts, marks the run `cancelled`, kills the run's
  exec sessions and waits up to 5 s for the drive to unwind before aborting
  its task. A later `continue_run` always runs on a new handle.
* **Panics** in a drive task are caught by a supervisor: the run is marked
  `failed` (`run_failed` is emitted) and its handle is released.
* **Exec sessions** (`exec_command`) are tracked per run: killed when a turn
  is cancelled, fails or panics; kept after a `done` turn so `write_stdin`
  can reach them in the next turn.
* **Permissions.** Every prompt (tool calls of any agent, network approvals)
  queues on one gate and owns the single pending slot. `apply_permission_decision`
  only delivers the answer; the asking side applies `allow_run` and persists
  `allow_project` / `allow_always` exactly once. A network "Allow for Run"
  approves that host for the current turn only, never other tool calls.

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
pub async fn undo_last_turn(state, path, run_id: Option<&str>) -> ApiResult<GitSnapshot>
```

`undo_last_turn` reverts only the paths the turn changed (pre-turn tree vs
the post-turn tree recorded when the turn ended). Paths changed again since
(user edits, later turns) are left alone and listed in `skipped`; reverted
paths are listed in `restored`. HEAD / index are restored only if still what
the turn left. The replaced state is saved as a `<run_id>#undo` snapshot, so
undoing that snapshot redoes the turn.

### `api::providers` — `pi` providers, models & auth

```rust
pub async fn list_providers(state) -> ApiResult<Vec<ProviderMetadata>>
pub async fn get_provider_configs(state) -> ApiResult<Value>            // keys masked
pub async fn save_provider_configs(state, input: &SaveProviderConfigInput) -> ApiResult<()>
pub async fn fetch_models(state, input: &FetchModelsInput) -> ApiResult<Vec<String>>
pub async fn test_provider(state, input: &TestProviderInput) -> ApiResult<TestProviderResult>
pub async fn get_pi_auth(state) -> ApiResult<Vec<Value>>                // ~/.pi/agent/auth.json status
pub async fn set_pi_auth(state, input: &SetPiAuthInput) -> ApiResult<()>

// OAuth login: xai, github-copilot, kimi-coding (device code) and openai
// ("Sign in with ChatGPT"), openrouter (authorization URL).
// Credentials go to ~/.locagens/auth.json only.
pub fn oauth_providers() -> Vec<OAuthProviderInfo>                       // { id, name, loginLabel, flow: "device_code" | "auth_url" }
pub async fn start_oauth_login(state, provider_id: &str) -> ApiResult<OAuthLoginStart>
    // { providerId, flow, userCode?, verificationUri? (https), authUrl?, expiresInSecs, intervalSecs, handle (opaque) }
    // device_code: show userCode + verificationUri. auth_url: open authUrl; a loopback server on 127.0.0.1
    // receives the redirect (userCode/verificationUri are absent, expiresInSecs/intervalSecs are 0).
    // `OAuthLoginStart::url_to_open()` returns whichever URL applies.
pub async fn start_oauth_login_with_domain(state, provider_id: &str, enterprise_domain: Option<&str>)
    -> ApiResult<OAuthLoginStart>                                         // github-copilot on GitHub Enterprise
pub async fn finish_oauth_login(state, start: &OAuthLoginStart) -> ApiResult<()>
    // waits for approval (device) or the loopback redirect (auth_url) and stores the credential;
    // drop the future to cancel (the loopback server stops with it)
pub async fn submit_oauth_redirect(state, start: &OAuthLoginStart, pasted_url: &str) -> ApiResult<()>
    // manual fallback of an auth_url login: the pasted redirect URL (openai: strict URL; openrouter:
    // URL, `code=...` or bare code). Completes the login; drop a pending finish_oauth_login afterwards
pub fn open_verification_url(url: &str) -> ApiResult<()>                 // https only, OS opener, no shell
```

With an `openai` OAuth credential the provider uses the `openai-responses` API
and the ChatGPT model list; requests carry `store: false` and no
`max_output_tokens`. GitHub Copilot models each carry their own API and are
narrowed to the account's `availableModelIds`; requests go to the host named
in the token's `proxy-ep` with Copilot's editor headers. OpenRouter keys are
stored as non-refreshing oauth credentials. OpenCode's `auth.json` API keys are
a read-only, lowest-precedence fallback.

`get_pi_auth` reports OAuth entries as `type: "oauth"`, `keyPreview: "oauth"`,
`expiresAt` (epoch ms) and `expired`; tokens are never returned. Expired OAuth
tokens are refreshed transparently when a run resolves its credential
(under an `auth.json.lock` directory, compatible with pi). `list_providers`
only returns configured providers (see `docs/CLI.md`).
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
pub async fn login_mcp_server(state, name, project_path, on_url: &(dyn Fn(&str) + Sync)) -> ApiResult<McpServerInfo>
pub async fn logout_mcp_server(state, name, project_path) -> ApiResult<bool>
```

`login_mcp_server` runs the MCP OAuth flow for a remote server (discovery,
dynamic registration, PKCE, 127.0.0.1 loopback callback) and hands the
authorization URL to `on_url` (open it with
`services::mcp::oauth::open_browser`). Tokens are stored in
`~/.locagens/mcp-oauth.json` (mode 0600 on unix). `McpServerInfo` carries
`needs_login` (server answered 401) and `signed_in` (token stored).
`services::mcp::shutdown_sessions()` DELETEs open streamable-HTTP sessions;
call it before the process exits.

#### MCP server configuration

Servers live in `mcp_servers.json` (user) and `<project>/.locagens/mcp.json`
(project) as `{ "mcpServers": { name: entry } }`. `stdio` entries use
`command` / `args` / `env`. Remote entries use `url` and `transport`
(`http`, `streamable-http` or `sse`; all three speak the MCP streamable HTTP
transport: `initialize` handshake, `Mcp-Session-Id` / `MCP-Protocol-Version`
headers, JSON or `text/event-stream` replies, re-initialize on a 404 session
loss, `DELETE` on shutdown; the legacy GET+endpoint SSE transport is not
implemented, matching Codex). Auth options, named like Codex's
`[mcp_servers.<name>]` (camelCase aliases accepted):

```json
{ "mcpServers": { "docs": {
    "transport": "streamable-http",
    "url": "https://mcp.example.com/mcp",
    "bearer_token_env_var": "DOCS_MCP_TOKEN",
    "http_headers": { "X-Team": "core" },
    "env_http_headers": { "X-Api-Key": "DOCS_API_KEY" },
    "oauth_client_id": "optional pre-registered client id",
    "scopes": ["read"]
} } }
```

Without a static `Authorization` header, a stored OAuth token (from
`lcgn mcp login`) is sent as the bearer and refreshed on expiry or a 401.

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

### `api::review` — code review

```rust
pub async fn start_review(state, payload: &StartReviewPayload) -> ApiResult<StartedRun>
pub async fn review_choices(state, project_path: Option<&str>) -> ApiResult<ReviewChoices>
pub fn review_of_message(message: &RunMessage) -> Option<ReviewPayload>
pub fn is_review_run(state, run_id) -> bool
```

`start_review` takes a `ReviewTarget` (`locagens_core::protocol::review`:
`Uncommitted`, `BaseBranch { branch }`, `Commit { sha, title }`,
`Custom { instructions }`), builds the diff with read-only git
(`orchestrator::review`: diff, merge-base, show, status, log, rev-parse,
ls-files, for-each-ref, cat-file only) and starts a `chat`-mode run whose
appended system prompt carries the reviewer rubric, the diff and a
`<review_target>` marker. Invalid targets (not a repo, no changes, unknown
branch or commit) fail fast with `bad_request`. When the run finishes, the
drive loop rewrites the last assistant message: `content` becomes a readable
summary and `rawResponse` a `ReviewPayload` JSON object (`kind` =
`review_output`, `target`, `hint`, `output` with `findings[]`,
`overall_correctness`, ...), read back with `review_of_message`. Output that
is not valid JSON degrades to a findings-less review whose explanation is the
raw text. `review_choices` lists branches/recent commits for target pickers.

### `api::symbols` — code symbol search

```rust
pub async fn search_symbols(state, path, query, kind) -> ApiResult<Vec<CodeSymbol>>
```

Registered projects search directly; unregistered-but-valid directories fall
back to a canonicalized search so the Symbols tab works before registration.

## Run model

Runs are **single-model**: one provider/model drives the whole run. The main
agent can fan work out to sub-agents — `explorer` (read-only) or `worker`
(full workspace tools) — each running its own `AgentLoop` with the run's
model. All sub-agents live in one per-run agent hub (max 4 live agents):
`spawn_agent` & co. manage persistent named agents, and the one-shot
`spawn_agents` batch registers its agents in the same hub, waits for every
first turn and closes them. There are no agent presets, no
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
| `exec_command` | `{ cmd, workdir?, yield_time_ms?, max_output_tokens?, shell?, login?, network_domains?, justification? }` | Runs via the ProcessManager with a secret-stripped env. Finishes → `{ exit_code, output, wall_time_ms }`; otherwise → `{ session_id, status: "running", output }`. |
| `write_stdin` | `{ session_id, chars?, yield_time_ms?, max_output_tokens? }` | Feeds a running exec session and polls its output (`running` / `exited`). |
| `view_image` | `{ path }` | Returns `{ ok, path, mime_type, bytes, width, height }` for a workspace image. |
| `update_plan` | Codex `{ explanation?, plan: [{step, status}] }` **or** Locagens `{ title, body?, tasks }` | Normalized into the same `db::Plan` + `plan_updated` event. |
| `request_user_input` | Codex `{ questions: [{ id?, header?, question, options? }] }` | Routed through the QuestionCard flow (`question_requested`). |
| `compact_context` | `{}` | Summarizes older history into a `[Compacted context summary]` block. |
| `spawn_agent` | `{ task_name, message, agent_type? }` (`name` / `task` / `role` aliases accepted) | Starts a persistent background sub-agent (Codex multi-agent v2). Returns `{ agent_id, task_name, nickname, role }`. `agent_type` is a role: `explorer` (read-only, default), `worker`, or a custom role from `agents/*.toml` (see below). |
| `send_message` | `{ target, message }` | Queues a message on the agent; does **not** start a turn. `target` is an agent id, task name or nickname. |
| `followup_task` | `{ target, message }` | Starts a new turn on an idle agent; for a running agent the task is folded in at its next round boundary. |
| `wait_agent` | `{ timeout_ms? }` (default 30 s, min 10 s, max 1 h) | Waits for a mailbox update from any agent. Returns `{ message, timed_out, updates: [{ agent_id, task_name, status, output }] }` (Codex returns only a summary; Locagens includes the content so it persists in the tool message). Returns at once when nothing is running. |
| `interrupt_agent` | `{ target }` | Ends the agent's current turn; the agent stays alive. Returns `{ previous_status }`. |
| `close_agent` | `{ target }` | Shuts the agent down, frees its slot. Max 4 live agents; completed agents hold a slot until closed. |
| `list_agents` | `{ path_prefix? }` | Live agents: `{ agent_id, task_name, nickname, role, status, unread_update }`. |
| `tool_search` | `{ query, limit? }` (default 8) | Present only when more than 8 MCP tools are connected: all MCP tools are then deferred, and each search (BM25 over name, description and argument names) appends the best matches to the tools of the next model request. |
| `get_context_remaining` | `{}` | `{ tokens_left, context_window, estimated_tokens_used, percent_remaining, tokens_until_auto_compact }`; `tokens_left` is the smaller of window-left and auto-compact-left, estimated with the compactor's ~4 chars/token. |
| `host_executable` | `{ name, all? }` | Locates an executable on `PATH` (Windows: `PATHEXT`). `{ name, lookup_key, found, path, paths, total_matches }`. |

Custom agent roles are TOML files (`name`, `description`, `developer_instructions`,
`nickname_candidates`, `sandbox_mode = "read-only"`) in
`<project>/.locagens/agents/` (highest precedence) and `<agent dir>/agents/`.

Approval/sandbox policy (Codex-compatible):

```txt
chat / plan            → on-request + read-only
accept_edits (Build)   → on-request + workspace-write
                         (known-safe read-only commands run without prompting)
ask_permissions        → untrusted + workspace-write (everything prompts)
auto (Full Access)     → never + danger-full-access
```

Commands requesting `network_domains` never use the known-safe bypass: the
domains are granted only by a prompt or a matching standing grant. On
Windows, sandboxed commands run under a restricted token with a per-domain
network proxy; unknown hosts raise a `network_access` permission request
while the command's tool call is running. See `docs/SANDBOX.md`.

Events added: `turn_diff_updated` (`{ diff, filesChanged }`),
`context_compacted`, `agent_spawned` (`{ agentId, name, taskName, role }`),
`patch_progress`.

`patch_progress` is emitted (throttled to ~60 ms) while the model is still
streaming an `apply_patch` call, so a UI can show the file list before the
call completes:

```json
{ "type": "patch_progress", "runId": "…", "messageId": "msg-…", "callId": "call_…",
  "agentName": null, "complete": false,
  "files": [ { "path": "src/a.rs", "op": "update", "added": 3, "removed": 1 } ] }
```

`op` is `add | update | delete` (`move_to` appears for renames); `added`/`removed` count the `+`/`-` lines
seen so far; `complete` becomes true at `*** End Patch`. The event is
advisory — the final `apply_patch` tool result and `turn_diff_updated` remain
authoritative. `permission_requested` additionally carries `agentRole` /
`agentName` when a sub-agent asked.

Auth & provider config: primary directory `~/.locagens` (`$LOCAGENS_HOME`);
fallback reads from `~/.codex` (`$CODEX_HOME`: `auth.json` `OPENAI_API_KEY`,
`config.toml` `[model_providers.*]`) and `~/.pi/agent`.
