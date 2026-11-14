# Storage — Locagens

**Storage engine:** Codex-style local files — one append-only JSONL *rollout* per session (date-sharded), small append-only index files, Markdown memories. No SQLite, no C library dependency.
**Implementation:** `crates/core/src/db.rs` (in-memory `RwLock<StorageState>` mirror + durable file persistence). The layout mirrors `codex-rs/rollout` (`sessions/YYYY/MM/DD/rollout-…jsonl`, `session_index.jsonl`) and `codex-rs/message-history` (`history.jsonl`).
**Root:** the Locagens home, `~/.locagens` (`$LOCAGENS_HOME`; helper `locagens_home()` in `crates/config/src/paths.rs`). `LOCAGENS_DATA_DIR` (or the legacy `LOCAGENS_DB_PATH` parent/stem) overrides the whole root — used by tests. Resolved by `db::resolve_data_root()`.

---

## File layout

```txt
~/.locagens/
  sessions/YYYY/MM/DD/rollout-YYYY-MM-DDThh-mm-ss-<run_id>.jsonl   one file per run
  session_index.jsonl     append-only {id, title, updated_at}; last line per id wins
  history.jsonl           append-only {session_id, ts, text}; one line per user prompt
  memories/<slug>.md      global memories, one fact per file
  projects/<sanitized-project-path>/memories/<slug>.md   project-scoped memories
  projects.json           list of Project            (atomic tmp + rename)
  permissions.json        list of PermissionRule     (atomic tmp + rename)
  .usage_seq              last allocated usage id (cross-process counter)
  .lock                   advisory lock for the shared files above
  run_locks/<run_id>.lock owner lock of an active run, held by the process driving it
  projects/<sanitized-project-path>/snapshots/   git-snapshot store (snapshots.json + <id>.index copies)
  .migrated-from-data-v1  marker: legacy data already imported
```

Provider credentials (`auth.json`, `models.json`) live in the same home but are
owned by `locagens-config`; no secret ever enters the files above.

### Rollout files

The file-name timestamp is the run's `createdAt` (UTC) and `<run_id>` is the run id
(characters other than `[A-Za-z0-9_-]` become `_`). Every line is

```json
{"timestamp": "2026-03-07T10:20:30.123Z", "type": "<kind>", "payload": { }}
```

| `type` | payload |
|---|---|
| `session_meta` | first line: the `Run` (plus `forked_from`, `forked_up_to_message_id` when forked) |
| `run_update` | full `Run` snapshot (status, title, error, any patch) |
| `message` | a `RunMessage` |
| `message_update` | replacement `RunMessage` for an existing `id` (a streaming message gets one checkpoint every 5 s plus the final one) |
| `plan` | full `Plan` snapshot |
| `usage` | a `UsageLogEntry` (the id comes from `.usage_seq`, under the lock only while it is allocated) |

Rollouts are strictly **append-only**: an in-place update is an appended line, and
loading replays the file so the last write wins. A truncated or corrupt line
(e.g. a crash mid-write) is skipped; the next append first terminates it with a
newline so it never corrupts a good line. Deleting a run removes its rollout file
(and now-empty date directories) and tombstones the id for the life of the process, so
a drive task that is still unwinding cannot write a headless rollout back;
`api::runs::delete_run` cancels an active run first. `session_index.jsonl` and
`history.jsonl` are never rewritten, and are not written at all for ephemeral
runs (`lcgn --no-session`, `Run.ephemeral`), which are also removed at startup if
a crash left them behind. `fork_run` writes a new rollout holding a fresh `session_meta`
(`forked_from`), the copied messages (new ids) and the plan; usage is not copied.

All state is mirrored in memory (O(1) reads). While a message streams, the
in-memory copy is updated on every chunk but the rollout only receives a
`message_update` checkpoint every 5 s and the final text, so a long answer costs
a handful of lines instead of one per 250 ms; a crash loses at most the last
seconds of text of a run that is failed on restart anyway.

`refresh_runs()` re-reads the rollouts whose mtime/size changed (and drops runs
whose file is gone) so runs written by another process become visible; runs
this process is driving are never overwritten. `api::runs::list_runs` calls it
at most every 2 s. `permissions.json`, `projects.json` and the memory files are
re-read when their mtime changes, so a grant revoked in one process stops
applying in the other.

An active run (`created`, `generating`, `awaiting_permission`, `awaiting_input`)
is *owned* by the process driving it: it holds an exclusive lock on
`run_locks/<run_id>.lock` until the run reaches a terminal status (the OS frees
it if the process dies). Startup cleanup appends a `run_update` (status
`failed`) and a system `message` only to active runs whose lock is not held, so
starting `lcgn` while the desktop drives a run, or the reverse, leaves that run alone.

