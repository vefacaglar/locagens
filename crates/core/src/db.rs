//! Codex-style local storage: one append-only JSONL "rollout" per session,
//! date-sharded, plus small append-only index files and Markdown memories.
//! No SQLite, no C library dependency.
//!
//! Layout under the Locagens home (`~/.locagens`, `$LOCAGENS_HOME`; the
//! `LOCAGENS_DATA_DIR` env var overrides the whole root, mainly for tests):
//!
//! ```txt
//! sessions/YYYY/MM/DD/rollout-YYYY-MM-DDThh-mm-ss-<run_id>.jsonl   one file per run
//! session_index.jsonl   append-only {id, title, updated_at}; last line per id wins
//! history.jsonl         append-only {session_id, ts, text}; one line per user prompt
//! memories/<slug>.md    global memories (YAML frontmatter + body)
//! projects/<sanitized-project-path>/memories/<slug>.md   project memories
//! projects.json, permissions.json   JSON, written atomically (tmp + rename)
//! ```
//!
//! A rollout line is `{"timestamp", "type", "payload"}` with `type` one of
//! `session_meta`, `run_update`, `message`, `message_update`, `plan`, `usage`.
//! Rollouts are strictly append-only: an update is a newly appended line and
//! loading replays the file (last write wins). A truncated or corrupt line is
//! skipped. Everything is mirrored in an in-memory `RwLock<StorageState>` so
//! reads stay O(1).
//!
//! A legacy `<app_support>/Locagens/data/` directory (run.json / messages.jsonl
//! / memory.json layout) is imported once, read-only, on first open.

use crate::util::now_iso;
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant, SystemTime};

const MIGRATION_MARKER: &str = ".migrated-from-data-v1";
const SESSION_INDEX_FILE: &str = "session_index.jsonl";
const HISTORY_FILE: &str = "history.jsonl";
const USAGE_SEQ_FILE: &str = ".usage_seq";

/// How long a mutation waits for the cross-process lock before degrading to
/// "no lock" (a warning is logged; the write still happens).
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);
/// Streamed message text is checkpointed to the rollout at most this often.
const STREAM_CHECKPOINT_INTERVAL: Duration = Duration::from_secs(5);
/// `refresh_runs_throttled` re-scans the rollouts at most this often.
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);
/// Directory (under the root) holding one owner lock file per active run.
const RUN_LOCKS_DIR: &str = "run_locks";

/// Held for the duration of one cross-process mutation of a shared file.
/// Backed by an advisory exclusive lock on `<root>/.lock` so the desktop app
/// and `lcgn` can run concurrently on the same root without overwriting each
/// other's `projects.json` / `permissions.json` / memory files / usage ids.
/// Rollouts are not locked (they are private to the run that owns them and
/// only ever appended to). `None` means the lock could not be taken (a
/// read-only or unreachable root); the caller proceeds without it.
struct CrossProcessLock(Option<std::fs::File>);

impl Drop for CrossProcessLock {
    fn drop(&mut self) {
        if let Some(file) = &self.0 {
            let _ = file.unlock();
        }
    }
}

/// Takes the advisory lock, retrying up to [`LOCK_TIMEOUT`]. Never panics:
/// when the lock file cannot be opened or locked the store degrades to
/// running unlocked with a warning.
fn lock_root(root: &Path) -> CrossProcessLock {
    let path = root.join(".lock");
    let file = match OpenOptions::new().create(true).truncate(false).write(true).open(&path) {
        Ok(file) => file,
        Err(err) => {
            tracing::warn!("[Database] Cannot open lock file {}: {err}; continuing without it", path.display());
            return CrossProcessLock(None);
        }
    };
    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return CrossProcessLock(Some(file)),
            Err(err) if is_lock_contended(&err) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(err) => {
                tracing::warn!("[Database] Advisory lock not taken on {}: {err}", path.display());
                return CrossProcessLock(None);
            }
        }
    }
}

fn is_lock_contended(err: &std::io::Error) -> bool {
    err.kind() == fs2::lock_contended_error().kind()
        || err.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

/// Owner lock of one active run: `<root>/run_locks/<run_id>.lock`.
fn run_lock_path(root: &Path, run_id: &str) -> PathBuf {
    root.join(RUN_LOCKS_DIR).join(format!("{}.lock", safe_file_part(run_id)))
}

/// Takes the exclusive owner lock of a run, held until released or the
/// process exits (the OS frees it on a crash, which is what makes it a
/// reliable "is the owner alive" probe).
fn acquire_run_lock(root: &Path, run_id: &str) -> Option<std::fs::File> {
    let path = run_lock_path(root, run_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let file = OpenOptions::new().create(true).truncate(false).write(true).open(&path).ok()?;
    file.try_lock_exclusive().ok()?;
    Some(file)
}

/// True when another live process (or `Db` instance) holds the run's owner lock.
fn run_owned_by_live_process(root: &Path, run_id: &str) -> bool {
    let path = run_lock_path(root, run_id);
    let Ok(file) = OpenOptions::new().write(true).open(&path) else {
        return false;
    };
    match file.try_lock_exclusive() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(err) => is_lock_contended(&err),
    }
}

/// `(mtime, length)` of a file; `None` when it does not exist.
type FileSig = Option<(SystemTime, u64)>;

fn file_sig(path: &Path) -> FileSig {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// A durable fact remembered via the `remember` tool / Settings → Memory.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Memory {
    pub id: i64,
    pub scope: String,
    pub project_path: String,
    pub category: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A persisted standing approval for workspace tool calls.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct PermissionRule {
    pub id: i64,
    pub scope: String,
    pub project_path: String,
    pub tool: String,
    pub command: String,
    pub network_domains: Vec<String>,
    pub status: String,
}

/// A registered project folder.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Project {
    pub path: String,
    pub name: String,
    pub created_at: String,
}

/// The columns of `runs` consumed by existing route modules (projected view).
#[derive(Clone, Debug, Default)]
pub struct RunRow {
    pub id: String,
    pub title: String,
    pub project_path: Option<String>,
    pub project_name: String,
    pub provider_id: String,
    pub provider_display_name: String,
    pub model: String,
}

/// Full run metadata, persisted as the `session_meta` / `run_update` lines of its rollout.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Run {
    pub id: String,
    pub title: String,
    pub task: String,
    pub project_path: String,
    pub project_name: String,
    pub status: String,
    pub provider_id: String,
    pub provider_display_name: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub mode: String,
    /// `lcgn --no-session`: never written to `session_index.jsonl` /
    /// `history.jsonl`, and removed again at startup if it was left behind.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ephemeral: bool,
    /// Custom system prompt (`--system-prompt` in `lcgn`); replaces the base.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Extra system-prompt text (`--append-system-prompt` in `lcgn`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,
    pub error_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_active_at: Option<String>,
}

/// One chat message of a run, persisted as a `message` / `message_update` rollout line
///
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RunMessage {
    pub id: String,
    pub run_id: String,
    pub role: String,
    pub agent_role: Option<String>,
    pub agent_name: Option<String>,
    pub provider_id: Option<String>,
    pub provider_display_name: Option<String>,
    pub model: Option<String>,
    pub content: String,
    pub reasoning_content: Option<String>,
    pub raw_response: Option<String>,
    pub created_at: String,
}

/// The active plan of a run, persisted as `plan` rollout lines.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Plan {
    pub id: String,
    pub run_id: String,
    pub title: String,
    pub body: Option<String>,
    pub tasks: Value,
    pub status: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// One usage entry, persisted as a `usage` rollout line.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageLogEntry {
    pub id: i64,
    pub run_id: String,
    pub agent_role: Option<String>,
    pub provider_id: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cache_hit_rate: f64,
    pub cost: f64,
    pub created_at: String,
    pub duration_ms: Option<i64>,
}

/// One line of a rollout file: `{"timestamp", "type", "payload"}`.
#[derive(Deserialize)]
struct RolloutLine {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    payload: Value,
}

/// Everything the store holds, mirrored in memory for O(1) reads.
#[derive(Default)]
struct StorageState {
    projects: Vec<Project>,
    permissions: Vec<PermissionRule>,
    memories: Vec<Memory>,
    /// Markdown file of each memory, by memory id.
    memory_files: HashMap<i64, PathBuf>,
    runs: BTreeMap<String, Run>,
    messages: HashMap<String, Vec<RunMessage>>,
    plans: HashMap<String, Plan>,
    usage: Vec<UsageLogEntry>,
    /// Rollout file of each run (known once the run has any line on disk).
    rollouts: HashMap<String, PathBuf>,
    next_permission_id: i64,
    next_memory_id: i64,
    next_usage_id: i64,
    /// Ids of deleted runs: late writes from a still-unwinding drive task
    /// must never recreate their rollout (QA-034).
    deleted_runs: HashSet<String>,
    /// Owner lock files of the runs this process drives (active runs only).
    run_claims: HashMap<String, std::fs::File>,
    /// Last time a streamed message was checkpointed to its rollout.
    stream_checkpoints: HashMap<String, Instant>,
    /// Signatures of the shared files as last read, so changes made by
    /// another process are noticed (QA-039).
    projects_sig: FileSig,
    permissions_sig: FileSig,
    memories_sig: u64,
    /// Signature of every rollout as last replayed.
    rollout_sigs: HashMap<PathBuf, FileSig>,
}

pub struct Db {
    root: PathBuf,
    state: RwLock<StorageState>,
    last_refresh: Mutex<Option<Instant>>,
}

const ACTIVE_RUN_STATUSES: [&str; 4] =
    ["created", "generating", "awaiting_permission", "awaiting_input"];

impl Db {
    /// Opens (creating if needed) the store rooted at `data_root`. When the
    /// root is the default Locagens home, the legacy `<app_support>/data`
    /// directory is imported once (see `migrate_legacy_data`).
    pub fn open(data_root: &Path) -> anyhow::Result<Self> {
        let legacy = default_legacy_dir_for(data_root);
        Self::open_with_legacy(data_root, legacy.as_deref())
    }

