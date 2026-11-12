//! Pure file-based storage layer (Codex/Claude/Pi-style JSON + JSONL) — zero
//! SQLite, zero C library dependency. Replaces the former `rusqlite`
//! (`locagens.db`) implementation while keeping the same public method
//! signatures so route modules compile unchanged.
//!
//! Layout under the data root (`LOCAGENS_DATA_DIR`, the `LOCAGENS_DB_PATH`
//! parent/stem, or `<app_support_dir>/data`):
//!
//! ```txt
//! projects.json              — list of Project
//! permissions.json           — list of PermissionRule (auto-increment ids)
//! memory.json                — list of Memory (auto-increment ids)
//! usage_logs.jsonl           — append-only JSONL of UsageLogEntry
//! sessions/<run_id>/run.json       — Run metadata
//! sessions/<run_id>/messages.jsonl — append-only JSONL of RunMessage
//! sessions/<run_id>/plan.json      — active Plan for the run
//! ```
//!
//! All state is kept in an in-memory `RwLock<StorageState>` (O(1) reads) and
//! every mutation is durably persisted: JSON files are written to a `.tmp`
//! sibling then renamed atomically; JSONL files are appended + flushed, or
//! rewritten atomically when a line is updated in place.

use crate::util::now_iso;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Held for the duration of one cross-process mutation of a shared file.
/// Backed by an advisory exclusive lock on `<data_root>/.lock` so the
/// backend server and `lcgn` can run concurrently on the same data dir
/// without overwriting each other's `projects.json` / `permissions.json` /
/// `memory.json` / `usage_logs.jsonl`. Per-run files under `sessions/` are
/// not locked (they are private to the run that owns them).
struct CrossProcessLock(std::fs::File);

impl Drop for CrossProcessLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// A durable fact remembered via the `remember` tool / Settings → Memory.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
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
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
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
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
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
    pub coder_provider_id: Option<String>,
    pub coder_model: Option<String>,
}

/// Full run metadata, persisted as `sessions/<run_id>/run.json`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
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
    pub coder_provider_id: Option<String>,
    pub coder_model: Option<String>,
    pub coder_reasoning_effort: Option<String>,
    pub agent_preset: Option<String>,
    pub utility_provider_id: Option<String>,
    pub utility_model: Option<String>,
    pub utility_reasoning_effort: Option<String>,
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

/// One chat message of a run, persisted as a line of
/// `sessions/<run_id>/messages.jsonl`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
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

/// The active plan of a run, persisted as `sessions/<run_id>/plan.json`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
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

/// One usage entry, persisted as a line of `usage_logs.jsonl`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
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

/// Everything the backend stores, mirrored in memory for O(1) reads.
#[derive(Default)]
struct StorageState {
    projects: Vec<Project>,
    permissions: Vec<PermissionRule>,
    memories: Vec<Memory>,
    runs: BTreeMap<String, Run>,
    messages: HashMap<String, Vec<RunMessage>>,
    plans: HashMap<String, Plan>,
    usage: Vec<UsageLogEntry>,
    next_permission_id: i64,
    next_memory_id: i64,
    next_usage_id: i64,
}

pub struct Db {
    root: PathBuf,
    state: RwLock<StorageState>,
}

const ACTIVE_RUN_STATUSES: [&str; 4] =
    ["created", "generating", "awaiting_permission", "awaiting_input"];

