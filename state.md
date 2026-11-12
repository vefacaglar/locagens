# Locagens — Migration State (`state.md`)

Authoritative live handoff and progress tracker for the **Locagens -> Rust + `pi`** migration.
Update this file continuously as phases complete.

---

## 1. Migration Goal & Architectural Decisions

- Replace the legacy Node.js/Fastify backend (`apps/api`) and SQLite sidecar (`apps/db-writer`)
  with a single unified **Rust (`Axum` + `Tokio` + file-based JSON/JSONL storage, Codex/Claude/Pi-style)**
  binary (`apps/backend`, binary `locagens-backend`). Zero SQLite, zero C library dependency
  (`rusqlite` removed from `Cargo.toml` in Phase 2).
- Replace custom provider/API-key storage (`providers.json`, `MacOSKeychainProviderSecretStore`)
  and custom `AgentLoop`/`ModelProvider` adapters with **`pi`**
  (`pi --mode rpc`, `pi auth` / `~/.pi/agent/auth.json`, `~/.pi/agent/models.json`, `pi --list-models`).
- Keep the existing **Vue 3 (`apps/web`)** UI and **Electron (`apps/desktop`)** shell, preserving
  Build/Plan/Full Access modes, Permission approval cards, Architect/Coder sub-agent windows,
  and Plan panel while adapting Provider/Settings UI to `pi`.
- **Final architecture (after Phases 7b + 8)**: a pure Rust workspace with two members —
  `apps/backend` (library `locagens_core` + CLI binary `lcgn`) and `apps/desktop-rs`
  (native Dioxus 0.7 desktop binary `locagens`). The legacy Vue web app, the Electron
  shell, the shared TypeScript contracts and the `locagens-backend` HTTP server are all
  removed; the desktop UI and `lcgn` call `locagens_core::api` in-process.

---

## 2. Mandatory Agent & Development Rules

- **Codebase exploration (read-only research & analysis)**: always use
  `gemini-4-hydrogen` (`Gemini 4 Hydrogen`), default effort `medium` (`low` for
  trivial lookups, `high`/`xhigh` for deep analysis), run as a read-only
  subagent or swarm worker pinned to a `gemini-4-hydrogen-*` model. Never burn a
  coding model on exploration.
- **All coding & file modifications**: Always use `pi` CLI with `opencode-go/glm-5.3-flash`:

  ```powershell
  $null | pi --model opencode-go/glm-5.3-flash --no-mcp --no-extensions --approve -p @path/to/prompt.md "Execute instructions"
  ```

- **Git discipline**: Commit and push to `origin/main` after each completed phase,
  and update `state.md` continuously.

---

## 3. Progress Log & Phase Checklist

- [x] **Phase 0: Workflow & Agent Rules Setup** (Committed & pushed in `c63ed45`)
  - Updated `Claude.md`, `Agents.md`, created `GEMINI.md` and `.agents/rules/pi-gemini-workflow.md`.
  - Removed all 11 legacy `.claude/agents/*.md` files.

- [x] **Phase 1: Rust Backend Foundation (`apps/backend`) + Core Routes** (Committed in `fed1c37`)
  - `apps/backend` with `Cargo.toml`, `src/{main,db,paths,util,auth}.rs`,
    `src/services/{mod,processes,skills,plugins,mcp,symbols}.rs` and
    `src/routes/{mod,settings,projects,memory,permissions,skills,processes,symbols,plugins,mcp,security}.rs`.