    fn open_with_legacy(data_root: &Path, legacy: Option<&Path>) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_root)?;
        if let Some(legacy) = legacy {
            if let Err(err) = migrate_legacy_data(legacy, data_root) {
                tracing::warn!("[Database] Legacy import from {} failed: {err}", legacy.display());
            }
        }
        let state = load_state(data_root)?;
        Ok(Self {
            root: data_root.to_path_buf(),
            state: RwLock::new(state),
            last_refresh: Mutex::new(None),
        })
    }

    /// The store root directory (rollouts, indexes and memories live here).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Takes the advisory cross-process lock (`<root>/.lock`, blocking).
    fn cross_process_lock(&self) -> CrossProcessLock {
        lock_root(&self.root)
    }

    /// Runs `mutate` under the cross-process lock after re-reading the
    /// shared files from disk, so mutations that raced with another process
    /// are applied on top of the latest on-disk state instead of clobbering
    /// it. The closure is expected to persist the files it changed.
    fn with_shared_mutation<T>(&self, mutate: impl FnOnce(&mut StorageState) -> T) -> T {
        let _lock = self.cross_process_lock();
        let mut state = self.write();
        reload_shared_files(&self.root, &mut state);
        let result = mutate(&mut state);
        // Our own writes must not look like another process's changes.
        state.projects_sig = file_sig(&self.root.join("projects.json"));
        state.permissions_sig = file_sig(&self.root.join("permissions.json"));
        state.memories_sig = memories_signature(&self.root);
        result
    }

    /// Seeds the default project and fails any run left active by a restart.
    pub fn run_startup_tasks(&self, ws_root: &Path, default_project_name: &str) {
        self.with_shared_mutation(|state| {
            let root = self.root.clone();

            // Seed default project if projects.json is empty.
            if state.projects.is_empty() {
                let project = Project {
                    path: ws_root.to_string_lossy().to_string(),
                    name: default_project_name.to_string(),
                    created_at: now_iso(),
                };
                state.projects.push(project);
                persist_projects(&self.root, &state.projects);
            }

            // Fail runs left in active states by a process that is gone. A
            // run another live process (an `lcgn`, the desktop) is driving
            // holds its owner lock and is left alone (QA-037).
            let stuck: Vec<String> = state
                .runs
                .values()
                .filter(|run| ACTIVE_RUN_STATUSES.contains(&run.status.as_str()))
                .filter(|run| !run_owned_by_live_process(&root, &run.id))
                .map(|run| run.id.clone())
                .collect();

            for run_id in stuck {
                // A `--no-session` run that was never cleaned up leaves no trace.
                if state.runs.get(&run_id).is_some_and(|run| run.ephemeral) {
                    remove_run(&root, state, &run_id);
                    continue;
                }
                let now = now_iso();
                let snapshot = state.runs.get_mut(&run_id).map(|run| {
                    run.status = "failed".to_string();
                    run.error_message =
                        Some("Session interrupted due to server restart.".to_string());
                    run.updated_at = now.clone();
                    run.clone()
                });
                if let Some(run) = snapshot {
                    append_event(&root, state, &run_id, &now, "run_update", run_payload(&run));
                    append_session_index(&root, &run);
                }
                let message = RunMessage {
                    id: format!(
                        "msg-err-restart-{}-{}",
                        run_id,
                        chrono::Utc::now().timestamp_millis()
                    ),
                    run_id: run_id.clone(),
                    role: "system".to_string(),
                    content: "Session interrupted due to server restart.".to_string(),
                    created_at: now.clone(),
                    ..Default::default()
                };
                state
                    .messages
                    .entry(run_id.clone())
                    .or_default()
                    .push(message.clone());
                append_event(&root, state, &run_id, &now, "message", message_payload(&message));
            }
        })
    }

    // ------------------------------------------------------------------
    // Projects
    // ------------------------------------------------------------------

    pub fn list_projects(&self) -> Vec<Project> {
        self.sync_shared_files(true, false, false);
        self.read().projects.clone()
    }

    pub fn get_project(&self, path: &str) -> Option<Project> {
        self.sync_shared_files(true, false, false);
        self.read()
            .projects
            .iter()
            .find(|project| project.path == path)
            .cloned()
    }

    pub fn create_project(&self, path: &str, name: &str) -> Project {
        self.with_shared_mutation(|state| {
            let mut updated: Option<Project> = None;
            for existing in state.projects.iter_mut() {
                if existing.path == path {
                    existing.name = name.to_string();
                    existing.created_at = now_iso();
                    updated = Some(existing.clone());
                    break;
                }
            }
            if let Some(existing) = updated {
                persist_projects(&self.root, &state.projects);
                return existing;
            }
            let project = Project {
                path: path.to_string(),
                name: name.to_string(),
                created_at: now_iso(),
            };
            state.projects.push(project.clone());
            persist_projects(&self.root, &state.projects);
            project
        })
    }

    pub fn delete_project(&self, path: &str) {
        self.with_shared_mutation(|state| {
            let before = state.projects.len();
            state.projects.retain(|project| project.path != path);
            if state.projects.len() != before {
                persist_projects(&self.root, &state.projects);
            }
        })
    }

    // ------------------------------------------------------------------
    // Memories (one Markdown file per fact)
    // ------------------------------------------------------------------

    pub fn list_memories(&self) -> Vec<Memory> {
        self.sync_shared_files(false, false, true);
        let mut list = self.read().memories.clone();
        list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        list
    }

    pub fn create_memory(
        &self,
        scope: &str,
        project_path: &str,
        category: &str,
        content: &str,
    ) -> anyhow::Result<Memory> {
        self.with_shared_mutation(|state| {
            let now = now_iso();
            let memory = Memory {
                id: state.next_memory_id,
                scope: scope.to_string(),
                project_path: project_path.to_string(),
                category: category.to_string(),
                content: content.to_string(),
                created_at: now.clone(),
                updated_at: now,
            };
            let path = memory_file_path(&self.root, &memory);
            write_text_atomic(&path, &render_memory(&memory))?;
            state.next_memory_id += 1;
            state.memory_files.insert(memory.id, path);
            state.memories.push(memory.clone());
            Ok(memory)
        })
    }

    pub fn update_memory(&self, id: i64, content: &str) -> Option<Memory> {
        self.with_shared_mutation(|state| {
            let memory = state.memories.iter_mut().find(|memory| memory.id == id)?;
            memory.content = content.to_string();
            memory.updated_at = now_iso();
            let updated = memory.clone();
            let path = state
                .memory_files
                .entry(id)
                .or_insert_with(|| memory_file_path(&self.root, &updated))
                .clone();
            if let Err(err) = write_text_atomic(&path, &render_memory(&updated)) {
                tracing::error!("[Database] Failed to write {}: {err}", path.display());
            }
            Some(updated)
        })
    }

    pub fn delete_memory(&self, id: i64) -> bool {
        self.with_shared_mutation(|state| {
            let before = state.memories.len();
            state.memories.retain(|memory| memory.id != id);
            if let Some(path) = state.memory_files.remove(&id) {
                let _ = std::fs::remove_file(path);
            }
            state.memories.len() != before
        })
    }

    pub fn clear_memories(&self) {
        self.with_shared_mutation(|state| {
            for (_, path) in state.memory_files.drain() {
                let _ = std::fs::remove_file(path);
            }
            state.memories.clear();
        })
    }

    // ------------------------------------------------------------------
    // Permissions
    // ------------------------------------------------------------------

    pub fn list_permissions(&self) -> Vec<PermissionRule> {
        self.sync_shared_files(false, true, false);
        let mut list = self.read().permissions.clone();
        list.sort_by_key(|rule| rule.id);
        list
    }

    /// Inserts a standing approval, deduplicating on
    /// `(scope, project_path, tool, command, network_domains)`.
    pub fn upsert_permission(
        &self,
        scope: &str,
        project_path: &str,
        tool: &str,
        command: &str,
        network_domains: Vec<String>,
        status: &str,
    ) -> PermissionRule {
        self.with_shared_mutation(|state| {
            let mut sorted_domains = network_domains.clone();
            sorted_domains.sort();
            let mut updated: Option<PermissionRule> = None;
            for existing in state.permissions.iter_mut() {
                let mut domains = existing.network_domains.clone();
                domains.sort();
                if existing.scope == scope
                    && existing.project_path == project_path
                    && existing.tool == tool
                    && existing.command == command
                    && domains == sorted_domains
                {
                    existing.status = status.to_string();
                    updated = Some(existing.clone());
                    break;
                }
            }
            if let Some(existing) = updated {
                persist_permissions(&self.root, &state.permissions);
                return existing;
            }
            let rule = PermissionRule {
                id: state.next_permission_id,
                scope: scope.to_string(),
                project_path: project_path.to_string(),
                tool: tool.to_string(),
                command: command.to_string(),
                network_domains,
                status: status.to_string(),
            };
            state.next_permission_id += 1;
            state.permissions.push(rule.clone());
            persist_permissions(&self.root, &state.permissions);
            rule
        })
    }

    pub fn delete_permission(&self, id: i64) -> bool {
        self.with_shared_mutation(|state| {
            let before = state.permissions.len();
            state.permissions.retain(|rule| rule.id != id);
            if state.permissions.len() != before {
                persist_permissions(&self.root, &state.permissions);
                true
            } else {
                false
            }
        })
    }

    pub fn clear_permissions(&self) {
        self.with_shared_mutation(|state| {
            if !state.permissions.is_empty() {
                state.permissions.clear();
                persist_permissions(&self.root, &state.permissions);
            }
        })
    }

    // ------------------------------------------------------------------
    // Runs
    // ------------------------------------------------------------------

    /// Starts the run's rollout with a `session_meta` line (or appends a
    /// `run_update` when the run id is already known). An active run is
    /// claimed by this process (see [`Db::run_startup_tasks`]).
    pub fn create_run(&self, run: Run) -> Run {
        let mut guard = self.write();
        let state = &mut *guard;
        state.deleted_runs.remove(&run.id);
        let kind = if state.runs.contains_key(&run.id) {
            "run_update"
        } else {
            "session_meta"
        };
        state.runs.insert(run.id.clone(), run.clone());
        append_event(&self.root, state, &run.id, &run.created_at, kind, run_payload(&run));
        append_session_index(&self.root, &run);
        sync_run_claim(&self.root, state, &run);
        run
    }

    pub fn update_run_status(&self, id: &str, status: &str, error_message: Option<&str>) {
        let mut guard = self.write();
        let state = &mut *guard;
        let snapshot = state.runs.get_mut(id).map(|run| {
            run.status = status.to_string();
            run.error_message = error_message.map(str::to_string);
            run.updated_at = now_iso();
            run.last_active_at = Some(now_iso());
            run.clone()
        });
        if let Some(run) = snapshot {
            append_event(&self.root, state, id, &run.created_at, "run_update", run_payload(&run));
            append_session_index(&self.root, &run);
            sync_run_claim(&self.root, state, &run);
        }
    }

    /// Applies an arbitrary metadata patch to a run (title, model fields,
    /// status, …) and appends the new snapshot as a `run_update` line.
    pub fn patch_run(&self, id: &str, patch: impl FnOnce(&mut Run)) -> Option<Run> {
        let mut guard = self.write();
        let state = &mut *guard;
        let updated = state.runs.get_mut(id).map(|run| {
            let old_title = run.title.clone();
            patch(run);
            run.updated_at = now_iso();
            run.last_active_at = Some(now_iso());
            (run.clone(), old_title != run.title)
        });
        let (run, title_changed) = updated?;
        append_event(&self.root, state, id, &run.created_at, "run_update", run_payload(&run));
        if title_changed {
            append_session_index(&self.root, &run);
        }
        sync_run_claim(&self.root, state, &run);
        Some(run)
    }

    pub fn touch_run(&self, id: &str) {
        let mut guard = self.write();
        let state = &mut *guard;
        let snapshot = state.runs.get_mut(id).map(|run| {
            run.updated_at = now_iso();
            run.last_active_at = Some(now_iso());
            run.clone()
        });
        if let Some(run) = snapshot {
            append_event(&self.root, state, id, &run.created_at, "run_update", run_payload(&run));
        }
    }

    pub fn list_runs(&self) -> Vec<Run> {
        self.read().runs.values().cloned().collect()
    }

    pub fn get_run_full(&self, id: &str) -> Option<Run> {
        self.read().runs.get(id).cloned()
    }

    /// Projected view kept for Phase 1 route compatibility.
    pub fn get_run(&self, id: &str) -> Option<RunRow> {
        self.get_run_full(id).map(|run| RunRow {
            id: run.id,
            title: run.title,
            project_path: Some(run.project_path).filter(|p| !p.is_empty()),
            project_name: run.project_name,
            provider_id: run.provider_id,
            provider_display_name: run.provider_display_name,
            model: run.model,
        })
    }

    /// Forks a run: writes a new rollout holding the source's metadata
    /// (`session_meta` with `forked_from`), messages (up to and including
    /// `up_to_message_id`; all of them when `None`) and plan under fresh ids,
    /// titled `<title> (fork)`. Returns `None` when the source run or the
    /// fork-point message does not exist.
    pub fn fork_run(&self, source_run_id: &str, up_to_message_id: Option<&str>) -> Option<Run> {
        let source = self.get_run_full(source_run_id)?;
        let messages = self.list_messages(source_run_id);
        let mut copied: Vec<RunMessage> = Vec::new();
        for message in &messages {
            copied.push(message.clone());
            if let Some(mid) = up_to_message_id {
                if message.id == mid {
                    break;
                }
            }
        }
        if let Some(mid) = up_to_message_id {
            if !copied.iter().any(|message| message.id == mid) {
                return None;
            }
        }

        let new_id = format!("run-{}", uuid::Uuid::new_v4());
        let now = now_iso();
        let mut fork = source.clone();
        fork.id = new_id.clone();
        fork.title = if source.title.trim().is_empty() {
            "Fork".to_string()
        } else {
            format!("{} (fork)", source.title)
        };
        fork.status = "done".to_string();
        fork.error_message = None;
        fork.ephemeral = false;
        fork.created_at = now.clone();
        fork.updated_at = now.clone();
        fork.last_active_at = Some(now.clone());

        let mut meta = run_payload(&fork);
        meta["forked_from"] = json!(source_run_id);
        if let Some(mid) = up_to_message_id {
            meta["forked_up_to_message_id"] = json!(mid);
        }
        let mut buffer = rollout_line(&now, "session_meta", &meta);
        for message in copied.iter_mut() {
            message.id = format!("msg-{}", uuid::Uuid::new_v4());
            message.run_id = new_id.clone();
            buffer.push_str(&rollout_line(&now, "message", &message_payload(message)));
        }
        let plan = self.get_plan(source_run_id).map(|mut plan| {
            plan.run_id = new_id.clone();
            plan
        });
        if let Some(plan) = plan.as_ref() {
            buffer.push_str(&rollout_line(&now, "plan", &plan_payload(plan)));
        }

        let path = new_rollout_path(&self.root, &new_id, &fork.created_at);
        append_text(&path, &buffer);
        let mut guard = self.write();
        let state = &mut *guard;
        state.rollouts.insert(new_id.clone(), path);
        state.runs.insert(new_id.clone(), fork.clone());
        state.messages.insert(new_id.clone(), copied);
        if let Some(plan) = plan {
            state.plans.insert(new_id, plan);
        }
        append_session_index(&self.root, &fork);
        Some(fork)
    }

    /// Deletes the run's rollout file (usage lines live in it and go too).
    /// The id is tombstoned, so a drive task that is still unwinding cannot
    /// write a headless rollout back. Callers that may be deleting an active
    /// run should go through `api::runs::delete_run`, which cancels first.
    pub fn delete_run(&self, id: &str) -> bool {
        let mut guard = self.write();
        let state = &mut *guard;
        remove_run(&self.root, state, id)
    }

    // ------------------------------------------------------------------
    // Messages
    // ------------------------------------------------------------------

    pub fn append_message(&self, message: RunMessage) {
        let mut guard = self.write();
        let state = &mut *guard;
        if state.deleted_runs.contains(&message.run_id) {
            return;
        }
        state
            .messages
            .entry(message.run_id.clone())
            .or_default()
            .push(message.clone());
        append_event(
            &self.root,
            state,
            &message.run_id,
            &message.created_at,
            "message",
            message_payload(&message),
        );
        let ephemeral = state.runs.get(&message.run_id).is_some_and(|run| run.ephemeral);
        if message.role == "user" && message.agent_role.is_none() && !ephemeral {
            append_history(&self.root, &message.run_id, &message.content, unix_secs(&message.created_at));
        }
    }

    pub fn list_messages(&self, run_id: &str) -> Vec<RunMessage> {
        self.read()
            .messages
            .get(run_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Updates a message in the in-memory index and appends a
    /// `message_update` line carrying the replacement message.
    pub fn update_message(
        &self,
        run_id: &str,
        message_id: &str,
        content: &str,
        reasoning_content: Option<&str>,
    ) -> Option<RunMessage> {
        self.update_message_full(run_id, message_id, content, reasoning_content, None)
    }

    /// Like `update_message`, but can also replace the serialized
    /// `rawResponse` blob (used for tool-call payloads on assistant messages).
    pub fn update_message_full(
        &self,
        run_id: &str,
        message_id: &str,
        content: &str,
        reasoning_content: Option<&str>,
        raw_response: Option<&str>,
    ) -> Option<RunMessage> {
        self.apply_message_update(run_id, message_id, content, reasoning_content, raw_response, true)
    }

    /// A streaming update of a message that is still being generated: the
    /// in-memory copy is always current, but the rollout only gets a
    /// `message_update` checkpoint every [`STREAM_CHECKPOINT_INTERVAL`]. The
    /// final `update_message_full` always persists, so a 100 KB answer costs a
    /// handful of lines instead of hundreds (QA-035). A crash loses at most the
    /// last few seconds of streamed text of a run that is failed on restart anyway.
    pub fn stream_message(
        &self,
        run_id: &str,
        message_id: &str,
        content: &str,
        reasoning_content: Option<&str>,
    ) -> Option<RunMessage> {
        self.apply_message_update(run_id, message_id, content, reasoning_content, None, false)
    }

    fn apply_message_update(
        &self,
        run_id: &str,
        message_id: &str,
        content: &str,
        reasoning_content: Option<&str>,
        raw_response: Option<&str>,
        force_persist: bool,
    ) -> Option<RunMessage> {
        let mut guard = self.write();
        let state = &mut *guard;
        let message = state
            .messages
            .get_mut(run_id)?
            .iter_mut()
            .find(|message| message.id == message_id)?;
        message.content = content.to_string();
        if let Some(reasoning) = reasoning_content {
            message.reasoning_content = Some(reasoning.to_string());
        }
        if let Some(raw) = raw_response {
            message.raw_response = Some(raw.to_string());
        }
        let updated = message.clone();
        let now = Instant::now();
        let due = match state.stream_checkpoints.get(message_id) {
            Some(last) => now.duration_since(*last) >= STREAM_CHECKPOINT_INTERVAL,
            None => {
                // The first streamed chunk only starts the clock: the
                // placeholder message is already on disk.
                state.stream_checkpoints.insert(message_id.to_string(), now);
                false
            }
        };
        if !force_persist && !due {
            return Some(updated);
        }
        if force_persist {
            state.stream_checkpoints.remove(message_id);
        } else {
            state.stream_checkpoints.insert(message_id.to_string(), now);
        }
        append_event(
            &self.root,
            state,
            run_id,
            &updated.created_at,
            "message_update",
            message_payload(&updated),
        );
        Some(updated)
    }

    // ------------------------------------------------------------------
    // Plans
    // ------------------------------------------------------------------

    pub fn get_plan(&self, run_id: &str) -> Option<Plan> {
        self.read().plans.get(run_id).cloned()
    }

    pub fn set_plan(&self, plan: Plan) {
        let mut guard = self.write();
        let state = &mut *guard;
        if state.deleted_runs.contains(&plan.run_id) {
            return;
        }
        state.plans.insert(plan.run_id.clone(), plan.clone());
        append_event(&self.root, state, &plan.run_id, &plan.created_at, "plan", plan_payload(&plan));
    }

    // ------------------------------------------------------------------
    // Usage logs (`usage` lines of the run's rollout)
    // ------------------------------------------------------------------

    /// Appends a usage line to the run's rollout. The cross-process lock is
    /// held only while the id is allocated from `.usage_seq`; the shared
    /// files are not re-read (QA-036).
    pub fn append_usage(&self, mut entry: UsageLogEntry) -> UsageLogEntry {
        let mut guard = self.write();
        let state = &mut *guard;
        if state.deleted_runs.contains(&entry.run_id) {
            return entry;
        }
        if entry.id == 0 {
            let _lock = self.cross_process_lock();
            entry.id = allocate_usage_id(&self.root, state);
        }
        state.usage.push(entry.clone());
        let payload = serde_json::to_value(&entry).unwrap_or(Value::Null);
        append_event(&self.root, state, &entry.run_id, &entry.created_at, "usage", payload);
        entry
    }

    /// Re-reads the rollouts that changed on disk so runs created or changed
    /// by another process (the desktop app or a second `lcgn`) become
    /// visible. Incremental: only rollouts whose mtime/size changed are
    /// replayed, and runs this process is driving (they hold their owner
    /// lock, and may have in-memory streaming state) are never overwritten.
    pub fn refresh_runs(&self) {
        let mut paths = Vec::new();
        collect_rollouts(&self.root.join("sessions"), &mut paths);
        paths.sort();
        let mut guard = self.write();
        let state = &mut *guard;
        let seen: HashSet<PathBuf> = paths.iter().cloned().collect();

        for path in paths {
            let sig = file_sig(&path);
            if state.rollout_sigs.get(&path) == Some(&sig) {
                continue;
            }
            state.rollout_sigs.insert(path.clone(), sig);
            let mut fresh = StorageState::default();
            replay_rollout(&path, &mut fresh);
            merge_replayed_runs(state, fresh);
        }

        // Rollouts deleted by another process take their runs with them.
        let gone: Vec<PathBuf> = state
            .rollout_sigs
            .keys()
            .filter(|path| !seen.contains(*path))
            .cloned()
            .collect();
        for path in gone {
            state.rollout_sigs.remove(&path);
            let orphans: Vec<String> = state
                .rollouts
                .iter()
                .filter(|(id, owned)| **owned == path && !state.run_claims.contains_key(*id))
                .map(|(id, _)| id.clone())
                .collect();
            for id in orphans {
                forget_run(state, &id);
            }
        }
        state.usage.sort_by_key(|entry| entry.id);
        state.next_usage_id = next_usage_id_from(&self.root, &state.usage);
    }

    /// `refresh_runs` at most every [`REFRESH_INTERVAL`]; cheap enough to call
    /// from every run listing.
    pub fn refresh_runs_throttled(&self) {
        {
            let mut last = self.last_refresh.lock().unwrap_or_else(PoisonError::into_inner);
            if last.is_some_and(|at| at.elapsed() < REFRESH_INTERVAL) {
                return;
            }
            *last = Some(Instant::now());
        }
        self.refresh_runs();
    }

    /// Reloads `permissions.json` / `projects.json` / the memory files when
    /// another process changed them, so grants revoked elsewhere take effect
    /// here (QA-039).
    fn sync_shared_files(&self, projects: bool, permissions: bool, memories: bool) {
        let (projects_sig, permissions_sig) = {
            let state = self.read();
            (state.projects_sig, state.permissions_sig)
        };
        let stale = (projects && file_sig(&self.root.join("projects.json")) != projects_sig)
            || (permissions && file_sig(&self.root.join("permissions.json")) != permissions_sig)
            || (memories && memories_signature(&self.root) != self.read().memories_sig);
        if stale {
            reload_shared_files(&self.root, &mut self.write());
        }
    }

    pub fn list_usage(&self, run_id: Option<&str>) -> Vec<UsageLogEntry> {
        let state = self.read();
        state
            .usage
            .iter()
            .filter(|entry| run_id.map(|id| entry.run_id == id).unwrap_or(true))
            .cloned()
            .collect()
    }

    // ------------------------------------------------------------------
    // Internals
    // ------------------------------------------------------------------

    fn read(&self) -> std::sync::RwLockReadGuard<'_, StorageState> {
        // A panic inside one closure must not kill every later Db call.
        self.state.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, StorageState> {
        self.state.write().unwrap_or_else(PoisonError::into_inner)
    }
}

// ----------------------------------------------------------------------
// Run ownership, deletion and merging
// ----------------------------------------------------------------------

/// Claims (active run) or releases (terminal run) this process's owner lock
/// for `run`, so other processes know whether it is still being driven.
fn sync_run_claim(root: &Path, state: &mut StorageState, run: &Run) {
    if ACTIVE_RUN_STATUSES.contains(&run.status.as_str()) {
        if !state.run_claims.contains_key(&run.id) {
            if let Some(file) = acquire_run_lock(root, &run.id) {
                state.run_claims.insert(run.id.clone(), file);
            }
        }
    } else {
        release_run_claim(root, state, &run.id);
    }
}

fn release_run_claim(root: &Path, state: &mut StorageState, run_id: &str) {
    if let Some(file) = state.run_claims.remove(run_id) {
        let _ = file.unlock();
        drop(file);
        let _ = std::fs::remove_file(run_lock_path(root, run_id));
    }
}

/// Drops every in-memory trace of a run (no file is touched).
fn forget_run(state: &mut StorageState, id: &str) {
    state.runs.remove(id);
    state.messages.remove(id);
    state.plans.remove(id);
    state.usage.retain(|entry| entry.run_id != id);
    state.rollouts.remove(id);
}

/// Deletes a run: tombstones the id, releases its owner lock, drops its
/// memory and removes its rollout file. Returns whether the run existed.
fn remove_run(root: &Path, state: &mut StorageState, id: &str) -> bool {
    state.deleted_runs.insert(id.to_string());
    release_run_claim(root, state, id);
    state.stream_checkpoints.retain(|message_id, _| {
        !state.messages.get(id).is_some_and(|messages| messages.iter().any(|m| &m.id == message_id))
    });
    let existed = state.runs.contains_key(id);
    let path = state.rollouts.get(id).cloned();
    forget_run(state, id);
    if let Some(path) = path {
        state.rollout_sigs.remove(&path);
        let _ = std::fs::remove_file(&path);
        prune_empty_dirs(&path, &root.join("sessions"));
    }
    existed
}

/// Folds freshly replayed runs into `state`, except runs this process owns
/// or has deleted.
fn merge_replayed_runs(state: &mut StorageState, mut fresh: StorageState) {
    let ids: Vec<String> = fresh.runs.keys().cloned().collect();
    for id in ids {
        if state.run_claims.contains_key(&id) || state.deleted_runs.contains(&id) {
            continue;
        }
        if let Some(run) = fresh.runs.remove(&id) {
            state.runs.insert(id.clone(), run);
        }
        state.messages.insert(id.clone(), fresh.messages.remove(&id).unwrap_or_default());
        match fresh.plans.remove(&id) {
            Some(plan) => {
                state.plans.insert(id.clone(), plan);
            }
            None => {
                state.plans.remove(&id);
            }
        }
        state.usage.retain(|entry| entry.run_id != id);
        state.usage.extend(fresh.usage.iter().filter(|entry| entry.run_id == id).cloned());
        if let Some(path) = fresh.rollouts.remove(&id) {
            state.rollouts.insert(id, path);
        }
    }
}

// ----------------------------------------------------------------------
// Rollout files
// ----------------------------------------------------------------------

fn run_payload(run: &Run) -> Value {
    serde_json::to_value(run).unwrap_or(Value::Null)
}

fn message_payload(message: &RunMessage) -> Value {
    serde_json::to_value(message).unwrap_or(Value::Null)
}

fn plan_payload(plan: &Plan) -> Value {
    serde_json::to_value(plan).unwrap_or(Value::Null)
}

/// One serialized rollout line, newline-terminated.
fn rollout_line(timestamp: &str, kind: &str, payload: &Value) -> String {
    let mut line = json!({ "timestamp": timestamp, "type": kind, "payload": payload }).to_string();
    line.push('\n');
    line
}

fn safe_file_part(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

fn parse_time(iso: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(iso)
        .map(|time| time.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn unix_secs(iso: &str) -> u64 {
    parse_time(iso).timestamp().max(0) as u64
}

/// `sessions/YYYY/MM/DD/rollout-YYYY-MM-DDThh-mm-ss-<run_id>.jsonl`.
fn new_rollout_path(root: &Path, run_id: &str, created_at: &str) -> PathBuf {
    let time = parse_time(created_at);
    root.join("sessions")
        .join(time.format("%Y").to_string())
        .join(time.format("%m").to_string())
        .join(time.format("%d").to_string())
        .join(format!(
            "rollout-{}-{}.jsonl",
            time.format("%Y-%m-%dT%H-%M-%S"),
            safe_file_part(run_id)
        ))
}

/// Appends one event to the run's rollout, creating the file (date-sharded
/// by `created_hint`) on first write.
fn append_event(
    root: &Path,
    state: &mut StorageState,
    run_id: &str,
    created_hint: &str,
    kind: &str,
    payload: Value,
) {
    if state.deleted_runs.contains(run_id) {
        return;
    }
    let path = state
        .rollouts
        .entry(run_id.to_string())
        .or_insert_with(|| new_rollout_path(root, run_id, created_hint))
        .clone();
    append_text(&path, &rollout_line(&now_iso(), kind, &payload));
}

/// Appends text to a file (creating it and its directories) and flushes. If
/// the file ends in a truncated line, a newline is written first so the new
/// line is not glued onto the corrupt one.
fn append_text(path: &Path, text: &str) {
    if let Err(err) = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(path)?;
        let mut buffer = String::new();
        if file.metadata()?.len() > 0 {
            file.seek(SeekFrom::End(-1))?;
            let mut last = [0u8; 1];
            file.read_exact(&mut last)?;
            if last[0] != b'\n' {
                buffer.push('\n');
            }
        }
        buffer.push_str(text);
        file.write_all(buffer.as_bytes())?;
        file.flush()?;
        Ok(())
    })() {
        tracing::error!("[Database] Failed to append {}: {err}", path.display());
    }
}

/// Removes now-empty date directories above a deleted rollout, stopping at
/// (and keeping) `stop`.
fn prune_empty_dirs(file: &Path, stop: &Path) {
    let mut dir = file.parent();
    while let Some(current) = dir {
        if current == stop || !current.starts_with(stop) || std::fs::remove_dir(current).is_err() {
            break;
        }
        dir = current.parent();
    }
}

fn collect_rollouts(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            collect_rollouts(&path, out);
        } else {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("rollout-") && name.ends_with(".jsonl") {
                out.push(path);
            }
        }
    }
}