impl Db {
    /// Opens (creating if needed) the file-based store rooted at `data_root`.
    pub fn open(data_root: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_root)?;
        let state = load_state(data_root)?;
        Ok(Self {
            root: data_root.to_path_buf(),
            state: RwLock::new(state),
        })
    }

    /// The data root directory (JSON/JSONL files live here).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Takes the advisory cross-process lock (`<root>/.lock`, blocking).
    fn cross_process_lock(&self) -> CrossProcessLock {
        let path = self.root.join(".lock");
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(&path)
            .unwrap_or_else(|err| panic!("Failed to open lock file {}: {err}", path.display()));
        if let Err(err) = file.lock_exclusive() {
            tracing::warn!("[Database] Advisory lock not taken on {}: {err}", path.display());
        }
        CrossProcessLock(file)
    }

    /// Runs `mutate` under the cross-process lock after re-reading the
    /// shared files from disk, so mutations that raced with another process
    /// are applied on top of the latest on-disk state instead of clobbering
    /// it. The closure is expected to persist the files it changed.
    fn with_shared_mutation<T>(&self, mutate: impl FnOnce(&mut StorageState) -> T) -> T {
        let _lock = self.cross_process_lock();
        let mut state = self.write();
        reload_shared_files(&self.root, &mut state);
        mutate(&mut state)
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

        // Reset any runs left in active states by the previous session.
        let stuck: Vec<String> = state
            .runs
            .values()
            .filter(|run| ACTIVE_RUN_STATUSES.contains(&run.status.as_str()))
            .map(|run| run.id.clone())
            .collect();

        for run_id in stuck {
            let now = now_iso();
            if let Some(run) = state.runs.get_mut(&run_id) {
                run.status = "failed".to_string();
                run.error_message = Some("Session interrupted due to server restart.".to_string());
                run.updated_at = now.clone();
                persist_run(&root, run);
            }
            let message = RunMessage {
                id: format!("msg-err-restart-{}-{}", run_id, chrono::Utc::now().timestamp_millis()),
                run_id: run_id.clone(),
                role: "system".to_string(),
                content: "Session interrupted due to server restart.".to_string(),
                created_at: now,
                ..Default::default()
            };
            let messages = state.messages.entry(run_id.clone()).or_default();
            messages.push(message.clone());
            append_jsonl(&root.join("sessions").join(&run_id).join("messages.jsonl"), &message);
        }
        })
    }

    // ------------------------------------------------------------------
    // Projects
    // ------------------------------------------------------------------

    pub fn list_projects(&self) -> Vec<Project> {
        self.read().projects.clone()
    }

    pub fn get_project(&self, path: &str) -> Option<Project> {
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
    // Memories
    // ------------------------------------------------------------------

    pub fn list_memories(&self) -> Vec<Memory> {
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
            state.next_memory_id += 1;
            state.memories.push(memory.clone());
            persist_memories(&self.root, &state.memories);
            Ok(memory)
        })
    }

    pub fn update_memory(&self, id: i64, content: &str) -> Option<Memory> {
        self.with_shared_mutation(|state| {
            let updated = state
                .memories
                .iter_mut()
                .find(|memory| memory.id == id)
                .map(|memory| {
                    memory.content = content.to_string();
                    memory.updated_at = now_iso();
                    memory.clone()
                });
            if updated.is_some() {
                persist_memories(&self.root, &state.memories);
            }
            updated
        })
    }

    pub fn delete_memory(&self, id: i64) -> bool {
        self.with_shared_mutation(|state| {
            let before = state.memories.len();
            state.memories.retain(|memory| memory.id != id);
            if state.memories.len() != before {
                persist_memories(&self.root, &state.memories);
                true
            } else {
                false
            }
        })
    }

    pub fn clear_memories(&self) {
        self.with_shared_mutation(|state| {
            if !state.memories.is_empty() {
                state.memories.clear();
                persist_memories(&self.root, &state.memories);
            }
        })
    }

    // ------------------------------------------------------------------
    // Permissions
    // ------------------------------------------------------------------

    pub fn list_permissions(&self) -> Vec<PermissionRule> {
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

    pub fn create_run(&self, run: Run) -> Run {
        let mut state = self.write();
        state.runs.insert(run.id.clone(), run.clone());
        persist_run(&self.root, &run);
        run
    }

    pub fn update_run_status(&self, id: &str, status: &str, error_message: Option<&str>) {
        let mut state = self.write();
        if let Some(run) = state.runs.get_mut(id) {
            run.status = status.to_string();
            run.error_message = error_message.map(str::to_string);
            run.updated_at = now_iso();
            run.last_active_at = Some(now_iso());
            persist_run(&self.root, run);
        }
    }

    /// Applies an arbitrary metadata patch to a run (title, model fields,
    /// preset fields, status, …) and persists it atomically.
    pub fn patch_run(&self, id: &str, patch: impl FnOnce(&mut Run)) -> Option<Run> {
        let mut state = self.write();
        let updated = state.runs.get_mut(id).map(|run| {
            patch(run);
            run.updated_at = now_iso();
            run.last_active_at = Some(now_iso());
            run.clone()
        });
        if let Some(run) = updated.as_ref() {
            persist_run(&self.root, run);
        }
        updated
    }

    pub fn touch_run(&self, id: &str) {
        let mut state = self.write();
        if let Some(run) = state.runs.get_mut(id) {
            run.updated_at = now_iso();
            run.last_active_at = Some(now_iso());
            persist_run(&self.root, run);
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
            coder_provider_id: run.coder_provider_id,
            coder_model: run.coder_model,
        })
    }

    pub fn delete_run(&self, id: &str) -> bool {
        let mut state = self.write();
        let removed = state.runs.remove(id).is_some();
        state.messages.remove(id);
        state.plans.remove(id);
        if removed {
            let _ = std::fs::remove_dir_all(self.root.join("sessions").join(id));
        }
        removed
    }

    // ------------------------------------------------------------------
    // Messages
    // ------------------------------------------------------------------

    pub fn append_message(&self, message: RunMessage) {
        let mut state = self.write();
        let messages = state.messages.entry(message.run_id.clone()).or_default();
        messages.push(message.clone());
        append_jsonl(&self.messages_path(&message.run_id), &message);
    }

    pub fn list_messages(&self, run_id: &str) -> Vec<RunMessage> {
        self.read()
            .messages
            .get(run_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Updates a message in the in-memory index, then rewrites
    /// `messages.jsonl` cleanly (tmp + rename).
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
        let mut state = self.write();
        let messages = state.messages.get_mut(run_id)?;
        let updated = messages
            .iter_mut()
            .find(|message| message.id == message_id)
            .map(|message| {
                message.content = content.to_string();
                if let Some(reasoning) = reasoning_content {
                    message.reasoning_content = Some(reasoning.to_string());
                }
                if let Some(raw) = raw_response {
                    message.raw_response = Some(raw.to_string());
                }
                message.clone()
            });
        if let Some(updated) = updated.as_ref() {
            rewrite_jsonl(&self.messages_path(run_id), messages);
            let _ = updated;
        }
        updated
    }

    // ------------------------------------------------------------------
    // Plans
    // ------------------------------------------------------------------

    pub fn get_plan(&self, run_id: &str) -> Option<Plan> {
        self.read().plans.get(run_id).cloned()
    }

    pub fn set_plan(&self, plan: Plan) {
        let mut state = self.write();
        state.plans.insert(plan.run_id.clone(), plan.clone());
        persist_plan(&self.root, &plan);
    }

    // ------------------------------------------------------------------
    // Usage logs
    // ------------------------------------------------------------------

    pub fn append_usage(&self, mut entry: UsageLogEntry) -> UsageLogEntry {
        self.with_shared_mutation(|state| {
            if entry.id == 0 {
                entry.id = state.next_usage_id;
                state.next_usage_id += 1;
            }
            state.usage.push(entry.clone());
            append_jsonl(&self.usage_logs_path(), &entry);
            entry
        })
    }

    /// Re-reads `sessions/` from disk so runs created by another process
    /// (the desktop backend or a second `lcgn`) become visible.
    pub fn refresh_runs(&self) {
        let mut state = self.write();
        state.runs.clear();
        state.messages.clear();
        state.plans.clear();
        load_sessions_from_disk(&self.root, &mut state);
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
        self.state.read().expect("storage lock poisoned")
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, StorageState> {
        self.state.write().expect("storage lock poisoned")
    }

    fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    fn run_dir(&self, run_id: &str) -> PathBuf {
        self.sessions_dir().join(run_id)
    }

    fn messages_path(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("messages.jsonl")
    }

    fn usage_logs_path(&self) -> PathBuf {
        self.root.join("usage_logs.jsonl")
    }
}

// ----------------------------------------------------------------------
// Persistence primitives
// ----------------------------------------------------------------------

/// Atomic JSON write: serialize to a `.tmp` sibling, then rename over the
/// destination so readers never observe a half-written file.
fn write_json_atomic(path: &Path, value: &impl Serialize) {
    if let Err(err) = (|| -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(value)
            .map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    })() {
        tracing::error!("[Database] Failed to persist {}: {err}", path.display());
    }
}

