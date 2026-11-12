# Locagens — Migration State (`state.md`)

Authoritative live handoff and progress tracker for the **Locagens -> Rust + `pi`** migration.
Update this file continuously as phases complete.

---

## 1. Migration Goal & Architectural Decisions

- Replace the legacy Node.js/Fastify backend (`apps/api`) and SQLite sidecar (`apps/db-writer`)
  with a single unified **Rust (`Axum` + `Tokio` + `Rusqlite`)** binary
  (`apps/backend`, binary `locagens-backend`).
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

- [ ] **Phase 1: Rust Backend Foundation (`apps/backend`) + SQLite + Core Routes**
  *(IN PROGRESS — active `pi` task writing `apps/backend/src/`)*
  - Completed files so far: `Cargo.toml`, `apps/backend/Cargo.toml`, `src/db.rs`,
    `src/paths.rs`, `src/util.rs`, `src/auth.rs`,
    `src/services/{mod,processes,skills,plugins,mcp}.rs`.
  - Remaining in Phase 1: `src/routes/{settings,projects,memory,permissions,skills,processes,symbols,plugins,mcp,security}.rs`,
    `src/main.rs`, `cargo check` verification, commit & push.

- [ ] **Phase 2: `pi`-Backed Provider, Model & Credential Management (`/api/providers/*`, `/api/agent-presets`)**
  - Bridge `/api/providers`, `/api/providers/config`, `/api/providers/fetch-models`,
    `/api/providers/test`, `/api/agent-presets` to `~/.pi/agent/auth.json`,
    `~/.pi/agent/models.json`, `~/.pi/agent/models-store.json`, `pi auth`,
    and `pi --list-models`.

- [ ] **Phase 3: `pi --mode rpc` Agent Orchestrator & Run/SSE Engine (`/api/runs/*`, `/api/runs/:id/events`)**
  - Implement Rust `pi --mode rpc` process supervisor & JSONL protocol driver
    (`prompt`, `steer`, `follow_up`, `clear_queue`, `abort`,
    `extension_ui_request`/`extension_ui_response`).
  - Map `pi` streaming events (`message_start`, `message_update`, `tool_execution_*`,
    `agent_settled`) to SQLite (`runs`, `messages`, `plans`, `usage_logs`) and SSE broadcast
    (`GET /api/runs/:id/events`).
  - Wire permission gating (`PermissionCoordinator` via `pi` extension hook / RPC UI)
    and multi-agent delegation (`coder`, `utility`, `explorer`).

- [ ] **Phase 4: Frontend (`apps/web`), Desktop (`apps/desktop`) Integration & Legacy Node.js Removal**
  - Update `apps/desktop/src/backend.ts` & `paths.ts` to spawn the Rust `locagens-backend` binary.
  - Update `apps/web` provider settings to work natively with `pi` providers/auth.
  - Remove legacy `apps/api` and `apps/db-writer`, update root `package.json` &
    `pnpm-workspace.yaml`, run full build & tests, commit & push.

---

## 4. Detailed Technical Reference Location

- `Claude.md`, `Agents.md`, `GEMINI.md`
- Conversation scratch specs in
  `C:\Users\User\.gemini\antigravity\brain\e0c11c6e-0150-4410-b6e6-439e0ce4221b\scratch\`
