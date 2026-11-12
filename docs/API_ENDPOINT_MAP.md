# API Endpoint Map

_Generated from `apps/backend/src/routes/` — grouped by Rust route module. `/ping`
is public; every `/api/*` route requires the process-local bearer token
(`AppState.auth_token`)._

**Implementation:** Rust (`Axum` + `Tokio`) — `apps/backend/src/main.rs` boots
the app, `apps/backend/src/routes/mod.rs` mounts all routers on the root
(`Axum::Router`). Storage lives in the file-based store `apps/backend/src/db.rs`
(JSON + JSONL, no SQLite) and provider/auth state in
`apps/backend/src/pi_providers.rs` (`~/.pi/agent/auth.json` & `models.json`).

---

## `routes/mod.rs` (base)

| Method | Path | Description | Handler (Rust) / Storage |
|--------|------|-------------|--------------------------|
| GET | `/ping` | Health check (status + timestamp) | inline `ping()` in `routes/mod.rs` |

---

## Run orchestration (`sessions/<run_id>/` storage)

Run endpoints are served by the run orchestration layer; all persistence goes
through `apps/backend/src/db.rs` (JSON/JSONL session files) — there is no SQLite.

| Method | Path | Description | Handler (Rust) / Storage |
|--------|------|-------------|--------------------------|
| POST | `/api/runs` | Create and trigger a new orchestration run | `db::Db::create_run(run)`, `sessions/<run_id>/run.json` |
| POST | `/api/runs/:id/cancel` | Cancel a running orchestration job | `db::Db::patch_run` (status → `failed`/`completed`) |
| POST | `/api/runs/:id/continue` | Continue a run with follow-up instructions in the same thread | `db::Db::patch_run`, append `messages.jsonl`, `db::Db::touch_run` |
| POST | `/api/runs/:id/messages` | Send a message to a run that is still working; delivered at the main agent's next step (409 when the run is not working) | append `sessions/<run_id>/messages.jsonl` |
| POST | `/api/runs/:id/permission` | Resolve a pending permission request for a running job | permission store (`permissions.json`), `db::Db::patch_run` |
| POST | `/api/runs/:id/answer` | Resolve a pending `ask_user_question` with user's selections | `sessions/<run_id>/messages.jsonl` |
| GET | `/api/runs` | Run history list | `db::Db::list_runs()` — walks `sessions/*/run.json` |
| GET | `/api/runs/:id` | Single run details | `db::Db::get_run(id)` → `RunRow` |
| GET | `/api/runs/:id/messages` | Messages for a single run | `sessions/<run_id>/messages.jsonl` |
| GET | `/api/runs/:id/usage` | Aggregated token/cost totals for a run, broken down by agent role | `usage_logs.jsonl` scan by `run_id` |
| GET | `/api/runs/:id/plan` | Active plan for a single run (drives plan side panel) | `sessions/<run_id>/plan.json` |
| GET | `/api/runs/:id/pending` | Pending user-facing request (permission / question) | orchestration state |
| GET | `/api/runs/:id/events` | SSE event stream for a specific run | `tokio-stream` sync watch / broadcast bus |
| GET | `/api/usage-logs` | Paginated/filterable usage log history | `usage_logs.jsonl` scan |

---

## `routes/providers.rs` — provider & agent presets

| Method | Path | Description | Handler (Rust) / Storage |
|--------|------|-------------|--------------------------|
| GET | `/api/providers` | Safe provider metadata (never exposes API keys) | `pi_providers::PiProviderManager` (`~/.pi/agent/auth.json`, `models.json`, `models-store.json`) |
| POST | `/api/providers/test` | Test a provider/model with a one-off completion | `pi_providers::PiProviderManager::test` |
| GET | `/api/providers/config` | Get editable provider configurations (API keys masked) | `pi_providers.rs` + `locagens-providers-meta.json` |
| POST | `/api/providers/config` | Save provider configurations | `pi_providers.rs` → `auth.json` / `models.json` writes |
| GET | `/api/agent-presets` | Dual-model agent presets (architect + coder) | `locagens-presets.json` via `pi_providers.rs` |
| PUT | `/api/agent-presets` | Save the full agent-preset set | `pi_providers.rs` → `locagens-presets.json` |
| POST | `/api/providers/fetch-models` | Fetch available models dynamically from a provider endpoint | inline `reqwest` fetch to provider API (`/models` or `/model`) |
| GET | `/api/pi/auth` | Native `pi` auth status (`~/.pi/agent/auth.json`) | `pi_providers::PiProviderManager` |
| POST | `/api/pi/auth` | Set a `pi` credential entry | `pi_providers.rs` → `~/.pi/agent/auth.json` |

---

## `routes/projects.rs` — projects, files & git

| Method | Path | Description | Handler (Rust) / Storage |
|--------|------|-------------|--------------------------|
| GET | `/api/projects` | List all projects | `db::Db::list_projects()` — `projects.json` |
| POST | `/api/projects` | Create/add a project manually | `db::Db::upsert_project` — `projects.json` |
| DELETE | `/api/projects` | Remove a project from the list | `db::Db::delete_project` — `projects.json` |
| GET | `/api/projects/files` | List workspace files for a project | `routes/projects.rs` (workspace walk, path-contained) |
| POST | `/api/projects/select-dir` | Trigger native OS folder picker (returns canonical path + name) | inline OS dialog / stdin picker |
| GET | `/api/projects/git/status` | Git branch and dirty-state for a registered canonical project | `tokio::process::Command("git", args)` |
| GET | `/api/projects/git/diff-details` | File-level diff details without mutating the Git index | `git` invocation, guarded disk reads |
| POST | `/api/projects/git/generate-message` | Generate a commit message for a run's registered project | provider completion via `pi_providers.rs` + `git diff` |
| POST | `/api/projects/git/commit` | Commit and optionally push using literal argument arrays; repository hooks are disabled | `git` invocation (literal argv, no shell) |
| GET | `/api/projects/symbols` | Search functions, classes, interfaces across workspace | `routes/symbols.rs` (`services/symbols.rs` indexer) |