- [x] **Phase 2: File-Based JSON/JSONL Storage (Codex/Claude/Pi-style) + Pure Rust `pi` Provider/Auth/Model Engine**
  - **Part A — storage (`src/db.rs`)**: removed `rusqlite` from `Cargo.toml`; zero SQLite,
    zero C library dependency, zero `locagens.db`. `Db` keeps the same public method
    signatures but stores everything under the data root (`LOCAGENS_DATA_DIR` →
    `LOCAGENS_DB_PATH` parent/stem → `<app_support_dir>/data`):
    - `projects.json`, `permissions.json` (dedup on `(scope, projectPath, tool, command, networkDomains)`),
      `memory.json` (auto-increment integer ids).
    - `sessions/<run_id>/run.json` (full Run metadata incl. preset/coder/utility fields),
      `sessions/<run_id>/messages.jsonl` (append-only `RunMessage` JSONL, clean rewrite on
      `update_message`), `sessions/<run_id>/plan.json`, `usage_logs.jsonl`.
    - In-memory `RwLock<StorageState>` (O(1) reads) with atomic `.tmp`-then-rename JSON
      writes and append/flush JSONL writes; startup loads all files and fails stuck runs
      (`created`/`generating`/`awaiting_permission`/`awaiting_input`) with
      "Session interrupted due to server restart." plus a system message line.
    - 4 `db.rs` unit tests (persistence across reopen, permission dedup/ids, stuck-run
      restart handling, message update + usage logs).
  - **Part B — `pi` engine (`src/pi_providers.rs`, `src/routes/providers.rs`)**: pure Rust,
    no `pi` CLI dependency for providers:
    - Reads/writes `~/.pi/agent/auth.json` (api_key / oauth / `!command`-cached / `$ENV_VAR`
      credentials), `~/.pi/agent/models.json` (custom baseUrl/api/models),
      `~/.pi/agent/models-store.json` (cached catalog), `<app_support>/locagens-providers-meta.json`
      and `<app_support>/locagens-presets.json`.
    - Built-in providers: `opencode-go`, `anthropic`, `openai`, `google`, `openrouter`,
      `deepseek`, `groq`, `xai`, `ollama` with the spec'd models; env fallback chain
      (`OPENCODE_API_KEY`, `ANTHROPIC_API_KEY`, …, `ZAI_API_KEY`).
    - Routes: `GET /api/providers`, `GET|POST /api/providers/config` (keys masked as
      `__LOCAGENS_PRESERVE_API_KEY__`), `POST /api/providers/fetch-models`,
      `POST /api/providers/test` (openai-compatible + Anthropic endpoints),
      `GET|PUT /api/agent-presets`, `GET|POST /api/pi/auth`. Mounted in `routes/mod.rs`,
      `AppState.pi` wired in `main.rs`.
    - 4 `pi_providers.rs` tests (auth roundtrip + masking, `$ENV_VAR` resolution, catalog
      merge + config masking, preset validation).
  - Verified: `cargo test` 13/13 passed, `cargo check` clean, live smoke run of the binary
    (ping/projects/providers/config/agent-presets/pi/auth all 200 with real
    `models-store.json` data).

- [x] **Phase 3: Pure Rust Agent Loop, Streaming LLM Adapter, Workspace Tools & Run/SSE Orchestrator**
  (`apps/backend/src/orchestrator/` + `apps/backend/src/routes/runs.rs` — 100% Rust, no `pi` CLI RPC)
  - **`orchestrator/llm.rs`**: pure Rust streaming client via `reqwest` — OpenAI-compatible
    (`{baseUrl}/chat/completions`, `stream: true`: `delta.content`, `reasoning_content`/`reasoning`,
    `tool_calls`, final usage chunk after `finish_reason`, `stream_options.include_usage` with
    automatic retry when rejected, `x-opencode-session` routing header) and Anthropic Messages
    (`content_block_start/delta`, `text_delta`/`thinking_delta`/`input_json_delta`, `message_delta`),
    non-streaming JSON fallback, `reasoning_effort` mapping (OpenAI) / `thinking.budget_tokens`
    (Anthropic), per-model `baseUrl`/`api` overrides from `models-store.json`.
  - **`orchestrator/tools.rs`**: workspace tools with strict canonicalized containment
    (`read_file`, `write_file`, `edit_file`, `delete_file`, `list_directory`, `create_directory`,
    `move_file`, `search_files`) + permission-gated dangerous tools (`run_command` — 120s timeout,
    secret-stripped env, deny-by-default network; `fetch_url` — SSRF private-IP block),
    `PermissionPreview` diff payloads, tool-def subsets for read-only sub-agents.
  - **`orchestrator/mod.rs`**: per-run `RunHandle` (cancellation token, `broadcast` SSE channel,
    mid-run message inbox, pending permission/question oneshot coordinators, run-level
    `allow_run`) + `AgentLoop` driving model↔tool rounds (max 40) for the main agent and
    delegated sub-agents (`coder`/`utility`/`explorer`/`worker`, depth cap 1, messages persisted
    & streamed with `agentRole`/`agentName`), mode-aware system prompts with memory injection,
    permission gating matrix (auto/accept_edits/ask_permissions/plan/chat + standing grants in
    `permissions.json`), usage logs with cost (provider-reported cost → pi per-million pricing),
    interactive tools (`set_chat_title`, `update_plan`, `remember`, `load_skill`,
    `ask_user_question`).
  - **`routes/runs.rs`**: `POST /api/runs`, `/continue`, `/cancel`, `/messages`, `/permission`,
    `/answer`; `GET /api/runs` (lastActiveAt DESC), `/:id`, `/:id/messages`, `/:id/usage`
    (`RunUsageSummary` with `byRole`), `/:id/plan`, `/:id/pending`, `/:id/events` (SSE: connected,
    pending replay, broadcast stream, terminal close, idle heartbeat), `GET /api/usage-logs`
    (paginated + search/filter).
  - `db.rs`: `patch_run` + `update_message_full` (rawResponse updates); `pi_providers.rs`:
    public `resolve_endpoint`/`model_info`, `ModelInfo: Default`.
  - Verified live end-to-end against `opencode-go`/`glm-5.3-flash`: plain completion, tool
    round-trip (list_directory → 13 files), ask_permissions pause → pending preview →
    allow_once → executed → done, ask_user_question pause → answer → done, spawn_agents
    explorer sub-agent (agentRole/agentName streamed), continue-run, mid-run message 409,
    cancel, usage logs (tokens + cost), SSE terminal replay.
  - `cargo test` 27/27 passed, `cargo check` clean (w64devkit + stable-x86_64-pc-windows-gnu).

