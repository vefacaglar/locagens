# Database Schema — Locagens

**Database engine:** SQLite (node:sqlite `DatabaseSync`)  
**File:** `apps/api/src/database/db.ts`  
**Default DB path:** `<workspace-root>/locagens.db` (or `:memory:` during tests)

---

## Table: `runs`

Stores one row per assistant run (conversation session).

| Column | Type | Constraints | Notes |
|---|---|---|---|
| `id` | `TEXT` | `PRIMARY KEY` | UUID or similar unique identifier |
| `title` | `TEXT` | `NOT NULL` | Human-readable run title |
| `task` | `TEXT` | `NOT NULL` | The user's task description |
| `project_path` | `TEXT` | `NOT NULL` `DEFAULT '<workspace-root>'` | Absolute path of the active workspace project |
| `project_name` | `TEXT` | `NOT NULL` `DEFAULT 'Locagens'` | Display name of the project |
| `status` | `TEXT` | `NOT NULL` | One of: `created`, `generating`, `awaiting_permission`, `running`, `failed`, `completed`, etc. |
| `provider_id` | `TEXT` | `NOT NULL` | AI provider identifier (e.g. `openai`, `anthropic`) |
| `provider_display_name` | `TEXT` | `NOT NULL` | Human-readable provider name |
| `model` | `TEXT` | `NOT NULL` | Model name (e.g. `gpt-4o`, `claude-sonnet-4`) |
| `mode` | `TEXT` | `NOT NULL` `DEFAULT 'accept_edits'` | Run mode (e.g. `accept_edits`, `plan`, `architect`) |
| `error_message` | `TEXT` | nullable | Error text if the run failed |
| `created_at` | `TEXT` | `NOT NULL` | ISO-8601 timestamp |
| `updated_at` | `TEXT` | `NOT NULL` | ISO-8601 timestamp |
| `last_active_at` | `TEXT` | nullable | ISO-8601 timestamp, populated on startup if null |
| `coder_provider_id` | `TEXT` | nullable | † Provider ID for the coder sub-agent (dual-model mode) |
| `coder_model` | `TEXT` | nullable | † Model name for the coder sub-agent |
| `agent_preset` | `TEXT` | nullable | † Agent preset name (e.g. `architect-coder`) |
| `utility_provider_id` | `TEXT` | nullable | † Provider ID for utility/fast model calls |
| `utility_model` | `TEXT` | nullable | † Model name for utility/fast model calls |
| `coder_reasoning_effort` | `TEXT` | nullable | † Reasoning effort for coder model (e.g. `low`, `medium`, `high`) |
| `utility_reasoning_effort` | `TEXT` | nullable | † Reasoning effort for utility model |
| `reasoning_effort` | `TEXT` | nullable | † Optional reasoning-effort selection for the primary model |

**† Migration columns** – added via `ALTER TABLE … ADD COLUMN` in post-CREATE migrations:

1. `mode` — added in a migration (also present in the current CREATE TABLE)
2. `last_active_at`
3. `coder_provider_id`, `coder_model`, `agent_preset`, `utility_provider_id`, `utility_model`
4. `coder_reasoning_effort`, `utility_reasoning_effort`
5. `reasoning_effort`

**Indexes:** none declared explicitly.

**Startup routine:** Runs in `'created'`, `'generating'`, or `'awaiting_permission'` status are reset to `'failed'` with `error_message = 'Session interrupted due to server restart.'`.

---

## Table: `messages`

Stores individual chat messages within a run.

| Column | Type | Constraints | Notes |
|---|---|---|---|
| `id` | `TEXT` | `PRIMARY KEY` | Unique message identifier |
| `run_id` | `TEXT` | `NOT NULL` `FK → runs(id)` | Parent run |
| `role` | `TEXT` | `NOT NULL` | `user`, `assistant`, `system`, `tool`, etc. |
| `agent_role` | `TEXT` | nullable | Sub-role: `architect`, `coder`, or `null` for single-agent |
| `agent_name` | `TEXT` | nullable | † Coder sub-agent identity (display name) |
| `provider_id` | `TEXT` | nullable | Provider used for this message |
| `provider_display_name` | `TEXT` | nullable | Human-readable provider name |
| `model` | `TEXT` | nullable | Model used for this message |
| `content` | `TEXT` | `NOT NULL` | Message body text |
| `reasoning_content` | `TEXT` | nullable | † Model's chain-of-thought / reasoning text |
| `raw_response` | `TEXT` | nullable | Full raw API response JSON |
| `created_at` | `TEXT` | `NOT NULL` | ISO-8601 timestamp |

