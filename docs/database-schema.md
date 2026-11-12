# Database Schema — Locagens

**Storage engine:** File-Based Storage (Codex/Claude/Pi-style `JSON` + `JSONL`) — zero SQLite, zero C library dependency.
**Implementation:** `apps/backend/src/db.rs` (pure Rust, in-memory `RwLock<StorageState>` + durable file persistence).
**Replaces:** the former SQLite engine (`node:sqlite` / `rusqlite`), `locagens.db`, and the `@locagens/db-writer` sidecar writer process.

**Default data root:** `<app_support_dir>/data`, overridable via `LOCAGENS_DATA_DIR` (or derived from the legacy `LOCAGENS_DB_PATH` parent/stem). Path resolution lives in `apps/backend/src/paths.rs`.

---

## File layout

```txt
<data-root>/
  projects.json               — list of Project
  permissions.json            — list of PermissionRule (auto-increment ids)
  memory.json                 — list of Memory (auto-increment ids)
  usage_logs.jsonl            — append-only JSONL of UsageLogEntry
  sessions/<run_id>/run.json       — Run metadata
  sessions/<run_id>/messages.jsonl — append-only JSONL of RunMessage
  sessions/<run_id>/plan.json      — active Plan for the run
```

All state is kept in an in-memory `RwLock<StorageState>` so reads are O(1); every
mutation is durably persisted:

- **JSON files** (`projects.json`, `permissions.json`, `memory.json`,
  `sessions/<run_id>/run.json`, `sessions/<run_id>/plan.json`) are serialized
  and written to a `.tmp` sibling, then renamed atomically.
- **JSONL files** (`usage_logs.jsonl`, `sessions/<run_id>/messages.jsonl`) are
  appended and flushed; when a line is updated in place, the file is rewritten
  atomically.
- Deleting a run removes the whole `sessions/<run_id>/` directory. Startup
  cleanup walks `sessions/` and resets runs stuck in `created`, `generating`, or
  `awaiting_permission` to `failed` (session folders keep their content).

---

## Record types

### `projects.json` — `Project`

Tracks known workspace projects.

| Field | Type | Notes |
|---|---|---|
| `path` | `string` | Absolute filesystem path (primary key) |
| `name` | `string` | Display name (default `Locagens` for the workspace root) |
| `createdAt` | `string` | ISO-8601 timestamp |

### `permissions.json` — `PermissionRule`

Per-tool and per-command grant records for security approval.

| Field | Type | Notes |
|---|---|---|
| `id` | `integer` | Auto-increment surrogate key (monotonic counter in the store) |
| `scope` | `string` | `'global'` or `'project'` |
| `projectPath` | `string` | Project path (empty string = all projects) |
| `tool` | `string` | Tool name (e.g. `run_command`, `write_file`, `search_files`) |
| `command` | `string` | Exact command string (for `run_command`); empty for other tools |
| `networkDomains` | `string[]` | Sorted list of exact network domains approved with the command |
| `status` | `string` | e.g. `'approved'`, `'denied'` |

Duplicate exact command/domain grants are prevented on write (the old SQLite
`UNIQUE(scope, project_path, tool, command, network_domains)` constraint).

### `memory.json` — `Memory`

Durable facts the assistant remembers across sessions (set via the `remember`
tool / Settings → Memory).

| Field | Type | Notes |
|---|---|---|
| `id` | `integer` | Auto-increment surrogate key |
| `scope` | `string` | `'global'` or `'project'` |
| `projectPath` | `string` | Empty string = global scope; project path otherwise |
| `category` | `string` | Category label (default `project`) |
| `content` | `string` | The remembered fact |
| `createdAt` / `updatedAt` | `string` | ISO-8601 timestamps |

### `usage_logs.jsonl` — `UsageLogEntry`

Append-only detailed records of all LLM completions, including token counts,
cache hits, and calculated costs. One JSON object per line.

| Field | Type | Notes |
|---|---|---|
| `id` | `integer` | Auto-increment surrogate key |
| `runId` | `string` | Parent run (`sessions/<run_id>/`) |
| `agentRole` | `string?` | Agent role (`main`, `explorer`, `worker`) |
| `providerId` | `string` | Provider identifier |
| `model` | `string` | Model name used for completion |
| `inputTokens` / `outputTokens` | `integer` | Prompt / completion token counts |
| `cacheReadTokens` / `cacheWriteTokens` | `integer` | Cache hit / miss-write token counts |
| `cacheHitRate` | `number` | Cache hit rate as percentage (0–100) |
| `cost` | `number` | Estimated cost of this call in USD |
| `createdAt` | `string` | ISO-8601 timestamp |

### `sessions/<run_id>/run.json` — `Run`

One file per assistant run (conversation session).