- [x] **Phase 4: Frontend (`apps/web`), Desktop (`apps/desktop`) Integration & Legacy Node.js Removal**
  - Update `apps/desktop/src/backend.ts` & `paths.ts` to spawn the Rust `locagens-backend` binary.
  - Update `apps/web` provider settings to work natively with `pi` providers/auth.
  - Remove legacy `apps/api` and `apps/db-writer`, update root `package.json` &
    `pnpm-workspace.yaml`, run full build & tests, commit & push.

- [x] **Phase 5: `lcgn` CLI (pi-like command line on the shared Rust core)** — branch `feat/loc-cli`, implemented in 5e07b88, verified (cargo test 50 unit + 4 integration pass, live lcgn checks), merged into main in 7615bc5
  - `apps/backend` is now a library (`locagens_core`, `src/lib.rs`, `build_core()`, `state.rs`) plus two binaries: `locagens-backend` (unchanged behavior/routes) and `lcgn` (`src/bin/lcgn.rs`, helpers in `src/cli/`: args, auth, models, render, repl, runner).
  - `routes/runs.rs`: create/continue/cancel/permission/answer logic extracted into `*_core` functions shared by the HTTP handlers and the CLI (HTTP statuses/bodies unchanged). `orchestrator/mod.rs`: only adds `--system-prompt`/`--append-system-prompt` support (`Run.system_prompt`/`append_system_prompt`, skipped in JSON when unset).
  - `db.rs`: advisory cross-process lock `<data>/.lock` (fs2) around every mutation of `projects.json`, `permissions.json`, `memory.json`, `usage_logs.jsonl`, re-reading them from disk under the lock; two-`Db` interleave test.
  - Flags: `-p`, `--model`, `--provider`, `--thinking`, `--mode text|json`, `--plan`, `--approve/-a`, `-c`, `-r`, `--session`, `--no-session`, `--no-tools`, `--system-prompt`, `--append-system-prompt`, `--cwd`, `--list-models`; `auth login|logout|check`; `sessions`; `@file` + piped stdin; exit codes 0/1/2; interactive REPL with slash commands (`/model /thinking /plan /build /new /sessions /usage /help /exit`).
  - `docs/CLI.md`, README CLI section; `apps/desktop/package.json` `extraResources` now also bundles `lcgn`/`lcgn.exe` (`build:backend` already builds both bins).
  - Verified: `cargo test` 50 unit tests (33 pre-existing + 17 new) + 4 integration tests (mock OpenAI-compatible server) all pass; live against `opencode-go/glm-5.3-flash`: `--list-models`, `-p ... --no-tools`, tool round trip with `--approve`, denial without `--approve`, `--mode json`, `-c`, `--plan`, bad model → exit 2.
  - Not verified / known limits: the interactive REPL, Ctrl+C cancel and the permission/question prompts were not exercised on a real TTY (no TTY in the test harness). A running `locagens-backend` keeps runs/projects in memory, so sessions created by `lcgn` appear in the desktop app after its next start (no live refresh yet). With an open non-TTY stdin that never closes, `lcgn -p` waits for stdin EOF (same as pi); close it with `$null |` or `< NUL`. Full-screen TUI is out of scope.