/// Replaces `items[id]` or pushes when absent.
fn upsert_message(messages: &mut Vec<RunMessage>, message: RunMessage) {
    match messages.iter().rposition(|existing| existing.id == message.id) {
        Some(index) => messages[index] = message,
        None => messages.push(message),
    }
}

/// Replays one rollout file into `state` (last write wins). Unparseable
/// lines, including a truncated last line, are skipped.
fn replay_rollout(path: &Path, state: &mut StorageState) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let raw = String::from_utf8_lossy(&bytes);
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(RolloutLine { kind, payload }) = serde_json::from_str::<RolloutLine>(line) else {
            continue;
        };
        match kind.as_str() {
            "session_meta" | "run_update" => {
                if let Ok(run) = serde_json::from_value::<Run>(payload) {
                    if !run.id.is_empty() {
                        state.rollouts.insert(run.id.clone(), path.to_path_buf());
                        state.runs.insert(run.id.clone(), run);
                    }
                }
            }
            "message" | "message_update" => {
                if let Ok(message) = serde_json::from_value::<RunMessage>(payload) {
                    state
                        .rollouts
                        .entry(message.run_id.clone())
                        .or_insert_with(|| path.to_path_buf());
                    upsert_message(state.messages.entry(message.run_id.clone()).or_default(), message);
                }
            }
            "plan" => {
                if let Ok(plan) = serde_json::from_value::<Plan>(payload) {
                    state
                        .rollouts
                        .entry(plan.run_id.clone())
                        .or_insert_with(|| path.to_path_buf());
                    state.plans.insert(plan.run_id.clone(), plan);
                }
            }
            "usage" => {
                if let Ok(entry) = serde_json::from_value::<UsageLogEntry>(payload) {
                    state
                        .rollouts
                        .entry(entry.run_id.clone())
                        .or_insert_with(|| path.to_path_buf());
                    state.usage.push(entry);
                }
            }
            _ => {}
        }
    }
}