---

## `routes/permissions.rs`

| Method | Path | Description | Handler (Rust) / Storage |
|--------|------|-------------|--------------------------|
| GET | `/api/permissions` | List all standing permissions | `db::Db::list_permissions()` — `permissions.json` |
| DELETE | `/api/permissions/:id` | Revoke a single permission by id | `db::Db::delete_permission(id)` |
| DELETE | `/api/permissions` | Revoke all permissions | `db::Db::clear_permissions()` |

---

## `routes/memory.rs`

| Method | Path | Description | Handler (Rust) / Storage |
|--------|------|-------------|--------------------------|
| GET | `/api/memories` | List every saved memory (global + per-project) | `db::Db::list_memories()` — `memory.json` |
| POST | `/api/memories` | Manually add a memory from the UI | `db::Db::create_memory({scope, projectPath, category, content})` |
| PUT | `/api/memories/:id` | Edit a memory's content | `db::Db::update_memory(id, content)` |
| DELETE | `/api/memories/:id` | Delete a single memory | `db::Db::delete_memory(id)` |
| DELETE | `/api/memories` | Clear all memories | `db::Db::clear_memories()` |

---

## `routes/skills.rs`

| Method | Path | Description | Handler (Rust) / Storage |
|--------|------|-------------|--------------------------|
| GET | `/api/skills` | List discovered skills + folder roots (optional `projectPath`) | `services/skills.rs::SkillRegistry::list_summaries` |
| GET | `/api/skills/:name` | Fetch a single skill body | `services/skills.rs::SkillRegistry` |
| POST | `/api/skills/install` | Install SKILL.md text into user/project skills (`target`, `content`, optional `projectPath`) | `services/skills.rs::install_skill_md` |
| DELETE | `/api/skills/:name` | Delete a skill from user or project root (`target`, optional `projectPath`) | `services/skills.rs::delete_skill` |
| POST | `/api/skills/open-folder` | Resolve/create allowlisted skills dir (`user` \| `project`) | `services/skills.rs::ensure_user_root` / `ensure_project_root` |

---

## `routes/mcp.rs`

| Method | Path | Description | Handler (Rust) / Storage |
|--------|------|-------------|--------------------------|
| GET | `/api/mcp/servers` | List configured MCP servers, connection status and tools | `services/mcp.rs::McpManager::status_list(projectPath)` |
| POST | `/api/mcp/servers` | Save/create MCP server configuration | `services/mcp.rs` config store save + restart |
| DELETE | `/api/mcp/servers/:name` | Remove an MCP server configuration | `services/mcp.rs::delete_server` |
| POST | `/api/mcp/servers/:name/restart` | Restart/reconnect an MCP server process | `services/mcp.rs::restart_server` |
| POST | `/api/mcp/servers/:name/toggle` | Enable or disable an MCP server | `services/mcp.rs::toggle_server` |

---

## `routes/plugins.rs`

| Method | Path | Description | Handler (Rust) / Storage |
|--------|------|-------------|--------------------------|
| GET | `/api/plugins` | List discovered plugins + built-in templates (context-mode etc.) | `services/plugins.rs::discover`, `get_templates` |
| GET | `/api/plugins/templates` | List built-in quick plugin templates | `services/plugins.rs::get_templates` (`plugins_templates.json`) |
| POST | `/api/plugins/install` | Install plugin from template, GitHub repo, NPM package, or JSON | `services/plugins.rs::install`, auto-registers bundled MCP servers |
| POST | `/api/plugins/:id/toggle` | Enable or disable a plugin | `services/plugins.rs::toggle` |
| DELETE | `/api/plugins/:id` | Delete a plugin directory from user or project root | `services/plugins.rs::delete` |

---

## `routes/processes.rs`

| Method | Path | Description | Handler (Rust) / Storage |
|--------|------|-------------|--------------------------|
| GET | `/api/processes` | List active/stopped background processes | `services/processes.rs::ProcessManager::list(projectPath)` |
| POST | `/api/processes/spawn` | Start a background command/server | `services/processes.rs::spawn_process(...)` (`tokio::process`) |
| POST | `/api/processes/:id/kill` | Terminate a running process | `services/processes.rs::kill(id)` |
| POST | `/api/processes/:id/restart` | Restart a background process | `services/processes.rs::restart(id)` |
| GET | `/api/processes/:id/logs` | Fetch stdout/stderr logs for a process | `services/processes.rs::get_logs(id)` |

---

## `routes/settings.rs`

| Method | Path | Description | Handler (Rust) / Storage |
|--------|------|-------------|--------------------------|
| GET | `/api/settings` | Current local app settings (backend port) | `routes/settings.rs::read_stored_port` |
| PUT | `/api/settings` | Update app settings (port change requires restart) | `routes/settings.rs::put_settings` → `settings.json` |

---

## `routes/security.rs`

| Method | Path | Description | Handler (Rust) / Storage |
|--------|------|-------------|--------------------------|
| GET | `/api/security/status` | Report local API and command-sandbox readiness | `routes/security.rs::status` |
| POST | `/api/security/sandbox/setup` | Install the Windows sandbox runtime after explicit UI confirmation | `routes/security.rs::sandbox_setup` |
