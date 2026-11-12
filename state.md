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

- [ ] **Phase 3: `pi --mode rpc` Agent Orchestrator & Run/SSE Engine (`/api/runs/*`, `/api/runs/:id/events`)**
  - Implement Rust `pi --mode rpc` process supervisor & JSONL protocol driver
    (`prompt`, `steer`, `follow_up`, `clear_queue`, `abort`,
    `extension_ui_request`/`extension_ui_response`).
  - Map `pi` streaming events (`message_start`, `message_update`, `tool_execution_*`,
    `agent_settled`) to the file-based store (`sessions/<id>/run.json`, `messages.jsonl`,
    `plan.json`, `usage_logs.jsonl`) and SSE broadcast (`GET /api/runs/:id/events`).
  - Wire permission gating (`PermissionCoordinator` via `pi` extension hook / RPC UI)
    and multi-agent delegation (`coder`, `utility`, `explorer`).

- [ ] **Phase 4: Frontend (`apps/web`), Desktop (`apps/desktop`) Integration & Legacy Node.js Removal**
  - Update `apps/desktop/src/backend.ts` & `paths.ts` to spawn the Rust `locagens-backend` binary.
  - Update `apps/web` provider settings to work natively with `pi` providers/auth.
  - Remove legacy `apps/api` and `apps/db-writer`, update root `package.json` &
    `pnpm-workspace.yaml`, run full build & tests, commit & push.

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