Storage errors do not panic the app: if `.lock` cannot be opened or taken within
10 s the mutation proceeds without the cross-process lock (with a warning), and
a poisoned in-memory lock is recovered rather than propagated.

### Memories

One Markdown file per fact: a YAML frontmatter block, then the fact verbatim.
String values are written JSON-quoted (valid YAML), so quotes, colons, newlines
and `---` inside a value need no further escaping; the body is everything after
the closing `---` line.

```md
---
id: 3
category: "reference"
scope: "project"
project_path: "C:/ws"
created_at: "2026-03-07T10:20:30.123Z"
updated_at: "2026-03-07T10:20:30.123Z"
---
Use cargo nextest for the workspace tests.
```

File name: `<first words of the fact>-<id>.md`; it is kept when the memory is
edited. `project_path` is only written for project memories (the directory name
is the path with non-alphanumeric characters replaced by `-`).

### Legacy import (one-time)

When the root is the default Locagens home and `<app_support_dir>/data/` exists
(the previous `projects.json` / `permissions.json` / `memory.json` /
`usage_logs.jsonl` / `sessions/<id>/{run.json,messages.jsonl,plan.json}` layout)
and `.migrated-from-data-v1` is absent, `Db::open` imports it: sessions become
rollouts (with their plan and matching usage lines), user prompts feed
`history.jsonl`, `session_index.jsonl` is seeded, `memory.json` becomes `.md`
files (ids kept), and `projects.json` / `permissions.json` are copied when not
already present. The marker is written last; the legacy directory is only read,
never modified or deleted. Usage lines whose run no longer exists are dropped.

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
| `tool` | `string` | Tool name (e.g. `run_command`, `write_file`, `search_files`; `network_access` for hosts approved at a sandbox network prompt, see `docs/SANDBOX.md`) |
| `command` | `string` | Exact command string (for `run_command`); empty for other tools |
| `networkDomains` | `string[]` | Sorted list of exact network domains approved with the command |
| `status` | `string` | e.g. `'approved'`, `'denied'` |

Duplicate exact command/domain grants are prevented on write.

### `memories/<slug>.md` — `Memory`

Durable facts the assistant remembers across sessions (set via the `remember`
tool / Settings → Memory). Frontmatter keys are the snake_case names below.

| Field | Type | Notes |
|---|---|---|
| `id` | `integer` | Auto-increment surrogate key (frontmatter `id`) |
| `scope` | `string` | `'global'` or `'project'` |
| `projectPath` | `string` | Empty string = global scope; project path otherwise |
| `category` | `string` | Category label (default `project`) |
| `content` | `string` | The remembered fact (the Markdown body, stored verbatim) |
| `createdAt` / `updatedAt` | `string` | ISO-8601 timestamps |

### `usage` lines — `UsageLogEntry`

Detailed records of all LLM completions (token counts, cache hits, cost),
appended to the owning run's rollout. `id` is unique across processes (counter
file `.usage_seq`); `list_usage` aggregates the `usage` lines of all rollouts.

| Field | Type | Notes |
|---|---|---|
| `id` | `integer` | Auto-increment surrogate key |
| `runId` | `string` | Parent run (the rollout the line lives in) |
| `agentRole` | `string?` | Agent role: `main` for the main agent, `explorer`, `worker`, or `title` for the first-turn chat-title call (legacy rows with `planner` are read as `main`) |
| `providerId` | `string` | Provider identifier |
| `model` | `string` | Model name used for completion |
| `inputTokens` / `outputTokens` | `integer` | Prompt / completion token counts |
| `cacheReadTokens` / `cacheWriteTokens` | `integer` | Cache hit / miss-write token counts |
| `cacheHitRate` | `number` | Cache hit rate as percentage (0–100) |
| `cost` | `number` | Estimated cost of this call in USD |
| `createdAt` | `string` | ISO-8601 timestamp |

### `session_meta` / `run_update` — `Run`

The run snapshot: first line of a rollout (`session_meta`); every later change
appends a full-snapshot `run_update`.

| Field | Type | Notes |
|---|---|---|
| `id` | `string` | Run id (also embedded in the rollout file name) |
| `title` | `string` | Human-readable run title |
| `task` | `string` | The user's task description |
| `projectPath` / `projectName` | `string` | Active workspace project (path defaults to the workspace root) |
| `status` | `string` | `created`, `generating`, `awaiting_permission`, `awaiting_input`, `done`, `failed`, `cancelled` |
| `providerId` / `providerDisplayName` | `string` | Selected provider (resolved from the agent config dir chain) |
| `model` | `string` | Model name |
| `mode` | `string` | Run mode (`accept_edits`, `plan`, `auto`, `chat`, `ask_permissions`) |
| `errorMessage` | `string?` | Error text if the run failed |
| `createdAt` / `updatedAt` | `string` | ISO-8601 timestamps |
| `lastActiveAt` | `string?` | Last activity |
| `reasoningEffort` | `string?` | Per-model reasoning-effort selection |
| `systemPrompt` / `appendSystemPrompt` | `string?` | Custom system prompt (`lcgn --system-prompt` / `--append-system-prompt`); omitted when unset |
| `forked_from` | `string?` | `session_meta` only: source run id of a fork |