**† Migration columns** – added via `ALTER TABLE`:

1. `reasoning_content` — added for storing chain-of-thought output
2. `agent_name` — added for identifying coder sub-agent identity

**Foreign keys:**

| Column | References | On Delete |
|---|---|---|
| `run_id` | `runs(id)` | (not specified; SQLite default — no action) |

**Indexes:** none declared explicitly.

---

## Table: `projects`

Tracks known workspace projects.

| Column | Type | Constraints | Notes |
|---|---|---|---|
| `path` | `TEXT` | `PRIMARY KEY` | Absolute filesystem path |
| `name` | `TEXT` | `NOT NULL` | Display name |
| `created_at` | `TEXT` | `NOT NULL` | ISO-8601 timestamp |

**Indexes:** none declared explicitly.

**Default seeding:** On first launch a row is inserted for the workspace root with name `'Locagens'`.

**Startup update:** Sets `name` to the default project name for rows where `path` is the workspace root (or root with trailing `/`) and the current name equals `path.basename(wsRoot)`.

---

## Table: `permissions`

Per-tool and per-command grant records for security approval.

| Column | Type | Constraints | Notes |
|---|---|---|---|
| `id` | `INTEGER` | `PRIMARY KEY AUTOINCREMENT` | Auto-generated surrogate key |
| `scope` | `TEXT` | `NOT NULL` | Scope kind — likely `'global'` or `'project'` |
| `project_path` | `TEXT` | `NOT NULL` `DEFAULT ''` | Project path (empty string = all projects) |
| `tool` | `TEXT` | `NOT NULL` `DEFAULT ''` | Tool name (e.g. `run_command`, `write_file`, `search_files`) |
| `command` | `TEXT` | `NOT NULL` `DEFAULT ''` | Exact command string (for `run_command`); empty for other tools |
| `network_domains` | `TEXT` | `NOT NULL` `DEFAULT '[]'` | Sorted JSON array of exact network domains approved with the command |
| `status` | `TEXT` | `NOT NULL` | e.g. `'approved'`, `'denied'` |
| | | `UNIQUE(scope, project_path, tool, command, network_domains)` | Prevents duplicate exact command/domain grants |

**Indexes:** none declared explicitly.

**Migration note:** Legacy tables without `tool` or `network_domains` allowed overly broad grants. If either column is missing, the table is **dropped and recreated**, clearing all existing grants.

---

## Table: `plans`

Stores plan documents associated with a run. A run may have multiple plan versions over time.

| Column | Type | Constraints | Notes |
|---|---|---|---|
| `id` | `TEXT` | `PRIMARY KEY` | Unique plan identifier |
| `run_id` | `TEXT` | `NOT NULL` `FK → runs(id)` | Parent run |
| `title` | `TEXT` | `NOT NULL` `DEFAULT 'Plan'` | Plan title |
| `body` | `TEXT` | nullable | Plan body / description (JSON or Markdown) |
| `tasks` | `TEXT` | `NOT NULL` `DEFAULT '[]'` | JSON array of task objects |
| `status` | `TEXT` | `NOT NULL` `DEFAULT 'active'` | `'active'`, `'superseded'`, etc. |
| `version` | `INTEGER` | `NOT NULL` `DEFAULT 1` | Monotonically increasing version per run |
| `created_at` | `TEXT` | `NOT NULL` | ISO-8601 timestamp |
| `updated_at` | `TEXT` | `NOT NULL` | ISO-8601 timestamp |

**Foreign keys:**

| Column | References | On Delete |
|---|---|---|
| `run_id` | `runs(id)` | (not specified) |

**Indexes:**

| Index name | On | Notes |
|---|---|---|
| `idx_plans_run` | `plans(run_id)` | Accelerates lookups by run |

---

## Table: `memory`

