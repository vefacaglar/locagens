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

/// Streamed message text is checkpointed to the rollout at most this often.
const STREAM_CHECKPOINT_INTERVAL: Duration = Duration::from_secs(5);
/// `refresh_runs_throttled` re-scans the rollouts at most this often.
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

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

/// An image pasted into a user message: its MIME type (`image/png`, ...) and
/// the base64 bytes (no `data:` prefix).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ImageAttachment {
    pub mime: String,
    pub data: String,
}

/// One chat message of a run, persisted as a `message` / `message_update` rollout line
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
    /// Anthropic thinking blocks (with signatures) of an assistant message,
    /// replayed to the provider on later rounds and turns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_blocks: Option<Value>,
    /// Images attached to a user message (pasted into the composer).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageAttachment>,
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
    /// `main` for the main agent, else the sub-agent role. Legacy `planner`
    /// lines read back as `main`.
    #[serde(deserialize_with = "deserialize_usage_role")]
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

/// Usage-log role of the main agent.
pub const MAIN_AGENT_ROLE: &str = "main";
/// Main-agent role written by versions before `main`.
const LEGACY_MAIN_AGENT_ROLE: &str = "planner";

impl UsageLogEntry {
    /// The entry's role; entries without one belong to the main agent.
    pub fn role(&self) -> &str {
        self.agent_role.as_deref().unwrap_or(MAIN_AGENT_ROLE)
    }
}

fn deserialize_usage_role<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    let role = Option::<String>::deserialize(deserializer)?;
    Ok(role.map(|role| if role == LEGACY_MAIN_AGENT_ROLE { MAIN_AGENT_ROLE.to_string() } else { role }))
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
                    append_event(&root, state, &run_id, &now, "run_update", run_update_payload(Some(&run), &run));
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

mod legacy_import;
mod locks;
mod memories;
mod messages;
mod permissions;
mod prompt_history;
mod projects;
mod rollout;
mod runs;
#[cfg(test)]
mod store_tests;
mod sync;
#[cfg(test)]
mod tests;
mod usage;

use self::legacy_import::*;
use self::locks::*;
use self::memories::*;
pub(crate) use self::memories::sanitize_project_path;
use self::messages::*;

use self::projects::*;
use self::rollout::*;
use self::runs::*;
use self::sync::*;
use self::usage::*;