Legacy data may still contain `coder*`/`utility*`/`agentPreset` keys from before
preset removal; they deserialize cleanly (unknown keys are ignored) but are no
longer read or written.

Derived view: `db::Db::get_run()` projects this into a `RunRow` with only the
columns consumed by route modules.

### `message` / `message_update` — `RunMessage`

Chat messages within a run. `message` appends a new one; `message_update`
carries the replacement message for an existing `id` (last write wins).

| Field | Type | Notes |
|---|---|---|
| `id` | `string` | Unique message identifier |
| `runId` | `string` | Parent run |
| `role` | `string` | `user`, `assistant`, `system`, `tool`, … |
| `agentRole` | `string?` | Sub-role: `explorer`, `worker`, or `null` for the main agent |
| `agentName` | `string?` | Sub-agent identity (display name) |
| `providerId` / `providerDisplayName` / `model` | `string?` | Provider/model used for this message |
| `content` | `string` | Message body text |
| `reasoningContent` | `string?` | Model's chain-of-thought / reasoning text |
| `rawResponse` | `string?` | Serialized tool-call payload / raw response |
| `createdAt` | `string` | ISO-8601 timestamp |

### `plan` — `Plan`

The active plan document for a run (drives the right-hand plan panel). Each
`plan` line is a full snapshot; the last one wins.

| Field | Type | Notes |
|---|---|---|
| `id` | `string` | Unique plan identifier |
| `runId` | `string` | Parent run |
| `title` | `string` | Plan title (default `Plan`) |
| `body` | `string?` | Plan body / description (JSON or Markdown) |
| `tasks` | `array` | JSON array of task objects (default `[]`) |
| `status` | `string` | `'active'`, `'superseded'`, … (default `active`) |
| `version` | `integer` | Monotonically increasing version per run (default `1`) |
| `createdAt` / `updatedAt` | `string` | ISO-8601 timestamps |

### `session_index.jsonl` and `history.jsonl`

```json
{"id": "run-…", "title": "Fix the build", "updated_at": "2026-03-07T10:25:00.000Z"}
{"session_id": "run-…", "ts": 1772879430, "text": "the user's prompt"}
```

The index gets a line when a run is created, its title changes, or its status
changes (last line per `id` wins; deleted runs are not removed). `history.jsonl`
gets one line per main-agent user prompt (`ts` = unix seconds).

---

## Relationship Summary

```
sessions/…/rollout-…-<run_id>.jsonl ── session_meta, run_update*, message*, message_update*, plan*, usage*
session_index.jsonl / history.jsonl ── reference runs by id
```

- **`projects.json`** is a standalone lookup file.
- **`permissions.json`** uses `scope` + `projectPath` (not a foreign key) to
  associate grants with projects.
- **Memories** use `scope` + `projectPath` (not a foreign key) to associate facts
  with projects.

---

## Provider & auth configuration (not app data)

Provider credentials and model catalogs live in the agent config dir chain
(`~/.locagens` → `~/.codex` → `~/.pi/agent`, see `agent_dir_chain()` in
`crates/config/src/paths.rs`) and are managed by `crates/config/src/providers.rs`:

```txt
auth.json          — provider id → credential
                     ({ "type": "api_key", "key": "..." },
                      "!command", "$ENV_VAR", or OAuth)
models.json        — custom provider configs & model overrides
models-live.json   — provider id → { "fetchedAtMs", "models": [ids] }: the
                     last `GET /models` list (re-fetched after 6 h); once
                     present it decides which models the provider offers
```

Desktop preferences live in the app's `settings.json` (appearance,
`hiddenModels` — `provider/model` ids switched off in Settings → Providers,
also honoured by the TUI — and the `layout` column widths).

Secrets never enter the storage files above.

---

## Notes

- All timestamp fields use ISO-8601 text (`history.jsonl` uses unix seconds, like Codex).
- There is no SQLite and no background writer; every write is synchronous under
  the in-process `RwLock` (plus the advisory `.lock` for the shared files).
- Streaming assistant output is persisted as periodic `message_update`
  snapshots, so a long reply leaves several superseded lines in its rollout.
- Legacy `locagens.db` files and the old `<app_support_dir>/data/` directory are
  no longer written and can be deleted once imported.
