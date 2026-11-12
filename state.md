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

---

## 2. Mandatory Agent & Development Rules

- **Codebase exploration (read-only)**: Always use `gemini-3.8-flash`
  (`research` subagent with `Model: "flash"`).
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

- [x] **Phase 5: `lcgn` CLI (pi-like command line on the shared Rust core)** — branch `feat/loc-cli`, not pushed, pending verifier + merge
  - `apps/backend` is now a library (`locagens_core`, `src/lib.rs`, `build_core()`, `state.rs`) plus two binaries: `locagens-backend` (unchanged behavior/routes) and `lcgn` (`src/bin/lcgn.rs`, helpers in `src/cli/`: args, auth, models, render, repl, runner).
  - `routes/runs.rs`: create/continue/cancel/permission/answer logic extracted into `*_core` functions shared by the HTTP handlers and the CLI (HTTP statuses/bodies unchanged). `orchestrator/mod.rs`: only adds `--system-prompt`/`--append-system-prompt` support (`Run.system_prompt`/`append_system_prompt`, skipped in JSON when unset).
  - `db.rs`: advisory cross-process lock `<data>/.lock` (fs2) around every mutation of `projects.json`, `permissions.json`, `memory.json`, `usage_logs.jsonl`, re-reading them from disk under the lock; two-`Db` interleave test.
  - Flags: `-p`, `--model`, `--provider`, `--thinking`, `--mode text|json`, `--plan`, `--approve/-a`, `-c`, `-r`, `--session`, `--no-session`, `--no-tools`, `--system-prompt`, `--append-system-prompt`, `--cwd`, `--list-models`; `auth login|logout|check`; `sessions`; `@file` + piped stdin; exit codes 0/1/2; interactive REPL with slash commands (`/model /thinking /plan /build /new /sessions /usage /help /exit`).
  - `docs/CLI.md`, README CLI section; `apps/desktop/package.json` `extraResources` now also bundles `lcgn`/`lcgn.exe` (`build:backend` already builds both bins).
  - Verified: `cargo test` 50 unit tests (33 pre-existing + 17 new) + 4 integration tests (mock OpenAI-compatible server) all pass; live against `opencode-go/glm-5.3-flash`: `--list-models`, `-p ... --no-tools`, tool round trip with `--approve`, denial without `--approve`, `--mode json`, `-c`, `--plan`, bad model → exit 2.
  - Not verified / known limits: the interactive REPL, Ctrl+C cancel and the permission/question prompts were not exercised on a real TTY (no TTY in the test harness). A running `locagens-backend` keeps runs/projects in memory, so sessions created by `lcgn` appear in the desktop app after its next start (no live refresh yet). With an open non-TTY stdin that never closes, `lcgn -p` waits for stdin EOF (same as pi); close it with `$null |` or `< NUL`. Full-screen TUI is out of scope.

---

## 4. Build Notes (this Windows machine)

- Prepend `$HOME\w64devkit\w64devkit\bin` to `PATH`, then run
  `rustup run stable-x86_64-pc-windows-gnu cargo test` /
  `rustup run stable-x86_64-pc-windows-gnu cargo check`.

---

## 5. Detailed Technical Reference Location

- `Claude.md`, `Agents.md`, `GEMINI.md`
- Conversation scratch specs in
  `C:\Users\User\.gemini\antigravity\brain\e0c11c6e-0150-4410-b6e6-439e0ce4221b\scratch\`