| Field | Type | Notes |
|---|---|---|
| `id` | `string` | UUID (folder name of the session) |
| `title` | `string` | Human-readable run title |
| `task` | `string` | The user's task description |
| `projectPath` / `projectName` | `string` | Active workspace project (path defaults to the workspace root) |
| `status` | `string` | `created`, `generating`, `awaiting_permission`, `running`, `failed`, `completed`, … |
| `providerId` / `providerDisplayName` | `string` | Selected provider (resolved from `~/.pi/agent/auth.json` / `models.json`) |
| `model` | `string` | Model name |
| `mode` | `string` | Run mode (`accept_edits`, `plan`, `auto`, `chat`, `ask_permissions`) |
| `errorMessage` | `string?` | Error text if the run failed |
| `createdAt` / `updatedAt` | `string` | ISO-8601 timestamps |
| `lastActiveAt` | `string?` | Populated on startup if null |
| `reasoningEffort` | `string?` | Per-model reasoning-effort selection |
| `systemPrompt` / `appendSystemPrompt` | `string?` | Custom system prompt (`lcgn --system-prompt` / `--append-system-prompt`); omitted from JSON when unset |

Legacy `run.json` files may still contain `coder*`/`utility*`/`agentPreset`
keys from before preset removal; they deserialize cleanly (unknown keys are
ignored) but are no longer read or written.

Derived view: `db::Db::get_run()` projects this into a `RunRow` with only the
columns consumed by route modules.

### `sessions/<run_id>/messages.jsonl` — `RunMessage`

Append-only JSONL of individual chat messages within a run.

| Field | Type | Notes |
|---|---|---|
| `id` | `string` | Unique message identifier |
| `runId` | `string` | Parent run (matches the session folder) |
| `role` | `string` | `user`, `assistant`, `system`, `tool`, … |
| `agentRole` | `string?` | Sub-role: `explorer`, `worker`, or `null` for the main agent |
| `agentName` | `string?` | Sub-agent identity (display name) |
| `providerId` / `providerDisplayName` / `model` | `string?` | Provider/model used for this message |
| `content` | `string` | Message body text |
| `reasoningContent` | `string?` | Model's chain-of-thought / reasoning text |
| `rawResponse` | `string?` | Full raw API response JSON |
| `createdAt` | `string` | ISO-8601 timestamp |

### `sessions/<run_id>/plan.json` — `Plan`

The active plan document for a run (drives the right-hand plan panel). A new
plan supersedes the previous one in place.

| Field | Type | Notes |
|---|---|---|
| `id` | `string` | Unique plan identifier |
| `runId` | `string` | Parent run (matches the session folder) |
| `title` | `string` | Plan title (default `Plan`) |
| `body` | `string?` | Plan body / description (JSON or Markdown) |
| `tasks` | `array` | JSON array of task objects (default `[]`) |
| `status` | `string` | `'active'`, `'superseded'`, … (default `active`) |
| `version` | `integer` | Monotonically increasing version per run (default `1`) |
| `createdAt` / `updatedAt` | `string` | ISO-8601 timestamps |

---

## Relationship Summary

```
sessions/<run_id>/run.json ──┬── messages.jsonl   (same folder, 1:N)
                             ├── plan.json        (same folder, 1 active)
                             └── usage_logs.jsonl (matched by runId)
```

- **`projects.json`** is a standalone lookup file.
- **`permissions.json`** uses `scope` + `projectPath` (not a foreign key) to
  associate grants with projects.
- **`memory.json`** uses `scope` + `projectPath` (not a foreign key) to
  associate facts with projects.

---

## Provider & auth configuration (not app data)

Provider credentials and model catalogs live in `pi`'s agent directory
(`~/.pi/agent`, overridable via `PI_CODING_AGENT_DIR`) and are managed by
`apps/backend/src/pi_providers.rs`:

```txt
~/.pi/agent/auth.json          — provider id → credential
                                 ({ "type": "api_key", "key": "..." },
                                  "!command", "$ENV_VAR", or OAuth)
~/.pi/agent/models.json        — custom provider configs & model overrides
~/.pi/agent/models-store.json  — pi's cached dynamic model catalog
```

Plus Locagens UI metadata under the app-support dir:

```txt
locagens-providers-meta.json   — display names / model settings (pricing, context limits)
```

Secrets never enter the app data files above.

---

## Notes

- All timestamp fields use ISO-8601 text format.
- There is no SQLite, no WAL/journal mode, no `busy_timeout`, no `DbWriterClient`
  background writer, and no `apps/db-writer` sidecar — writes are serialized by
  the in-process `RwLock` and persisted synchronously.
- Startup cleanup resets runs stuck in `'created'`, `'generating'`, or
  `'awaiting_permission'` to `'failed'` with an interrupt error message.
- Legacy `locagens.db` / `locagens.db-shm` / `locagens.db-wal` files are no
  longer read or written and can be deleted.