Durable facts the assistant remembers across sessions (set via the `remember` tool).

| Column | Type | Constraints | Notes |
|---|---|---|---|
| `id` | `INTEGER` | `PRIMARY KEY AUTOINCREMENT` | Auto-generated surrogate key |
| `scope` | `TEXT` | `NOT NULL` | `'global'` or `'project'` |
| `project_path` | `TEXT` | `NOT NULL` `DEFAULT ''` | Empty string = global scope; project path otherwise |
| `category` | `TEXT` | `NOT NULL` `DEFAULT 'project'` | Category label |
| `content` | `TEXT` | `NOT NULL` | The remembered fact |
| `created_at` | `TEXT` | `NOT NULL` | ISO-8601 timestamp |
| `updated_at` | `TEXT` | `NOT NULL` | ISO-8601 timestamp |

**Indexes:**

| Index name | On | Notes |
|---|---|---|
| `idx_memory_scope_project` | `memory(scope, project_path)` | Accelerates queries filtering by scope and project |

---

## Table: `usage_logs`

Detailed records of all LLM completions, including token counts, cache hits, and calculated costs.

| Column | Type | Constraints | Notes |
|---|---|---|---|
| `id` | `INTEGER` | `PRIMARY KEY AUTOINCREMENT` | Auto-generated surrogate key |
| `run_id` | `TEXT` | `NOT NULL` `FK → runs(id)` | Parent run |
| `agent_role` | `TEXT` | nullable | Agent role (e.g., `planner`, `coder`, `utility`) |
| `provider_id` | `TEXT` | `NOT NULL` | Provider identifier (e.g. `openai`, `anthropic`) |
| `model` | `TEXT` | `NOT NULL` | Model name used for completion |
| `input_tokens` | `INTEGER` | `NOT NULL` `DEFAULT 0` | Input / prompt tokens count |
| `output_tokens` | `INTEGER` | `NOT NULL` `DEFAULT 0` | Output / completion tokens count |
| `cache_read_tokens` | `INTEGER` | `NOT NULL` `DEFAULT 0` | Cache hit (read) tokens count |
| `cache_write_tokens` | `INTEGER` | `NOT NULL` `DEFAULT 0` | Cache miss (written) tokens count |
| `cache_hit_rate` | `REAL` | `NOT NULL` `DEFAULT 0.0` | Cache hit rate as percentage (0 to 100) |
| `cost` | `REAL` | `NOT NULL` `DEFAULT 0.0` | Estimated cost of this call in USD |
| `created_at` | `TEXT` | `NOT NULL` | ISO-8601 timestamp |

**Indexes:**

| Index name | On | Notes |
|---|---|---|
| `idx_usage_logs_run` | `usage_logs(run_id)` | Accelerates lookups of usage logs by run |

---

## Relationship Summary

```
runs ──┬── messages     (1:N via run_id)
       ├── plans        (1:N via run_id)
       ├── usage_logs   (1:N via run_id)
       └── (implicitly referenced by permissions/memory via project_path)
```

- **`projects`** is a standalone lookup table.
- **`permissions`** uses `scope` + `project_path` (not a foreign key) to associate grants with projects.
- **`memory`** uses `scope` + `project_path` (not a foreign key) to associate facts with projects.

---

## Index Summary

| Table | Index Name | Columns |
|---|---|---|
| `plans` | `idx_plans_run` | `run_id` |
| `memory` | `idx_memory_scope_project` | `scope, project_path` |
| `usage_logs` | `idx_usage_logs_run` | `run_id` |

---

## Notes

- All timestamp columns use ISO-8601 text format (`TEXT`).
- SQLite is configured with `busy_timeout = 10000`, `journal_mode = WAL`, and `synchronous = NORMAL` for concurrency.
- A `DbWriterClient` background writer (the `@locagens/db-writer` Node.js sidecar process in `apps/db-writer`) is used when not in `:memory:` mode to serialise writes.
- The `runs` table has a startup cleanup routine: runs stuck in `'created'`, `'generating'`, or `'awaiting_permission'` are marked `'failed'` and a system message is inserted.
- Migration columns are added idempotently via `ALTER TABLE … ADD COLUMN` wrapped in try/catch.
