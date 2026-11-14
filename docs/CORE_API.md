# `locagens_core::api` — Typed In-Process Core API

`locagens_core::api` is the **typed in-process facade** over the Locagens core
(`crates/core`, crate `locagens-core`; consumed by `apps/cli` and
`apps/desktop-rs`). It exposes every core operation as plain Rust functions
returning typed values — no HTTP involved.

## Who uses it

| Consumer | How |
| --- | --- |
| **Native Rust desktop app** (`apps/desktop-rs`, Dioxus) | calls `api` functions directly on the UI thread / from async tasks |
| **`lcgn` CLI** (`apps/cli` entry, implementation in `crates/core/src/cli/`) | creates runs, answers questions and streams events through the facade |

## Startup

```rust
pub fn bootstrap() -> anyhow::Result<AppState>
```

Opens the data store (file-based JSON/JSONL under `~/.locagens`), loads
provider auth/models (`crates/config`: `~/.locagens` with `~/.codex` and
`~/.pi/agent` as read fallbacks), runs startup tasks (fail runs whose owner
process is gone, project sync), and returns the shared `AppState`:

```rust
pub struct AppState {
    pub db: Arc<Db>,                          // file-based store
    pub pi: Arc<PiProviderManager>,           // providers, models, auth
    pub orchestrator: Arc<Orchestrator>,      // run handles + drive loop
    pub settings_path: PathBuf,
    pub default_project_path: PathBuf,
    pub processes: Arc<ProcessManager>,
}
```

The desktop UI and `lcgn` both call `bootstrap` once at startup.

## Error handling

Every facade function returns `ApiResult<T> = Result<T, ApiError>`.

```rust
pub enum ApiErrorKind { BadRequest, NotFound, Conflict, Internal }  // 400 / 404 / 409 / 500
pub struct ApiError  { pub kind: ApiErrorKind, pub message: String }
pub type ApiResult<T> = Result<T, ApiError>;
```

Constructors: `ApiError::bad_request`, `not_found`, `conflict`, `internal`.
`ApiError` implements `std::error::Error` + `Display`; messages pass through
unchanged.

## Modules

### `api::runs` — run lifecycle, messages, usage, events

```rust
pub async fn create_run(state, payload: &CreateRunPayload)  -> ApiResult<StartedRun>
pub async fn continue_run(state, run_id, payload: &ContinueRunPayload) -> ApiResult<ContinuedRun>
pub async fn fork_run(state, run_id, up_to_message_id: Option<&str>) -> ApiResult<Run>  // copy of a run, optionally cut at a message
pub async fn cancel_run(state, run_id)                      -> ApiResult<()>
pub async fn delete_run(state, run_id)                      -> ApiResult<()>  // cancels an active run, waits for it, then deletes
pub async fn queue_message(state, run_id, content)          -> ApiResult<()>  // mid-run inbox (409 when idle)

// permission and question answers
pub async fn permission_decision(state, run_id, decision: &str) -> ApiResult<()>  // unknown decision: 400; nothing pending: 409
pub async fn apply_permission_decision(state, handle: &Arc<RunHandle>, decision: &str) -> bool  // false when nothing pending; unknown -> deny
pub async fn answer_question(state, run_id, selections: Vec<Vec<String>>, notes: Vec<String>) -> ApiResult<()>
pub async fn answer_question_core(handle: &Arc<RunHandle>, selections, notes) -> bool

// reads
pub async fn list_runs(state)         -> ApiResult<Vec<Run>>             // lastActiveAt DESC; picks up runs of other processes
pub async fn get_run(state, run_id)   -> ApiResult<Run>
pub async fn list_messages(state, run_id) -> ApiResult<Vec<RunMessage>>
pub async fn run_plan(state, run_id)  -> ApiResult<Option<Plan>>
pub async fn run_pending(state, run_id) -> ApiResult<PendingRequests>    // pending permission/question
pub async fn run_usage(state, run_id) -> ApiResult<RunUsageSummary>      // tokens/cost, byRole
pub fn usage_summary(run_id, entries: &[UsageLogEntry]) -> RunUsageSummary
pub async fn usage_logs(state, query: &UsageLogQuery) -> ApiResult<UsageLogsResponse>

// events
pub async fn subscribe_run_events(state, run_id) -> EventRx
pub async fn resync_run(state, run_id) -> RunResync   // { run, messages, events }: rebuild after a lagged receiver
```