- [x] **Phase 6a: `locagens_core::api` in-process facade** — commits `35d3f1c` + `07daa4a`
  - `src/api/` modules (`error`, `runs`, `projects`, `providers`, `permissions`,
    `memory`, `mcp`, `plugins`, `processes`, `security`, `skills`, `symbols`):
    typed Rust functions with the exact semantics of the HTTP routes, returning
    `ApiResult<T>` (`ApiError` + `ApiErrorKind` mapping 1:1 to 400/404/409/500).
  - `api::bootstrap() -> anyhow::Result<AppState>` opens the data store, runs
    startup tasks and returns the shared `AppState` for the desktop UI / `lcgn`.
  - `runs::subscribe_run_events(state, run_id) -> EventRx` (broadcast receiver of
    JSON run events; terminal replay / pending replay / live forward / poll).
  - HTTP routes became thin adapters (`routes/*` call `api::*`); `lcgn` and the
    future desktop app call the facade directly.

- [x] **Phase 6b: Remove model/agent presets feature + `docs/CORE_API.md`**
  - Removed the dual/tri-model preset feature completely:
    - `db.rs`: `Run`/`RunRow` no longer carry `coder*`/`utility*`/`agentPreset`
      fields (no `deny_unknown_fields`, so legacy `run.json` files on disk
      still deserialize cleanly).
    - `pi_providers.rs`: dropped `AgentPreset*` types, `presets_path`,
      `get_presets`/`save_presets`/`default_presets` and both preset tests;
      no `locagens-presets.json` anymore.
    - Removed `GET|PUT /api/agent-presets` (routes + api facade).
    - `api/runs.rs`: `CreateRunPayload`/`ContinueRunPayload` no longer accept
      preset/coder/utility fields.
    - `orchestrator/tools.rs`: removed `delegate_tasks`/`delegate_to_utility`
      tool defs (kept `spawn_agents`); `orchestrator/mod.rs`: removed the
      delegation methods/match arms and preset tool selection — build modes
      always include the workspace mutation tools + `spawn_agents`
      (`explorer` read-only / `worker` full tools, run's own model).
  - Runs are now strictly single-model: one provider/model + `spawn_agents` fan-out.
  - Created `docs/CORE_API.md` documenting `locagens_core::api` (bootstrap,
    `ApiError`/`ApiErrorKind`/`ApiResult`, per-module signatures, event
    subscription, single-model run model).
  - Docs updated (`Claude.md`, `Agents.md`, `README.md`,
    `docs/API_ENDPOINT_MAP.md`, `docs/database-schema.md`): preset references
    removed; noted single-model runs with `spawn_agents` and the native Rust
    desktop UI (Dioxus).
  - Verified: `cargo test` (stable-x86_64-pc-windows-gnu, w64devkit) 48 unit +
    4 integration tests pass, zero warnings.

---

## Next Phases

- [x] **Phase 7a: Native Rust Dioxus desktop foundation (`apps/desktop-rs`)**
  - New workspace member `apps/desktop-rs` (crate `locagens-desktop`, binary
    `locagens`, Dioxus 0.7 + tokio + pulldown-cmark + rfd) wired directly
    in-process to `locagens_core::api` — no HTTP. `api::bootstrap()` runs in
    `main`, the shared `AppState` is stored in a global and every component /
    spawned task reads it via `state_ui::core()`.
  - `src/main.rs`: launches `dioxus::LaunchBuilder::desktop()` with a 1380x860
    resizable `"Locagens"` window; root `App` provides the `UiStore` context.
  - `src/theme.rs`: `build_stylesheet(dark)` + `theme_class(dark)` generate a
    complete stylesheet per runtime OS — Windows 11 Fluent (Segoe UI Variable,
    Mica-like `#191919/#202020/#272727` layers, `#0078d4/#60cdff` accent),
    macOS vibrancy (SF Pro, `#1c1c1e/#242426/#2c2c2e`, `#0a84ff`), neutral
    Linux fallback — including the 3-column shell, chat thread, tool groups,
    reasoning panels, diff views, permission/question cards, composer, modals,
    settings/usage tables and badges.
  - `src/markdown.rs`: `render_markdown` (pulldown-cmark tables + strike +
    task lists), `DiffLine`/`parse_diff`, `looks_like_diff`, `pseudo_diff`
    (old/new content → diff lines) and `summarize_args`.
  - `src/state_ui.rs`: `UiStore` (Copy struct of Signals — dark_mode, page
    Chat/Settings/UsageLogs, projects, selected_project, runs, search,
    active run + messages + plan + usage, pending permission/question,
    provider catalog + provider/model/effort/mode, right panel, toast,
    prompt) with `refresh_initial`, `refresh_runs/projects`, `select_run`
    (loads run/messages/plan/usage/pending and attaches a live event stream
    for active runs) and `attach_event_stream` processing all emitted event
    types (`message_created/updated`, `status_changed`,
    `permission_requested`, `question_requested`, `plan_updated`,
    `run_title_changed`, `run_completed`, `run_failed`; `message_delta` /
    `tool_call_*` arrive as `message_updated` full-message replays).
  - `src/components/`: `sidebar.rs` (brand + OS badge, New Chat, Add Project
    via rfd folder picker or manual path modal, run search, project tree with
    status dots/model badges/per-run delete, footer nav), `chat_header.rs`
    (title, project pill, mode/status badges, token+cost summary, theme
    toggle, Stop Run via `runs::cancel_run`, Inspector toggle),
    `message_thread.rs` (user bubbles, Markdown assistant answers with
    collapsible `ReasoningPanel`, `ToolGroupCard`s pairing tool calls with
    their tool responses incl. auto `DiffView` for write/edit diffs,
    `SubAgentCard` groups for explorer/worker messages), `cards.rs`
    (`PermissionCard` with Allow Once/Run/Project/Always/Deny →
    `runs::permission_decision`; stepped `QuestionCard` with multi/single
    select options + custom notes → `runs::answer_question`), `composer.rs`
    (Enter/Shift+Enter textarea, 📎 file attach via rfd appending `@path`,
    @file/@symbol quick-insert bar with real project file chips, mode +
    provider + model + effort selectors, Send/Continue/Queue →
    `runs::create_run` / `continue_run` / `queue_message` attaching the
    pre-subscribed event receiver), `right_panel.rs` (Plan/Agents/Git/
    Terminal tabs — plan tasks, sub-agent summary, git status + commit/push),
    `settings.rs` (Providers with default picker, pi auth accounts, permission
    rule deletion, memory CRUD, info tabs for Skills/MCP/Plugins/Security),
    `usage_logs.rs` (metrics cards + paged 50-row table with search).
  - Backend: added `PartialEq` derives to `db.rs` structs and
    `ProviderMetadata` (required by the Dioxus component macro).
  - Verified: `cargo check --workspace` zero warnings/errors;
    `cargo build -p locagens-desktop` links; backend `cargo test` 48 unit + 4
    integration still pass; the window launches and stays running.
  - Not verified / known limits: no real end-to-end agent run was driven
    through the new UI yet; select/`value` binding uses `selected` attributes;
    terminal tab is a Phase 7b placeholder.
- [x] **Phase 7b: Desktop parity — inspector + settings tabs, unit tests**
  - `right_panel.rs` **Git tab**: branch/working-tree status, changed-file list from
    `api::projects::git_diff_details` (kind badges: created/edited/deleted/moved), per-file
    **Stage/Unstage** plus Stage all/Unstage all via the new `api::projects::git_stage` /
    `git_unstage` facade functions (backend addition), per-file unified **DiffView** built
    with `markdown::pseudo_diff` + `parse_diff`, and Commit / Push via `git_commit`.
    Data auto-refreshes (4 s poll) and re-polls immediately after every action via a
    version signal.
  - `right_panel.rs` **Terminal / Processes tab**: launch background commands in the
    selected project (`api::processes::spawn_process` with `SpawnProcessInput`),
    auto-refreshing process list (`list_processes`), per-process stdout/stderr log view
    (`process_logs`, 1.2 s poll, self-terminating when the process dies or the selection
    changes) and **Stop** (`kill_process`). The legacy placeholder is gone.
  - `settings.rs` — all 8 tabs fully wired to `locagens_core::api`:
    - **Providers** (catalog + default picker) and **pi Auth** (`providers::get_pi_auth`).
    - **Permissions**: standing grants list with delete (`permissions::*`).
    - **Memory**: list, create (Enter), **Edit → `memory::update_memory`**, delete.
    - **Skills** (`skills::list_skills/get_skill/install_skill/delete_skill`): list with
      user/project source badges, create + edit `SKILL.md` in a frontmatter-aware editor
      (`NEW_SKILL_TEMPLATE`), target scope selector (user scope or the selected project),
      delete. The previous static info tab is gone.
    - **MCP** (`mcp::list/upsert/toggle/delete_mcp_server`): server list with transport,
      scope, tool count and live status/error, enable/disable, delete, and an add form
      with stdio (command) / SSE (URL) transport selector.
    - **Plugins** (`plugins::list/install/toggle/delete_plugin` + `InstallPayload`):
      install from the built-in template catalog or a local folder, per-plugin
      enable/disable and uninstall. The previous static info tab is gone.
    - **Security** (`security::security_status` + `setup_sandbox(true)`): sandbox
      status/warnings/errors, workspace policy (deny-by-default network, workspace-writes
      only) and the Windows sandbox setup action when available.
  - **Unit tests (18 in `locagens-desktop`)**: `markdown.rs` — table/tasklist/strikethrough
    rendering, `parse_diff` kinds, empty input, `looks_like_diff`, `pseudo_diff`,
    `summarize_args` (command/path/url/edit/fallback/truncation); `theme.rs` —
    `theme_class`, distinct dark/light styles per platform, Windows Fluent fonts +
    `#60cdff`/`#0067c0` accents, macOS vibrancy fonts + `#0a84ff`/`#007aff` accents,
    Linux fallback, core layout section coverage and detected-OS ↔ explicit-platform
    equivalence (`build_stylesheet_for(platform, dark)` refactor makes every platform
    testable regardless of the build host).
  - `settings.rs`/`usage_logs.rs` inspected and confirmed wired to `api::runs::usage_logs`.
  - Note: the core `ProcessManager` exposes list/spawn/kill/restart/logs; there is no
    `clear_finished` operation, so the tab relies on the manager's own lifecycle pruning.
- [x] **Phase 8: Drop legacy `apps/web`, `apps/desktop`, `packages/shared` and the HTTP server** —
  merged branch `feat/phase8-cleanup` (`19f5422`) into `main` (merge commit `af5be81`),
  keeping the `PartialEq` derives added in `7283e9d` and the workspace member
  `apps/desktop-rs` while dropping:
  - `apps/web`, `apps/desktop`, `packages/shared`, root `package.json`, `tsconfig.json`,
    `pnpm-lock.yaml`, `pnpm-workspace.yaml` (and the stale untracked `node_modules` /
    `dist` build outputs).
  - The legacy HTTP server: `apps/backend/src/main.rs`, `routes/*`, `auth.rs`; the backend
    crate is now purely library `locagens_core` + binary `lcgn`.
  - Root `Cargo.toml` members: `["apps/backend", "apps/desktop-rs"]`.
  - CI updated: `.github/workflows/ci.yml` tests + release-builds both `locagens-backend`
    (CLI `lcgn`) and `locagens-desktop` (desktop `locagens`) per OS matrix;
    `.github/workflows/release-mac.yml` already collects both binaries per target.

### Final state (Phases 0–8 complete)

- Workspace: `apps/backend` (`locagens_core` lib + `lcgn` CLI) and `apps/desktop-rs`
  (native Dioxus 0.7 desktop app, binary `locagens`, in-process `locagens_core::api`,
  no HTTP). File-based JSON/JSONL storage, pure Rust `pi` provider/auth/model engine,
  pure Rust streaming agent loop with workspace tools, permission gating, `spawn_agents`
  fan-out, usage logs with cost.
- Binaries: **`locagens`** (desktop app) and **`lcgn`** (CLI).
- Tests: **98 passing** across the workspace — 76 `locagens_core` lib unit tests
  (incl. the Phase 9 Codex harness suites), 4 `lcgn` integration tests (mock
  OpenAI-compatible server), 18 `locagens-desktop` unit tests (11 markdown +
  7 theme) — zero warnings, `cargo build --release --workspace` clean on
  `stable-x86_64-pc-windows-gnu` + w64devkit.
- CI: `cargo test/build --release` for both crates on ubuntu/macos/windows matrices.
- Not verified / known limits: the desktop UI has not been exercised with a real
  end-to-end agent run in an automated harness (manual verification only); the `lcgn`
  interactive REPL and permission/question prompts still need a real TTY.

---


- [x] **Phase 9: Codex Harness Architecture port (`codex-rs` → Locagens) + `.vscode` tasks/launch** —
  implemented in this workspace:
  - **`.vscode/tasks.json` + `.vscode/launch.json`**: w64devkit PATH prepend,
    `rustup run stable-x86_64-pc-windows-gnu cargo …` tasks on Windows (plain
    `cargo` on linux/osx) for check / debug+release desktop & CLI builds /
    tests / run targets; cppdbg+gdb (`w64devkit` gdb) and CodeLLDB launch
    configurations for `locagens` and `lcgn --list-models`. `.gitignore`
    un-ignores both files.
  - **`orchestrator/apply_patch.rs`** (port of `codex-rs/apply-patch`):
    `*** Begin Patch` parser (add/delete/update/move hunks, multi-chunk
    updates, `@@ context`, `*** End of File`, lenient heredoc stripping),
    `seek_sequence` fuzzy matcher (exact → `trim_end` → `trim` → Unicode
    punctuation normalization), workspace-safe applier with `safe_path`
    containment (path traversal rejected), `preview_patch` +
    `affected_paths` + unified-diff generation and Codex's exact
    `Success. Updated the following files:` summary. 10 unit tests.
  - **`orchestrator/agents_md.rs`** (port of `core/src/agents_md.rs`):
    hierarchical discovery from git root → cwd, one doc per level
    (`AGENTS.override.md` > `AGENTS.md` > `CLAUDE.md` > `.agents.md`),
    `

--- project-doc ---

` separator, 32 KiB budget; injected into
    the system prompt.
  - **`orchestrator/env_context.rs`**: Codex `<environment_context>` XML
    block (cwd/shell/approval_policy/sandbox_mode/network_access/
    workspace_roots/current_date) seeded as the first user message.
  - **`orchestrator/command_safety.rs`**: Codex `ApprovalPolicy` +
    `SandboxMode` mapped from Locagens modes; known-safe read-only command
    classification (bypasses prompting in `accept_edits`), dangerous-command
    detection (`rm -rf`, `git reset --hard`, `sudo`, …).
  - **`orchestrator/turn_diff_tracker.rs`**: baseline snapshots before
    mutations, aggregate unified diff, `turn_diff_updated` events with
    `filesChanged`.
  - **`orchestrator/compact.rs`**: Codex SUMMARIZATION_PROMPT,
    auto-compaction above 90k estimated tokens (+ `compact_context` tool),
    `[Compacted context summary]` block, last 8 messages preserved.
  - **Tools (`orchestrator/tools.rs` + `mod.rs`)**: `apply_patch`,
    `exec_command` (ProcessManager sessions, secret-stripped env, yield /
    poll semantics) + `write_stdin` (stdin piping added to ProcessManager),
    `view_image`, `request_user_input` (Codex schema → QuestionCard),
    `update_plan` dual-schema normalization, and Codex interactive
    sub-agents `spawn_agent` / `send_input` / `wait_agent` / `close_agent` /
    `list_agents` (background tasks with per-agent inboxes).
  - **Config/auth (`paths.rs`, `pi_providers.rs`)**: primary directory
    `~/.locagens` (`$LOCAGENS_HOME`), read-only fallbacks `~/.codex`
    (`$CODEX_HOME`: `auth.json` `OPENAI_API_KEY` → `openai` provider,
    `config.toml` `[model_providers.*]` via the new `toml` dependency) and
    `~/.pi/agent` (legacy credentials keep working). UI/CLI labels updated
    to "Codex / Locagens Auth & Providers".
  - **Docs**: `Claude.md`, `Agents.md`, `GEMINI.md`, `README.md`,
    `docs/CORE_API.md`, `docs/CLI.md` updated with the Phase 9 sections.
  - Tests: **98 passing** across the workspace (76 `locagens_core` lib unit
    tests incl. the new apply-patch/agents-md/command-safety/compact/
    env-context/turn-diff suites, 4 `lcgn` integration tests, 18
    `locagens-desktop` unit tests) — zero warnings.

## 4. Build Notes (this Windows machine)

- Prepend `$HOME\w64devkit\w64devkit\bin` to `PATH`, then run
  `rustup run stable-x86_64-pc-windows-gnu cargo test` /
  `rustup run stable-x86_64-pc-windows-gnu cargo check`.

---

## 5. Detailed Technical Reference Location

- `Claude.md`, `Agents.md`, `GEMINI.md`
- Conversation scratch specs in
  `C:\Users\User\.gemini\antigravity\brain\e0c11c6e-0150-4410-b6e6-439e0ce4221b\scratch\`