/// Scans `sessions/` and replays every rollout into `state`.
fn load_rollouts(root: &Path, state: &mut StorageState) {
    let mut paths = Vec::new();
    collect_rollouts(&root.join("sessions"), &mut paths);
    paths.sort();
    for path in paths {
        state.rollout_sigs.insert(path.clone(), file_sig(&path));
        replay_rollout(&path, state);
    }
    state.usage.sort_by_key(|entry| entry.id);
}

// ----------------------------------------------------------------------
// Index files
// ----------------------------------------------------------------------

/// `session_index.jsonl`: append-only `{id, title, updated_at}`; the last
/// line per id wins.
fn append_session_index(root: &Path, run: &Run) {
    if run.ephemeral {
        return;
    }
    let mut line = json!({
        "id": run.id,
        "title": run.title,
        "updated_at": run.updated_at,
    })
    .to_string();
    line.push('\n');
    append_text(&root.join(SESSION_INDEX_FILE), &line);
}

/// `history.jsonl`: append-only `{session_id, ts, text}`, one per user prompt.
fn append_history(root: &Path, session_id: &str, text: &str, ts: u64) {
    // `history.persistence = "none"` in config.toml turns the file off.
    if !crate::config_runtime::current().history_persistence_enabled() {
        return;
    }
    let mut line = json!({ "session_id": session_id, "ts": ts, "text": text }).to_string();
    line.push('\n');
    append_text(&root.join(HISTORY_FILE), &line);
}

/// Next usage id: above everything in memory and in the cross-process
/// counter file `.usage_seq` (usage lines live in separate rollouts, so the
/// counter is what keeps ids unique between processes).
fn allocate_usage_id(root: &Path, state: &mut StorageState) -> i64 {
    let on_disk = std::fs::read_to_string(root.join(USAGE_SEQ_FILE))
        .ok()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .unwrap_or(0);
    let id = on_disk.max(state.next_usage_id - 1).max(0) + 1;
    if let Err(err) = std::fs::write(root.join(USAGE_SEQ_FILE), id.to_string()) {
        tracing::error!("[Database] Failed to write {USAGE_SEQ_FILE}: {err}");
    }
    state.next_usage_id = id + 1;
    id
}