`CreateRunPayload` / `ContinueRunPayload` take an optional `images: [ImageAttachment { mime, data }]`
(`image/*` MIME, base64 data; at most 5 per message, ~5 MB each, else `BadRequest`). The user
`RunMessage` stores them and every later turn replays them; a message may be images only.

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
pub type RunEvent  = serde_json::Value;               // { "type": ..., ... }
pub type EventRx   = tokio::sync::broadcast::Receiver<RunEvent>;
pub async fn subscribe_run_events(state: &AppState, run_id: &str) -> EventRx
```

Semantics:

* **terminal run** — replays the terminal event, then closes;
* **live handle** — replays pending permission/question, then forwards every
  event until the sender closes or all receivers drop;
* **no handle / not terminal** — polls until a handle appears, the run turns
  terminal, or the last receiver drops.

A receiver that falls behind gets `{"type":"lagged"}` (or `RecvError::Lagged`
on a handle receiver). Do not ignore it: call `api::runs::resync_run` and
re-apply the returned `RunResync` (`run`, `messages`, and the replayable
`events`: pending prompts and the terminal event) (`lcgn` does this in `cli/events.rs`).

Event types: `run_started`, `status_changed`, `message_created`,
`message_updated`, `plan_updated`, `title_changed`, `permission_requested`,
`question_requested`, `turn_diff_updated`, `context_compacted`,
`agent_spawned`, `patch_progress`, `run_completed`, `run_failed`,
`run_cancelled`, `lagged` (sub-agent messages carry `agentRole`/`agentName`).

### `api::projects` — projects, files, git

```rust
// projects and files
pub async fn list_projects(state) -> ApiResult<Vec<Project>>
pub async fn create_project(state, input: CreateProjectInput) -> ApiResult<Project>
pub async fn delete_project(state, path) -> ApiResult<()>
pub fn require_registered_project(state, raw) -> ApiResult<PathBuf>     // workspace containment
pub async fn list_files(state, path, query: Option<&str>) -> ApiResult<Vec<String>>
pub fn fuzzy_file_score(path, query) -> Option<i64>                     // ranking used by the @file popup

// git
pub async fn git_status(state, path) -> ApiResult<GitStatus>
pub async fn git_diff_details(state, path) -> ApiResult<Vec<GitFileDiff>>  // per-file changes for the git panel
pub async fn git_diff(state, path, staged: bool) -> ApiResult<String>      // plain `git diff [--cached]`
pub async fn git_diff_worktree(state, path) -> ApiResult<String>          // `git diff HEAD` (staged + unstaged; no untracked)
pub async fn git_stage(state, path, files: &[String]) -> ApiResult<()>
pub async fn git_unstage(state, path, files: &[String]) -> ApiResult<()>
pub async fn git_generate_message(state, run_id) -> ApiResult<String>      // LLM commit message
pub async fn git_commit(state, input: GitCommitInput) -> ApiResult<()>

// turn snapshots and undo
pub async fn create_git_snapshot(state, path, run_id) -> ApiResult<Option<GitSnapshot>>
pub async fn create_git_snapshot_for_path(project_path: &Path, run_id) -> Result<Option<GitSnapshot>, String>
pub async fn record_turn_end_for_path(project_path: &Path, snapshot_id) -> Result<(), String>
pub async fn undo_last_turn(state, path, run_id: Option<&str>) -> ApiResult<GitSnapshot>
pub async fn undo_last_turn_for_path(project_path: &Path, run_id: Option<&str>) -> Result<GitSnapshot, String>

// worktrees
pub async fn list_worktrees(state, path) -> ApiResult<Vec<GitWorktree>>
pub async fn create_worktree(state, path, branch, register: bool) -> ApiResult<GitWorktree>
    // <parent>/.locagens-worktrees/<project>/<branch-slug>; `register` also adds it as a project
pub async fn remove_worktree(state, path, worktree_path, force: bool) -> ApiResult<()>

// project trust
pub struct ProjectTrust { pub trusted: bool, pub ignored_files: Vec<String> }
pub fn is_project_trusted(path) -> bool
pub async fn project_trust(path) -> ApiResult<ProjectTrust>      // ignored_files: project config skipped while untrusted
pub async fn trust_project(path, trusted: bool) -> ApiResult<ProjectTrust>  // writes [projects."<path>"] trust_level
```

The snapshot/undo engine is an internal service; these facade functions are
the stable entry points.

`undo_last_turn` reverts only the paths the turn changed (pre-turn tree vs
the post-turn tree recorded when the turn ended). Paths changed again since
(user edits, later turns) are left alone and listed in `skipped`; reverted
paths are listed in `restored`. HEAD / index are restored only if still what
the turn left. The replaced state is saved as a `<run_id>#undo` snapshot, so
undoing that snapshot redoes the turn.

Untrusted projects load no `<project>/.locagens/` hooks, MCP servers,
plugins, `.rules` or agent roles; home and filesystem roots cannot be
trusted.