/// Append one JSON line and flush immediately.
fn append_jsonl(path: &Path, value: &impl Serialize) {
    if let Err(err) = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let mut line = serde_json::to_string(value).map_err(std::io::Error::other)?;
        line.push('\n');
        file.write_all(line.as_bytes())?;
        file.flush()?;
        Ok(())
    })() {
        tracing::error!("[Database] Failed to append {}: {err}", path.display());
    }
}

/// Rewrite a JSONL file atomically from the in-memory list (used when a line
/// is updated in place).
fn rewrite_jsonl<T: Serialize>(path: &Path, items: &[T]) {
    if let Err(err) = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("jsonl.tmp");
        let mut file = std::fs::File::create(&tmp)?;
        for item in items {
            let mut line = serde_json::to_string(item).map_err(std::io::Error::other)?;
            line.push('\n');
            file.write_all(line.as_bytes())?;
        }
        file.flush()?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    })() {
        tracing::error!("[Database] Failed to rewrite {}: {err}", path.display());
    }
}

fn persist_projects(root: &Path, projects: &[Project]) {
    write_json_atomic(&root.join("projects.json"), &projects);
}

fn persist_permissions(root: &Path, permissions: &[PermissionRule]) {
    write_json_atomic(&root.join("permissions.json"), &permissions);
}