fn next_usage_id_from(root: &Path, usage: &[UsageLogEntry]) -> i64 {
    let on_disk = std::fs::read_to_string(root.join(USAGE_SEQ_FILE))
        .ok()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .unwrap_or(0);
    usage.iter().map(|entry| entry.id).max().unwrap_or(0).max(on_disk) + 1
}

// ----------------------------------------------------------------------
// Memories as Markdown
// ----------------------------------------------------------------------

/// Directory name for a project path: every non-alphanumeric char becomes `-`.
pub(crate) fn sanitize_project_path(path: &str) -> String {
    let name: String = path
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .take(120)
        .collect();
    if name.is_empty() { "project".to_string() } else { name }
}

fn memory_dir(root: &Path, memory: &Memory) -> PathBuf {
    if memory.project_path.is_empty() {
        root.join("memories")
    } else {
        root.join("projects")
            .join(sanitize_project_path(&memory.project_path))
            .join("memories")
    }
}

/// `<first words of the fact>-<id>.md`.
fn memory_file_path(root: &Path, memory: &Memory) -> PathBuf {
    let words: Vec<String> = memory
        .content
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(6)
        .map(|word| word.to_lowercase())
        .collect();
    let mut slug: String = words.join("-").chars().take(40).collect();
    if slug.is_empty() {
        slug = "memory".to_string();
    }
    memory_dir(root, memory).join(format!("{slug}-{}.md", memory.id))
}

/// YAML frontmatter (string values JSON-quoted, which is valid YAML and
/// needs no further escaping) followed by the verbatim body.
fn render_memory(memory: &Memory) -> String {
    let quote = |value: &str| serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string());
    let mut text = String::from("---\n");
    text.push_str(&format!("id: {}\n", memory.id));
    text.push_str(&format!("category: {}\n", quote(&memory.category)));
    text.push_str(&format!("scope: {}\n", quote(&memory.scope)));
    if !memory.project_path.is_empty() {
        text.push_str(&format!("project_path: {}\n", quote(&memory.project_path)));
    }
    text.push_str(&format!("created_at: {}\n", quote(&memory.created_at)));
    text.push_str(&format!("updated_at: {}\n", quote(&memory.updated_at)));
    text.push_str("---\n");
    text.push_str(&memory.content);
    text
}

fn parse_frontmatter_value(raw: &str) -> String {
    let raw = raw.trim();
    if raw.starts_with('"') {
        if let Ok(value) = serde_json::from_str::<String>(raw) {
            return value;
        }
    }
    raw.trim_matches('\'').to_string()
}

fn parse_memory(raw: &str) -> Option<Memory> {
    let rest = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))?;
    let mut memory = Memory::default();
    let mut has_id = false;
    let mut offset = 0;
    let mut body = None;
    for line in rest.split_inclusive('\n') {
        offset += line.len();
        if line.trim_end() == "---" {
            body = Some(&rest[offset..]);
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = parse_frontmatter_value(value);
        match key.trim() {
            "id" => {
                memory.id = value.parse().ok()?;
                has_id = true;
            }
            "category" => memory.category = value,
            "scope" => memory.scope = value,
            "project_path" => memory.project_path = value,
            "created_at" => memory.created_at = value,
            "updated_at" => memory.updated_at = value,
            _ => {}
        }
    }
    if !has_id {
        return None;
    }
    memory.content = body?.to_string();
    if memory.scope.is_empty() {
        memory.scope = if memory.project_path.is_empty() { "global" } else { "project" }.to_string();
    }
    Some(memory)
}

/// The global memory directory plus one per project directory.
fn memory_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![root.join("memories")];
    if let Ok(entries) = std::fs::read_dir(root.join("projects")) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                dirs.push(entry.path().join("memories"));
            }
        }
    }
    dirs
}

/// Cheap fingerprint of all memory files (path, mtime, size): changes when
/// another process adds, edits or removes a memory.
fn memories_signature(root: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut entries: Vec<(PathBuf, FileSig)> = Vec::new();
    for dir in memory_dirs(root) {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        for entry in read.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                let sig = file_sig(&path);
                entries.push((path, sig));
            }
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    entries.hash(&mut hasher);
    hasher.finish()
}

/// Reads every memory `.md` (global and per-project) with its file path.
fn load_memories(root: &Path) -> Vec<(Memory, PathBuf)> {
    let mut found = Vec::new();
    for dir in memory_dirs(root) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            if let Some(memory) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| parse_memory(&raw))
            {
                found.push((memory, path));
            }
        }
    }
    found.sort_by_key(|(memory, _)| memory.id);
    found
}

// ----------------------------------------------------------------------
// JSON primitives
// ----------------------------------------------------------------------

/// Atomic text write: write a `.tmp` sibling, then rename over the target.
fn write_text_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Atomic JSON write so readers never observe a half-written file.
fn write_json_atomic(path: &Path, value: &impl Serialize) {
    if let Err(err) = (|| -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
        write_text_atomic(path, &json)
    })() {
        tracing::error!("[Database] Failed to persist {}: {err}", path.display());
    }
}

fn persist_projects(root: &Path, projects: &[Project]) {
    write_json_atomic(&root.join("projects.json"), &projects);
}

fn persist_permissions(root: &Path, permissions: &[PermissionRule]) {
    write_json_atomic(&root.join("permissions.json"), &permissions);
}

// ----------------------------------------------------------------------
// Startup loading
// ----------------------------------------------------------------------