### `api::providers` — providers, models & auth

```rust
pub async fn list_providers(state) -> ApiResult<Vec<ProviderMetadata>>
pub async fn get_provider_configs(state) -> ApiResult<HashMap<String, EditableProviderConfig>>  // keys masked
pub async fn save_provider_configs(state, configs: &HashMap<String, SaveProviderConfigInput>) -> ApiResult<()>
pub async fn fetch_models(state, input: &FetchModelsInput) -> ApiResult<FetchModelsResult>
pub async fn test_provider(state, input: &TestProviderInput) -> ApiResult<TestProviderResult>
pub async fn get_pi_auth(state) -> ApiResult<Vec<Value>>                // auth.json status (keys never returned)
pub async fn set_pi_auth(state, input: &SetPiAuthInput) -> ApiResult<()>
pub fn login_providers(state) -> Vec<LoginProvider>   // every connectable provider + auth status (no keys)

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

Secret files (`auth.json`, `models.json`, `device-id`, `mcp-oauth.json`, MCP
config files) are written through `locagens_config::private_file::write_private_atomic`
(same-dir temp file, mode 0600 on unix, fsync, rename). The `auth.json` lock is
kept fresh by a 2 s heartbeat and released only if its owner token still
matches. `keyPreview` of an API key is `…` plus its last 4 characters (keys of
20+ characters; shorter ones show `***`). `!command` keys run with a 30 s
timeout (`cmd /S /C` on Windows). `test_provider` / `fetch_models` use a key
typed in the UI literally (no `!`/`$` expansion) and never send the stored
credential to a `baseUrl` other than the provider's configured one.

### `api::permissions` — standing grants (`permissions.json`)

```rust
pub async fn list_permissions(state) -> ApiResult<Vec<PermissionRule>>
pub async fn delete_permission(state, id: i64) -> ApiResult<()>
pub async fn restore_permission(state, rule: &PermissionRule) -> ApiResult<PermissionRule>  // undo of a revoke
pub async fn clear_permissions(state) -> ApiResult<()>
// Answers prompt `prompt_id` (from `permission_requested.promptId`) only while
// it is still the pending one; a stale answer is a conflict, never applied to
// the next prompt.
pub async fn answer_permission_prompt(state, run_id, prompt_id: u64, decision) -> ApiResult<()>
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
Every `McpServerInfo` returned by this facade is masked: `config.env` and
`config.http_headers` keep their keys but every value is
`__LOCAGENS_PRESERVE_API_KEY__`; `upsert_mcp_server` maps that marker back to
the stored value (a marker with no stored value is dropped). OAuth refreshes
are single-flight per server (in-process mutex plus `mcp-oauth.refresh.lock`),
an unparsable `mcp-oauth.json` is never overwritten, and discovered OAuth
endpoints must be https (http only for loopback) with a matching `issuer`.
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
pub async fn delete_plugin(state, id, scope: Option<&str>, project_path: Option<&str>) -> ApiResult<()>  // scope: "project" or user (default)
```

### `api::processes` — managed terminal processes

```rust
pub async fn list_processes(state, project_path: Option<&str>) -> ApiResult<Vec<ProcessInfo>>
pub async fn spawn_process(state, input: &SpawnProcessInput) -> ApiResult<ProcessInfo>
pub async fn kill_process(state, id) -> ApiResult<bool>
pub async fn restart_process(state, id) -> ApiResult<ProcessInfo>
pub async fn process_logs(state, id) -> ApiResult<ProcessLogs>
```

### `api::terminal` — interactive shells (desktop Terminal tabs)

The user's own shell (`pwsh.exe` if on PATH, else `powershell.exe`; `$SHELL`
or `/bin/sh` on Unix) on a pty (`portable-pty`, ConPTY on Windows), parsed by
`vt100` (5000 lines of scrollback). cwd is the project folder, the user's
environment is kept, nothing is permission-gated (it is not an agent tool).
Sessions live in a process-wide registry (`services::terminal`), survive the
panel being hidden, and are killed by `close` / `close_all` (call on exit).
These functions are synchronous.

```rust
pub fn open(project_dir: &str, cols: u16, rows: u16) -> ApiResult<String>   // session id
pub fn list(project_dir: Option<&str>) -> Vec<TerminalInfo>                // {id, projectDir, title, exited}, oldest first
pub fn write(id, bytes: &[u8]) -> ApiResult<()>                            // keys / paste; returns the view to the live screen
pub fn resize(id, cols: u16, rows: u16) -> ApiResult<()>
pub fn scroll(id, lines: i32)                                              // > 0 = into history
pub fn snapshot(id) -> Option<TerminalScreen>                              // rows of styled TerminalSpans, cursor cell flagged
pub fn version(id) -> Option<u64>                                          // bumps on output, resize, scroll, exit
pub fn subscribe(id) -> Option<tokio::sync::watch::Receiver<u64>>          // `changed().await` instead of polling
pub fn close(id) -> bool
pub fn close_all()
```

The service answers the terminal queries shells send (DSR cursor position —
ConPTY waits for it at startup — and DA1) and keeps the OSC window title for
the tab label.

### `api::security` — sandbox status & setup

```rust
pub async fn security_status() -> ApiResult<SecurityStatus>   // sandbox backend and availability
pub async fn setup_sandbox(confirmed: bool) -> ApiResult<SecurityStatus>
```

### `api::skills` — SKILL.md packs

```rust
pub async fn list_skills(state, project_path) -> ApiResult<SkillsListResponse>
pub async fn get_skill(state, name, project_path) -> ApiResult<SkillSummary>
pub async fn install_skill(state, input: InstallSkillInput) -> ApiResult<SkillSummary>
pub async fn skills_folder(state, input: InstallSkillInput) -> ApiResult<SkillsFolder>
pub async fn delete_skill(state, name, target: Option<&str>, project_path: Option<&str>) -> ApiResult<bool>  // target: "project" | "user"
pub async fn skill_source(state, name, project_path) -> ApiResult<String>   // raw SKILL.md (project wins)
```

### `api::queue` — queued mid-run messages

```rust
pub async fn queued_messages(state, run_id) -> ApiResult<Vec<String>>           // oldest first
pub async fn withdraw_queued(state, run_id, content) -> ApiResult<bool>         // any position
pub async fn withdraw_queued_message(handle: &RunHandle, content) -> bool       // newest only
```

At the end of a drive the core emits `queued_messages_undelivered { messages }`
for queued text the turn never delivered; frontends put it back for resending.

### `api::ui_state` — last model, effort and mode (`ui_state.json`)

```rust
pub fn load_ui_state() -> UiState                       // { model, effort, mode, recentModels }
pub fn remember_model(provider_id, model, effort: Option<&str>) -> ApiResult<()>
pub fn remember_effort(effort) -> ApiResult<()>
pub fn remember_mode(mode) -> ApiResult<()>              // unknown modes are rejected
```

Shared by desktop and `lcgn`; the file format is documented in
`services/ui_state.rs`.

### `api::review` — code review

```rust
pub async fn start_review(state, payload: &StartReviewPayload) -> ApiResult<StartedRun>
pub async fn review_choices(state, project_path: Option<&str>) -> ApiResult<ReviewChoices>
pub fn review_of_message(message: &RunMessage) -> Option<ReviewPayload>
pub fn is_review_run(state, run_id) -> bool
```

`start_review` takes a `StartReviewPayload` (`target`, `project_path`,
`provider_id`, `model`, `reasoning_effort`) whose `target` is a `ReviewTarget`
(`locagens_protocol::review`: `Uncommitted`, `BaseBranch { branch }`,
`Commit { sha, title }`, `Custom { instructions }`). It builds the diff with read-only git
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
pub async fn search_symbols(state, path, query: Option<&str>, kind: Option<&str>) -> ApiResult<Vec<CodeSymbol>>
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

## Codex harness tools

The agent loop exposes the Codex toolset in addition to the classic
workspace tools. The per-mode tool lists are in `README.md`; `load_skill`,
`list_mcp_resources`, `read_mcp_resource`, `remember`, `ask_user_question`,
`set_chat_title` and `spawn_agents` are not tabulated below.

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
| `web_search` | `{ query, max_results? }` (1..10, default 5) | Local web search for every provider: a numbered list of title, URL and snippet plus the backend that answered; pages are then read with `fetch_url`. Backends, first with a key wins: Tavily (`tavily` / `TAVILY_API_KEY`), Brave Search (`brave-search` / `BRAVE_API_KEY`), Exa (`exa` / `EXA_API_KEY`); keys resolve like provider keys (`auth.json` literal, `!command`, `$ENV_VAR`). Without a key it falls back to DuckDuckGo's HTML page (keyless, weaker results). Only those fixed hosts are contacted. Offered in Plan and the edit modes (not Chat) and to `worker` sub-agents; prompts like `fetch_url` (grant label "searching the web"). |

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

`create_run` / `continue_run` reject unknown modes with `BadRequest`;
`apply_permission_decision` sends any unknown decision as `deny`.
Outside `auto`, dangerous commands and mutations of protected paths
(`.git`, `.locagens`, `.codex`, `.agents`) always prompt and their approval
is downgraded to `allow_once`; `fetch_url` and `web_search` prompt in every
other mode.
`allow_run` covers only the approved tool.

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