fn persist_memories(root: &Path, memories: &[Memory]) {
    write_json_atomic(&root.join("memory.json"), &memories);
}

fn persist_run(root: &Path, run: &Run) {
    let dir = root.join("sessions").join(&run.id);
    if let Err(err) = std::fs::create_dir_all(&dir) {
        tracing::error!("[Database] Failed to create {}: {err}", dir.display());
        return;
    }
    write_json_atomic(&dir.join("run.json"), run);
}

fn persist_plan(root: &Path, plan: &Plan) {
    let dir = root.join("sessions").join(&plan.run_id);
    if let Err(err) = std::fs::create_dir_all(&dir) {
        tracing::error!("[Database] Failed to create {}: {err}", dir.display());
        return;
    }
    write_json_atomic(&dir.join("plan.json"), plan);
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
    std::fs::read_to_string(path)
        .map(|raw| {
            raw.lines()
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn load_state(root: &Path) -> anyhow::Result<StorageState> {
    let mut state = StorageState {
        projects: read_json_or::<Vec<Project>>(&root.join("projects.json")),
        permissions: read_json_or::<Vec<PermissionRule>>(&root.join("permissions.json")),
        memories: read_json_or::<Vec<Memory>>(&root.join("memory.json")),
        usage: read_jsonl_or::<UsageLogEntry>(&root.join("usage_logs.jsonl")),
        ..Default::default()
    };

    state.next_permission_id = state
        .permissions
        .iter()
        .map(|rule| rule.id)
        .max()
        .unwrap_or(0)
        + 1;
    state.next_memory_id = state.memories.iter().map(|m| m.id).max().unwrap_or(0) + 1;
    state.next_usage_id = state.usage.iter().map(|u| u.id).max().unwrap_or(0) + 1;

    load_sessions_from_disk(root, &mut state);

    Ok(state)
}

/// Scans `sessions/` into `state`, replacing whatever was loaded before
/// (used at startup and by `refresh_runs`).
fn load_sessions_from_disk(root: &Path, state: &mut StorageState) {
    let sessions_dir = root.join("sessions");
    if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let run_id = entry.file_name().to_string_lossy().to_string();
            let run: Run = read_json_or(&entry.path().join("run.json"));
            if !run.id.is_empty() {
                state.runs.insert(run.id.clone(), run);
            }
            // Messages and plans load even without run.json so orphaned
            // session folders keep their content.
            state
                .messages
                .insert(run_id.clone(), read_jsonl_or::<RunMessage>(&entry.path().join("messages.jsonl")));
            if let Some(plan) = load_optional_plan(&entry.path().join("plan.json")) {
                state.plans.insert(run_id.clone(), plan);
            }
        }
    }
}

fn load_optional_plan(path: &Path) -> Option<Plan> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Re-reads the cross-process-shared files from disk into `state` (used by
/// `with_shared_mutation`, under the advisory lock, so a mutation always
/// applies on top of the latest on-disk state written by another process).
fn reload_shared_files(root: &Path, state: &mut StorageState) {
    state.projects = read_json_or::<Vec<Project>>(&root.join("projects.json"));
    state.permissions = read_json_or::<Vec<PermissionRule>>(&root.join("permissions.json"));
    state.memories = read_json_or::<Vec<Memory>>(&root.join("memory.json"));
    state.usage = read_jsonl_or::<UsageLogEntry>(&root.join("usage_logs.jsonl"));
    state.next_permission_id = state
        .permissions
        .iter()
        .map(|rule| rule.id)
        .max()
        .unwrap_or(0)
        + 1;
    state.next_memory_id = state.memories.iter().map(|m| m.id).max().unwrap_or(0) + 1;
    state.next_usage_id = state.usage.iter().map(|u| u.id).max().unwrap_or(0) + 1;
}

/// Resolve the storage data root: `LOCAGENS_DATA_DIR`, else the
/// `LOCAGENS_DB_PATH` parent joined with its file stem, else
/// `<app_support_dir>/data`.
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
    crate::paths::app_support_dir().join("data")
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
            id: "run-1".into(),
            title: "Test".into(),
            status: "generating".into(),
            provider_id: "opencode-go".into(),
            provider_display_name: "OpenCode".into(),
            model: "glm-5.3-flash".into(),
            created_at: now_iso(),
            updated_at: now_iso(),
            ..Default::default()
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
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn messages_append_update_and_usage() {
        let root = temp_root("messages");
        let db = Db::open(&root).unwrap();
        let message = RunMessage {
            id: "m1".into(),
            run_id: "run-x".into(),
            role: "assistant".into(),
            content: "first".into(),
            created_at: now_iso(),
            ..Default::default()
        };
        db.append_message(message.clone());
        db.append_message(RunMessage {
            id: "m2".into(),
            run_id: "run-x".into(),
            role: "user".into(),
            content: "second".into(),
            created_at: now_iso(),
            ..Default::default()
        });
        let updated = db.update_message("run-x", "m1", "rewritten", Some("thinking"));
        assert_eq!(updated.unwrap().content, "rewritten");

        drop(db);
        let db2 = Db::open(&root).unwrap();
        let messages = db2.list_messages("run-x");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "rewritten");
        assert_eq!(messages[0].reasoning_content.as_deref(), Some("thinking"));

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

    /// Two `Db` instances on one data dir (backend server + `lcgn`) must not
    /// overwrite each other's shared files: mutations re-read the files from
    /// disk under the advisory lock before applying the change.
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

        let _ = std::fs::remove_dir_all(&root);
    }
}