fn read_json_or<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn read_jsonl_or<T: for<'de> Deserialize<'de>>(path: &Path) -> Vec<T> {
    std::fs::read(path)
        .map(|bytes| {
            String::from_utf8_lossy(&bytes)
                .lines()
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn load_state(root: &Path) -> anyhow::Result<StorageState> {
    let mut state = StorageState::default();
    reload_shared_files(root, &mut state);
    load_rollouts(root, &mut state);
    state.next_usage_id = next_usage_id_from(root, &state.usage);
    Ok(state)
}

/// Re-reads the cross-process-shared files (`projects.json`,
/// `permissions.json`, memory Markdown) into `state`. Used at startup and by
/// `with_shared_mutation` under the advisory lock, so a mutation always
/// applies on top of the latest on-disk state written by another process.
fn reload_shared_files(root: &Path, state: &mut StorageState) {
    // Signatures first: a write racing with the read below is then picked up
    // by the next sync instead of being lost.
    state.projects_sig = file_sig(&root.join("projects.json"));
    state.permissions_sig = file_sig(&root.join("permissions.json"));
    state.memories_sig = memories_signature(root);
    state.projects = read_json_or::<Vec<Project>>(&root.join("projects.json"));
    state.permissions = read_json_or::<Vec<PermissionRule>>(&root.join("permissions.json"));
    let memories = load_memories(root);
    state.memory_files = memories
        .iter()
        .map(|(memory, path)| (memory.id, path.clone()))
        .collect();
    state.memories = memories.into_iter().map(|(memory, _)| memory).collect();
    state.next_permission_id = state
        .permissions
        .iter()
        .map(|rule| rule.id)
        .max()
        .unwrap_or(0)
        + 1;
    state.next_memory_id = state.memories.iter().map(|m| m.id).max().unwrap_or(0) + 1;
}

// ----------------------------------------------------------------------
// Legacy import
// ----------------------------------------------------------------------

/// The legacy data dir to import from, only when `root` is the default
/// Locagens home (so custom roots and tests never read the real app data).
fn default_legacy_dir_for(root: &Path) -> Option<PathBuf> {
    (root == crate::paths::locagens_home()).then(|| crate::paths::app_support_dir().join("data"))
}

fn load_optional_plan(path: &Path) -> Option<Plan> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// One-time import of the legacy layout (`projects.json`, `permissions.json`,
/// `memory.json`, `usage_logs.jsonl`, `sessions/<id>/{run.json,messages.jsonl,
/// plan.json}`) into rollouts, Markdown memories and the index files. The
/// legacy directory is only read. `<root>/.migrated-from-data-v1` is written
/// last, so an interrupted import is retried (existing files are not
/// overwritten). Usage lines of runs that no longer exist are dropped.
fn migrate_legacy_data(legacy: &Path, root: &Path) -> anyhow::Result<()> {
    let marker = root.join(MIGRATION_MARKER);
    if marker.exists() || !legacy.is_dir() {
        return Ok(());
    }
    let _lock = lock_root(root);
    if marker.exists() {
        return Ok(());
    }

    for name in ["projects.json", "permissions.json"] {
        let (from, to) = (legacy.join(name), root.join(name));
        if from.is_file() && !to.exists() {
            std::fs::copy(&from, &to)?;
        }
    }

    let mut usage_by_run: HashMap<String, Vec<UsageLogEntry>> = HashMap::new();
    let mut max_usage_id = 0;
    for entry in read_jsonl_or::<UsageLogEntry>(&legacy.join("usage_logs.jsonl")) {
        max_usage_id = max_usage_id.max(entry.id);
        usage_by_run.entry(entry.run_id.clone()).or_default().push(entry);
    }

    let mut history: Vec<(u64, String, String)> = Vec::new();
    let mut index_lines = String::new();
    if let Ok(entries) = std::fs::read_dir(legacy.join("sessions")) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let dir = entry.path();
            let dir_id = entry.file_name().to_string_lossy().to_string();
            let mut run: Run = read_json_or(&dir.join("run.json"));
            let messages = read_jsonl_or::<RunMessage>(&dir.join("messages.jsonl"));
            let plan = load_optional_plan(&dir.join("plan.json"));
            let has_run = !run.id.is_empty();
            if !has_run && messages.is_empty() && plan.is_none() {
                continue;
            }
            if !has_run {
                run.id = dir_id;
            }
            let created = if !run.created_at.is_empty() {
                run.created_at.clone()
            } else {
                messages.first().map(|m| m.created_at.clone()).unwrap_or_else(now_iso)
            };
            let path = new_rollout_path(root, &run.id, &created);
            if path.exists() {
                continue;
            }

            let mut buffer = String::new();
            if has_run {
                buffer.push_str(&rollout_line(&created, "session_meta", &run_payload(&run)));
            }
            for message in &messages {
                buffer.push_str(&rollout_line(&message.created_at, "message", &message_payload(message)));
                if message.role == "user" && message.agent_role.is_none() {
                    history.push((unix_secs(&message.created_at), run.id.clone(), message.content.clone()));
                }
            }
            if let Some(plan) = plan.as_ref() {
                buffer.push_str(&rollout_line(&plan.updated_at, "plan", &plan_payload(plan)));
            }
            for usage in usage_by_run.remove(&run.id).unwrap_or_default() {
                let payload = serde_json::to_value(&usage).unwrap_or(Value::Null);
                buffer.push_str(&rollout_line(&usage.created_at, "usage", &payload));
            }
            write_text_atomic(&path, &buffer)?;
            if has_run {
                index_lines.push_str(
                    &json!({ "id": run.id, "title": run.title, "updated_at": run.updated_at }).to_string(),
                );
                index_lines.push('\n');
            }
        }
    }
    if !index_lines.is_empty() {
        append_text(&root.join(SESSION_INDEX_FILE), &index_lines);
    }
    if !history.is_empty() && !root.join(HISTORY_FILE).exists() {
        history.sort_by_key(|(ts, _, _)| *ts);
        let mut buffer = String::new();
        for (ts, session_id, text) in history {
            buffer.push_str(&json!({ "session_id": session_id, "ts": ts, "text": text }).to_string());
            buffer.push('\n');
        }
        append_text(&root.join(HISTORY_FILE), &buffer);
    }

    let mut taken: HashSet<i64> = HashSet::new();
    for memory in read_json_or::<Vec<Memory>>(&legacy.join("memory.json")) {
        if !taken.insert(memory.id) {
            continue;
        }
        let path = memory_file_path(root, &memory);
        if !path.exists() {
            write_text_atomic(&path, &render_memory(&memory))?;
        }
    }

    if max_usage_id > 0 {
        std::fs::write(root.join(USAGE_SEQ_FILE), max_usage_id.to_string())?;
    }
    std::fs::write(&marker, format!("{}\nimported from {}\n", now_iso(), legacy.display()))?;
    Ok(())
}

/// Resolve the store root: `LOCAGENS_DATA_DIR`, else the `LOCAGENS_DB_PATH`
/// parent joined with its file stem, else the Locagens home
/// (`$LOCAGENS_HOME` or `~/.locagens`).
pub fn resolve_data_root() -> PathBuf {
    if let Ok(dir) = std::env::var("LOCAGENS_DATA_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(path) = std::env::var("LOCAGENS_DB_PATH") {
        if !path.is_empty() {
            let db_path = PathBuf::from(&path);
            let parent = db_path.parent().unwrap_or_else(|| Path::new("."));
            let stem = db_path
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_else(|| "data".to_string());
            return parent.join(stem);
        }
    }
    crate::paths::locagens_home()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "locagens-db-test-{}-{}",
            label,
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn rollouts_in(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        collect_rollouts(&root.join("sessions"), &mut out);
        out.sort();
        out
    }

    fn lines_of(path: &Path) -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn kinds_of(path: &Path) -> Vec<String> {
        lines_of(path)
            .iter()
            .map(|line| line["type"].as_str().unwrap().to_string())
            .collect()
    }

    fn sample_run(id: &str) -> Run {
        Run {
            id: id.into(),
            title: "Test".into(),
            task: "do things".into(),
            status: "done".into(),
            provider_id: "p".into(),
            model: "m".into(),
            created_at: "2026-03-07T10:20:30.000Z".into(),
            updated_at: "2026-03-07T10:20:30.000Z".into(),
            ..Default::default()
        }
    }

    fn msg(run: &str, id: &str, role: &str, content: &str) -> RunMessage {
        RunMessage {
            id: id.into(),
            run_id: run.into(),
            role: role.into(),
            content: content.into(),
            created_at: now_iso(),
            ..Default::default()
        }
    }

    #[test]
    fn projects_and_memories_persist_across_reopen() {
        let root = temp_root("persist");
        let db = Db::open(&root).unwrap();
        db.run_startup_tasks(Path::new("C:/ws"), "Locagens");
        assert_eq!(db.list_projects().len(), 1);
        assert_eq!(db.list_projects()[0].name, "Locagens");

        db.create_memory("project", "C:/ws", "reference", "hello fact").unwrap();
        db.create_project("C:/other", "Other");
        let memory = db.list_memories()[0].clone();
        let updated = db.update_memory(memory.id, "edited fact").unwrap();
        assert_eq!(updated.content, "edited fact");
        drop(db);

        let db2 = Db::open(&root).unwrap();
        db2.run_startup_tasks(Path::new("C:/ws"), "Locagens");
        assert_eq!(db2.list_projects().len(), 2);
        let memories = db2.list_memories();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].content, "edited fact");
        assert_eq!(memories[0].id, memory.id);

        assert!(db2.delete_memory(memory.id));
        assert!(!db2.delete_memory(memory.id));
        db2.clear_memories();
        assert!(db2.list_memories().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn permission_dedup_and_ids() {
        let root = temp_root("perms");
        let db = Db::open(&root).unwrap();
        let first = db.upsert_permission("run", "C:/ws", "run_command", "git status", vec![], "allow_once");
        let second = db.upsert_permission("run", "C:/ws", "run_command", "git status", vec![], "allow_always");
        let other = db.upsert_permission("run", "C:/ws", "run_command", "git push", vec![], "allow_once");
        assert_eq!(first.id, 1);
        assert_eq!(second.id, 1, "duplicate rule is deduplicated");
        assert_eq!(second.status, "allow_always");
        assert_eq!(other.id, 2);
        assert_eq!(db.list_permissions().len(), 2);
        assert!(db.delete_permission(1));
        assert_eq!(db.list_permissions().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stuck_runs_fail_on_startup_with_system_message() {
        let root = temp_root("stuck");
        let db = Db::open(&root).unwrap();
        db.create_run(Run {
            status: "generating".into(),
            ..sample_run("run-1")
        });
        drop(db);

        let db2 = Db::open(&root).unwrap();
        db2.run_startup_tasks(Path::new("C:/ws"), "Locagens");
        let run = db2.get_run_full("run-1").unwrap();
        assert_eq!(run.status, "failed");
        assert_eq!(
            run.error_message.as_deref(),
            Some("Session interrupted due to server restart.")
        );
        let messages = db2.list_messages("run-1");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "system");

        // The failure is appended, not rewritten, and survives a reopen.
        let files = rollouts_in(&root);
        assert_eq!(kinds_of(&files[0]), ["session_meta", "run_update", "message"]);
        drop(db2);
        let db3 = Db::open(&root).unwrap();
        assert_eq!(db3.get_run_full("run-1").unwrap().status, "failed");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rollout_round_trip_layout_and_history() {
        let root = temp_root("roundtrip");
        let db = Db::open(&root).unwrap();
        db.create_run(sample_run("run-a"));
        db.append_message(msg("run-a", "m1", "user", "hello there"));
        db.append_message(RunMessage {
            agent_role: Some("explorer".into()),
            ..msg("run-a", "m2", "user", "sub-agent prompt")
        });
        db.append_message(msg("run-a", "m3", "assistant", "hi"));
        db.set_plan(Plan {
            id: "plan-1".into(),
            run_id: "run-a".into(),
            title: "Plan".into(),
            tasks: json!([{"text": "step"}]),
            version: 1,
            ..Default::default()
        });
        db.append_usage(UsageLogEntry {
            run_id: "run-a".into(),
            provider_id: "p".into(),
            model: "m".into(),
            input_tokens: 10,
            cost: 0.5,
            ..Default::default()
        });
        drop(db);

        // One date-sharded rollout named after the run's creation time.
        let files = rollouts_in(&root);
        assert_eq!(files.len(), 1);
        let rel = files[0].strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        assert_eq!(rel, "sessions/2026/03/07/rollout-2026-03-07T10-20-30-run-a.jsonl");
        assert_eq!(
            kinds_of(&files[0]),
            ["session_meta", "message", "message", "message", "plan", "usage"]
        );
        let first = &lines_of(&files[0])[0];
        assert!(first["timestamp"].is_string());
        assert_eq!(first["payload"]["id"], "run-a");

        let db2 = Db::open(&root).unwrap();
        assert_eq!(db2.get_run_full("run-a").unwrap(), sample_run("run-a"));
        assert_eq!(db2.list_messages("run-a").len(), 3);
        assert_eq!(db2.get_plan("run-a").unwrap().title, "Plan");
        assert_eq!(db2.list_usage(Some("run-a")).len(), 1);
        assert_eq!(db2.list_usage(None)[0].cost, 0.5);
        assert!(!root.join("usage_logs.jsonl").exists());

        // Index: last line per id wins; history: only the main-agent prompt.
        let history: Vec<Value> = read_jsonl_or(&root.join(HISTORY_FILE));
        assert_eq!(history.len(), 1);
        assert_eq!(history[0]["session_id"], "run-a");
        assert_eq!(history[0]["text"], "hello there");
        assert!(history[0]["ts"].is_u64());
        db2.patch_run("run-a", |run| run.title = "Renamed".into());
        let index: Vec<Value> = read_jsonl_or(&root.join(SESSION_INDEX_FILE));
        assert_eq!(index.first().unwrap()["title"], "Test");
        assert_eq!(index.last().unwrap()["id"], "run-a");
        assert_eq!(index.last().unwrap()["title"], "Renamed");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn updates_are_appended_and_replayed_last_write_wins() {
        let root = temp_root("messages");
        let db = Db::open(&root).unwrap();
        db.append_message(msg("run-x", "m1", "assistant", "first"));
        db.append_message(msg("run-x", "m2", "user", "second"));
        let updated = db.update_message("run-x", "m1", "rewritten", Some("thinking"));
        assert_eq!(updated.unwrap().content, "rewritten");
        db.update_message_full("run-x", "m1", "rewritten again", None, Some("[]"));

        let files = rollouts_in(&root);
        assert_eq!(
            kinds_of(&files[0]),
            ["message", "message", "message_update", "message_update"],
            "updates append lines instead of rewriting"
        );
        let original = std::fs::read_to_string(&files[0]).unwrap();
        assert!(original.lines().next().unwrap().contains("\"first\""));

        drop(db);
        let db2 = Db::open(&root).unwrap();
        let messages = db2.list_messages("run-x");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "rewritten again");
        assert_eq!(messages[0].reasoning_content.as_deref(), Some("thinking"));
        assert_eq!(messages[0].raw_response.as_deref(), Some("[]"));

        let entry = db2.append_usage(UsageLogEntry {
            run_id: "run-x".into(),
            provider_id: "opencode-go".into(),
            model: "glm-5.3-flash".into(),
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        });
        assert_eq!(entry.id, 1);
        assert_eq!(db2.list_usage(Some("run-x")).len(), 1);
        assert_eq!(db2.list_usage(Some("run-y")).len(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_updates_replay_to_latest_snapshot() {
        let root = temp_root("runupdates");
        let db = Db::open(&root).unwrap();
        db.create_run(sample_run("run-u"));
        db.update_run_status("run-u", "failed", Some("boom"));
        db.patch_run("run-u", |run| run.title = "New title".into());
        drop(db);

        let files = rollouts_in(&root);
        assert_eq!(kinds_of(&files[0]), ["session_meta", "run_update", "run_update"]);
        let db2 = Db::open(&root).unwrap();
        let run = db2.get_run_full("run-u").unwrap();
        assert_eq!(run.status, "failed");
        assert_eq!(run.error_message.as_deref(), Some("boom"));
        assert_eq!(run.title, "New title");
        assert_eq!(db2.list_runs().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fork_run_writes_new_rollout_with_copied_lines() {
        let root = temp_root("fork");
        let db = Db::open(&root).unwrap();
        let source = db.create_run(Run {
            title: "Original".into(),
            ..sample_run("run-src")
        });
        for (id, role, content) in [
            ("m1", "user", "first"),
            ("m2", "assistant", "answer"),
            ("m3", "user", "second"),
        ] {
            db.append_message(msg("run-src", id, role, content));
        }
        db.set_plan(Plan {
            id: "plan-1".into(),
            run_id: "run-src".into(),
            title: "Plan".into(),
            tasks: json!([{"text": "step"}]),
            version: 1,
            ..Default::default()
        });

        // Fork up to (and including) the middle message.
        let fork = db.fork_run("run-src", Some("m2")).expect("fork created");
        assert_ne!(fork.id, source.id);
        assert_eq!(fork.title, "Original (fork)");
        assert_eq!(fork.status, "done");
        assert_eq!(fork.error_message, None);
        let fork_messages = db.list_messages(&fork.id);
        assert_eq!(fork_messages.len(), 2);
        assert_eq!(fork_messages[0].content, "first");
        assert_eq!(fork_messages[1].content, "answer");
        assert!(fork_messages.iter().all(|message| message.run_id == fork.id));
        assert!(fork_messages.iter().all(|message| message.id != "m1" && message.id != "m2"));
        let fork_plan = db.get_plan(&fork.id).expect("plan cloned");
        assert_eq!(fork_plan.title, "Plan");
        assert_eq!(fork_plan.run_id, fork.id);

        // It is its own rollout file, marked as forked.
        assert_eq!(rollouts_in(&root).len(), 2);
        let fork_file = rollouts_in(&root)
            .into_iter()
            .find(|path| path.to_string_lossy().contains(&fork.id))
            .unwrap();
        assert_eq!(kinds_of(&fork_file), ["session_meta", "message", "message", "plan"]);
        assert_eq!(lines_of(&fork_file)[0]["payload"]["forked_from"], "run-src");

        // Fork is fully independent: mutations on either side don't leak.
        db.append_message(msg("run-src", "m4", "user", "later"));
        db.append_message(msg(&fork.id, "mf", "user", "fork-only"));
        assert_eq!(db.list_messages("run-src").len(), 4);
        assert_eq!(db.list_messages(&fork.id).len(), 3);
        assert_eq!(db.get_plan("run-src").unwrap().run_id, "run-src");

        // Unknown source or unknown fork point → None.
        assert!(db.fork_run("nope", None).is_none());
        assert!(db.fork_run("run-src", Some("missing")).is_none());

        // Forking without a cut point copies everything.
        let full = db.fork_run("run-src", None).unwrap();
        assert_eq!(db.list_messages(&full.id).len(), 4);

        // And it all survives a reopen.
        drop(db);
        let db2 = Db::open(&root).unwrap();
        assert_eq!(db2.list_runs().len(), 3);
        assert_eq!(db2.list_messages(&fork.id).len(), 3);
        assert_eq!(db2.get_plan(&fork.id).unwrap().run_id, fork.id);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_run_removes_rollout_and_empty_date_dirs() {
        let root = temp_root("delete");
        let db = Db::open(&root).unwrap();
        db.create_run(sample_run("run-d"));
        db.create_run(sample_run("run-keep"));
        db.append_message(msg("run-d", "m1", "user", "bye"));
        db.append_usage(UsageLogEntry {
            run_id: "run-d".into(),
            ..Default::default()
        });
        db.append_usage(UsageLogEntry {
            run_id: "run-keep".into(),
            ..Default::default()
        });
        assert_eq!(rollouts_in(&root).len(), 2);

        assert!(db.delete_run("run-d"));
        assert!(!db.delete_run("run-d"));
        assert_eq!(rollouts_in(&root).len(), 1);
        assert!(db.get_run_full("run-d").is_none());
        assert!(db.list_messages("run-d").is_empty());
        assert_eq!(db.list_usage(None).len(), 1);

        assert!(db.delete_run("run-keep"));
        assert!(!root.join("sessions").join("2026").exists(), "empty date dirs pruned");
        drop(db);
        let db2 = Db::open(&root).unwrap();
        assert!(db2.list_runs().is_empty());
        assert!(db2.list_usage(None).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupt_or_truncated_last_line_is_skipped_and_appends_recover() {
        let root = temp_root("corrupt");
        let db = Db::open(&root).unwrap();
        db.create_run(sample_run("run-c"));
        db.append_message(msg("run-c", "m1", "user", "kept"));
        db.append_message(msg("run-c", "m2", "assistant", "will be cut"));
        drop(db);

        // Cut the file in the middle of the last line.
        let file = rollouts_in(&root).remove(0);
        let raw = std::fs::read(&file).unwrap();
        std::fs::write(&file, &raw[..raw.len() - 20]).unwrap();

        let db2 = Db::open(&root).unwrap();
        let messages = db2.list_messages("run-c");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "kept");
        assert!(db2.get_run_full("run-c").is_some());

        // A new append must not be glued onto the truncated line.
        db2.append_message(msg("run-c", "m3", "assistant", "after"));
        drop(db2);
        let db3 = Db::open(&root).unwrap();
        let contents: Vec<String> = db3.list_messages("run-c").into_iter().map(|m| m.content).collect();
        assert_eq!(contents, ["kept", "after"]);

        // Garbage in the middle is skipped too.
        let mut text = std::fs::read_to_string(&file).unwrap();
        text.insert_str(text.find('\n').unwrap() + 1, "not json at all\n");
        std::fs::write(&file, text).unwrap();
        let db4 = Db::open(&root).unwrap();
        assert_eq!(db4.list_messages("run-c").len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memories_are_markdown_files_with_escaped_frontmatter() {
        let root = temp_root("memmd");
        let db = Db::open(&root).unwrap();
        let nasty = "line one\n---\nid: 99\n\"quoted\" # not a comment: yes\r\nünïcode ✓\n\ntrailing  ";
        let global = db.create_memory("global", "", "we\"ird: cat\negory", nasty).unwrap();
        let project = db
            .create_memory("project", "C:\\Users\\me\\proj: x", "reference", "Use cargo nextest")
            .unwrap();

        let global_files: Vec<_> = std::fs::read_dir(root.join("memories")).unwrap().flatten().collect();
        assert_eq!(global_files.len(), 1);
        let text = std::fs::read_to_string(global_files[0].path()).unwrap();
        assert!(text.starts_with("---\nid: 1\n"));
        assert!(text.ends_with(nasty), "body is stored verbatim");
        let project_dir = root.join("projects").join("C--Users-me-proj--x").join("memories");
        let project_files: Vec<_> = std::fs::read_dir(&project_dir).unwrap().flatten().collect();
        assert_eq!(project_files.len(), 1);
        assert!(project_files[0].file_name().to_string_lossy().starts_with("use-cargo-nextest-"));
        assert!(!root.join("memory.json").exists());

        drop(db);
        let db2 = Db::open(&root).unwrap();
        let memories = db2.list_memories();
        assert_eq!(memories.len(), 2);
        let g = memories.iter().find(|m| m.id == global.id).unwrap();
        assert_eq!(g, &global);
        let p = memories.iter().find(|m| m.id == project.id).unwrap();
        assert_eq!(p, &project);

        // Updates rewrite the same file; deletes remove it.
        let edited = db2.update_memory(global.id, "short").unwrap();
        assert_eq!(edited.category, global.category);
        assert_eq!(std::fs::read_dir(root.join("memories")).unwrap().count(), 1);
        assert!(db2.delete_memory(project.id));
        assert_eq!(std::fs::read_dir(&project_dir).unwrap().count(), 0);
        drop(db2);
        let db3 = Db::open(&root).unwrap();
        assert_eq!(db3.list_memories().len(), 1);
        assert_eq!(db3.list_memories()[0].content, "short");
        assert_eq!(db3.create_memory("global", "", "c", "next").unwrap().id, 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_data_is_imported_once_and_left_untouched() {
        let legacy = temp_root("legacy-src");
        let root = temp_root("legacy-dst");

        // Build a legacy `data/` tree by hand.
        let run = sample_run("run-old");
        let dir = legacy.join("sessions").join("run-old");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("run.json"), serde_json::to_string_pretty(&run).unwrap()).unwrap();
        let m1 = RunMessage {
            created_at: "2026-03-07T10:21:00.000Z".into(),
            ..msg("run-old", "m1", "user", "old prompt")
        };
        let m2 = RunMessage {
            created_at: "2026-03-07T10:22:00.000Z".into(),
            ..msg("run-old", "m2", "assistant", "old answer")
        };
        let jsonl = [&m1, &m2]
            .iter()
            .map(|m| serde_json::to_string(m).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.join("messages.jsonl"), format!("{jsonl}\n")).unwrap();
        let plan = Plan {
            id: "plan-old".into(),
            run_id: "run-old".into(),
            title: "Old plan".into(),
            tasks: json!([]),
            version: 1,
            ..Default::default()
        };
        std::fs::write(dir.join("plan.json"), serde_json::to_string(&plan).unwrap()).unwrap();
        let usage = |id: i64, run_id: &str| UsageLogEntry {
            id,
            run_id: run_id.into(),
            provider_id: "p".into(),
            model: "m".into(),
            input_tokens: 7,
            ..Default::default()
        };
        std::fs::write(
            legacy.join("usage_logs.jsonl"),
            format!(
                "{}\n{}\n{}\n",
                serde_json::to_string(&usage(1, "run-old")).unwrap(),
                serde_json::to_string(&usage(2, "run-gone")).unwrap(),
                serde_json::to_string(&usage(3, "run-old")).unwrap(),
            ),
        )
        .unwrap();
        let memory = Memory {
            id: 5,
            scope: "project".into(),
            project_path: "C:/ws".into(),
            category: "reference".into(),
            content: "legacy fact: with colon".into(),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-02T00:00:00.000Z".into(),
        };
        std::fs::write(legacy.join("memory.json"), serde_json::to_string(&vec![memory.clone()]).unwrap()).unwrap();
        std::fs::write(
            legacy.join("projects.json"),
            serde_json::to_string(&vec![Project {
                path: "C:/ws".into(),
                name: "WS".into(),
                created_at: now_iso(),
            }])
            .unwrap(),
        )
        .unwrap();
        std::fs::write(legacy.join("permissions.json"), "[]").unwrap();

        let snapshot = |dir: &Path| -> Vec<(PathBuf, Vec<u8>)> {
            let mut out = Vec::new();
            fn walk(dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
                for entry in std::fs::read_dir(dir).unwrap().flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        walk(&path, out);
                    } else {
                        out.push((path.clone(), std::fs::read(&path).unwrap()));
                    }
                }
            }
            walk(dir, &mut out);
            out.sort();
            out
        };
        let before = snapshot(&legacy);

        let db = Db::open_with_legacy(&root, Some(&legacy)).unwrap();
        assert_eq!(snapshot(&legacy), before, "legacy dir is never modified");
        assert!(root.join(MIGRATION_MARKER).exists());
        assert_eq!(db.get_run_full("run-old").unwrap(), run);
        let messages = db.list_messages("run-old");
        assert_eq!(messages, vec![m1.clone(), m2.clone()]);
        assert_eq!(db.get_plan("run-old").unwrap().title, "Old plan");
        let usage_ids: Vec<i64> = db.list_usage(None).iter().map(|u| u.id).collect();
        assert_eq!(usage_ids, [1, 3], "usage of vanished runs is dropped");
        assert_eq!(db.list_memories(), vec![memory.clone()]);
        assert_eq!(db.list_projects()[0].name, "WS");
        let history: Vec<Value> = read_jsonl_or(&root.join(HISTORY_FILE));
        assert_eq!(history.len(), 1);
        assert_eq!(history[0]["text"], "old prompt");
        assert_eq!(history[0]["ts"], unix_secs("2026-03-07T10:21:00.000Z"));
        let index: Vec<Value> = read_jsonl_or(&root.join(SESSION_INDEX_FILE));
        assert_eq!(index.len(), 1);
        assert_eq!(index[0]["id"], "run-old");
        // Fresh ids continue after the imported ones.
        assert_eq!(db.append_usage(usage(0, "run-old")).id, 4);
        assert_eq!(db.create_memory("global", "", "c", "new").unwrap().id, 6);
        drop(db);

        // Second open does not import again (even after changing the legacy dir).
        std::fs::write(legacy.join("memory.json"), "[]").unwrap();
        let db2 = Db::open_with_legacy(&root, Some(&legacy)).unwrap();
        assert_eq!(db2.list_memories().len(), 2);
        assert_eq!(db2.list_runs().len(), 1);
        assert_eq!(rollouts_in(&root).len(), 1);
        let history: Vec<Value> = read_jsonl_or(&root.join(HISTORY_FILE));
        assert_eq!(history.len(), 1);

        // A plain `open` on a custom root never looks at the real app data.
        assert!(default_legacy_dir_for(&root).is_none());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&legacy);
    }

    /// Two `Db` instances on one root (desktop + `lcgn`) must not overwrite
    /// each other's shared files: mutations re-read them from disk under the
    /// advisory lock before applying the change.
    #[test]
    fn two_instances_interleave_without_losing_writes() {
        let root = temp_root("cross");
        let db1 = Db::open(&root).unwrap();
        let db2 = Db::open(&root).unwrap();

        db1.create_project("C:/a", "A");
        db2.create_project("C:/b", "B");
        db1.create_memory("project", "C:/a", "reference", "one").unwrap();
        db2.create_memory("project", "C:/b", "reference", "two").unwrap();
        let p1 = db1.upsert_permission("run", "C:/a", "run_command", "git status", vec![], "allow_once");
        let p2 = db2.upsert_permission("run", "C:/b", "run_command", "git push", vec![], "allow_once");
        let u1 = db1.append_usage(UsageLogEntry {
            run_id: "r1".into(),
            provider_id: "p".into(),
            model: "m".into(),
            ..Default::default()
        });
        let u2 = db2.append_usage(UsageLogEntry {
            run_id: "r2".into(),
            provider_id: "p".into(),
            model: "m".into(),
            ..Default::default()
        });

        // Fresh instance sees everything both wrote, with no id collisions.
        let db3 = Db::open(&root).unwrap();
        let paths: Vec<String> = db3.list_projects().iter().map(|p| p.path.clone()).collect();
        assert!(paths.contains(&"C:/a".to_string()) && paths.contains(&"C:/b".to_string()));
        let memories = db3.list_memories();
        assert_eq!(memories.len(), 2);
        assert!(memories.iter().any(|m| m.content == "one"));
        assert!(memories.iter().any(|m| m.content == "two"));
        assert_eq!(db3.list_permissions().len(), 2);
        assert_ne!(p1.id, p2.id, "ids must not collide across processes");
        assert_eq!(db3.list_usage(None).len(), 2);
        assert_ne!(u1.id, u2.id, "usage ids must not collide across processes");

        // refresh_runs picks up another instance's runs.
        db1.create_run(sample_run("run-from-1"));
        assert!(db2.get_run_full("run-from-1").is_none());
        db2.refresh_runs();
        assert!(db2.get_run_full("run-from-1").is_some());

        let _ = std::fs::remove_dir_all(&root);
    }

    // ------------------------------------------------------------------
    // QA-034..041 regression tests
    // ------------------------------------------------------------------

    fn active_run(id: &str) -> Run {
        Run {
            status: "generating".into(),
            ..sample_run(id)
        }
    }

    #[test]
    fn deleted_run_is_never_resurrected_by_late_writes() {
        let root = temp_root("tombstone");
        let db = Db::open(&root).unwrap();
        db.create_run(active_run("run-del"));
        db.append_message(msg("run-del", "m1", "user", "hi"));
        assert_eq!(rollouts_in(&root).len(), 1);

        assert!(db.delete_run("run-del"));
        assert!(rollouts_in(&root).is_empty());

        // A drive task that is still unwinding keeps writing.
        db.append_message(msg("run-del", "m2", "assistant", "late"));
        db.update_run_status("run-del", "done", None);
        db.set_plan(Plan {
            run_id: "run-del".into(),
            ..Default::default()
        });
        db.append_usage(UsageLogEntry {
            run_id: "run-del".into(),
            ..Default::default()
        });
        assert!(db.stream_message("run-del", "m2", "x", None).is_none());
        assert!(rollouts_in(&root).is_empty(), "no headless rollout may reappear");
        assert!(db.list_messages("run-del").is_empty());
        assert!(db.list_usage(Some("run-del")).is_empty());
        assert!(db.get_plan("run-del").is_none());

        // A reopen shows nothing either.
        drop(db);
        assert!(Db::open(&root).unwrap().list_runs().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn streaming_updates_are_coalesced_but_the_final_text_persists() {
        let root = temp_root("stream");
        let db = Db::open(&root).unwrap();
        db.create_run(active_run("run-s"));
        db.append_message(msg("run-s", "a1", "assistant", ""));
        let mut text = String::new();
        for chunk in 0..200 {
            text.push_str(&format!("chunk {chunk} "));
            let live = db.stream_message("run-s", "a1", &text, Some("thinking")).unwrap();
            assert_eq!(live.content, text, "memory is always current");
        }
        assert_eq!(db.list_messages("run-s")[0].content, text);
        let streaming_lines = kinds_of(&rollouts_in(&root)[0])
            .iter()
            .filter(|kind| *kind == "message_update")
            .count();
        assert_eq!(streaming_lines, 0, "intermediate states stay in memory");

        db.update_message_full("run-s", "a1", &text, Some("thinking"), None);
        let kinds = kinds_of(&rollouts_in(&root)[0]);
        assert_eq!(kinds.iter().filter(|kind| *kind == "message_update").count(), 1);

        drop(db);
        let reopened = Db::open(&root).unwrap();
        assert_eq!(reopened.list_messages("run-s")[0].content, text);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn startup_leaves_runs_of_a_live_process_alone() {
        let root = temp_root("owner");
        let owner = Db::open(&root).unwrap();
        owner.create_run(active_run("run-live"));

        // A second process starts while the first still drives the run.
        let other = Db::open(&root).unwrap();
        other.run_startup_tasks(Path::new("C:/ws"), "Locagens");
        assert_eq!(other.get_run_full("run-live").unwrap().status, "generating");
        assert!(other.list_messages("run-live").is_empty(), "no restart message injected");

        // The owner finishes: the claim is released, nothing is stuck.
        owner.update_run_status("run-live", "done", None);
        assert!(!run_lock_path(&root, "run-live").exists());

        // An owner that died mid-run (lock released with the process) is failed.
        owner.create_run(active_run("run-dead"));
        drop(owner);
        let third = Db::open(&root).unwrap();
        third.run_startup_tasks(Path::new("C:/ws"), "Locagens");
        assert_eq!(third.get_run_full("run-dead").unwrap().status, "failed");
        assert_eq!(third.get_run_full("run-live").unwrap().status, "done");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_panic_inside_a_closure_does_not_kill_the_store() {
        let root = temp_root("poison");
        let db = Db::open(&root).unwrap();
        db.create_run(sample_run("run-p"));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            db.patch_run("run-p", |_| panic!("boom"));
        }));
        assert!(result.is_err());
        // Every later call still works.
        assert!(db.get_run_full("run-p").is_some());
        db.append_message(msg("run-p", "m", "user", "still alive"));
        assert_eq!(db.list_messages("run-p").len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unopenable_lock_file_degrades_instead_of_panicking() {
        let root = temp_root("nolock");
        // `.lock` is a directory, so it can never be opened as a file.
        std::fs::create_dir_all(root.join(".lock")).unwrap();
        let db = Db::open(&root).unwrap();
        let project = db.create_project("C:/x", "x");
        assert_eq!(project.path, "C:/x");
        assert_eq!(db.list_projects().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn changes_from_another_process_become_visible() {
        let root = temp_root("crossproc");
        let a = Db::open(&root).unwrap();
        let b = Db::open(&root).unwrap();

        // Permissions: a grant revoked by one instance disappears for the other.
        let rule = a.upsert_permission("global", "", "run_command", "ls", vec![], "allow_always");
        assert_eq!(b.list_permissions().len(), 1);
        a.delete_permission(rule.id);
        assert!(b.list_permissions().is_empty());

        // Projects and memories likewise.
        a.create_project("C:/shared", "shared");
        assert!(b.get_project("C:/shared").is_some());
        let memory = a.create_memory("global", "", "ref", "remember me").unwrap();
        assert_eq!(b.list_memories().len(), 1);
        a.update_memory(memory.id, "edited");
        assert_eq!(b.list_memories()[0].content, "edited");
        a.delete_memory(memory.id);
        assert!(b.list_memories().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refresh_runs_is_incremental_and_spares_runs_this_process_drives() {
        let root = temp_root("refresh");
        let a = Db::open(&root).unwrap();
        let b = Db::open(&root).unwrap();

        // b is driving a run with live streaming state.
        b.create_run(active_run("run-mine"));
        b.append_message(msg("run-mine", "am", "assistant", ""));
        b.stream_message("run-mine", "am", "streamed so far", None);

        // a finishes one run, then continues it with another message.
        a.create_run(sample_run("run-theirs"));
        b.refresh_runs();
        assert!(b.get_run_full("run-theirs").is_some());
        assert!(b.list_messages("run-theirs").is_empty());
        a.append_message(msg("run-theirs", "t1", "user", "more"));
        b.refresh_runs();
        assert_eq!(b.list_messages("run-theirs").len(), 1);

        // The run b drives was not reverted to its (stale) on-disk state.
        assert_eq!(b.list_messages("run-mine")[0].content, "streamed so far");

        // A run deleted by another process disappears.
        a.delete_run("run-theirs");
        b.refresh_runs();
        assert!(b.get_run_full("run-theirs").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_session_runs_leave_no_index_or_history_and_are_swept_at_startup() {
        let root = temp_root("ephemeral");
        let db = Db::open(&root).unwrap();
        db.create_run(Run {
            ephemeral: true,
            ..active_run("run-eph")
        });
        db.append_message(msg("run-eph", "u1", "user", "secret prompt"));
        db.update_run_status("run-eph", "generating", None);
        assert!(!root.join(SESSION_INDEX_FILE).exists());
        assert!(!root.join(HISTORY_FILE).exists());

        // A crash left it behind: the next startup removes it entirely.
        drop(db);
        let reopened = Db::open(&root).unwrap();
        assert!(reopened.get_run_full("run-eph").is_some());
        reopened.run_startup_tasks(Path::new("C:/ws"), "Locagens");
        assert!(reopened.get_run_full("run-eph").is_none());
        assert!(rollouts_in(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn usage_ids_stay_unique_without_reloading_shared_files() {
        let root = temp_root("usage");
        let a = Db::open(&root).unwrap();
        let b = Db::open(&root).unwrap();
        let mut ids = HashSet::new();
        for i in 0..10 {
            let (db, run) = if i % 2 == 0 { (&a, "ra") } else { (&b, "rb") };
            let entry = db.append_usage(UsageLogEntry {
                run_id: run.into(),
                ..Default::default()
            });
            assert!(ids.insert(entry.id), "duplicate usage id {}", entry.id);
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
