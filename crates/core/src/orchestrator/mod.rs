//! Pure Rust agent orchestrator.
//!
//! Owns the per-run handles (cancellation token, SSE broadcast channel,
//! mid-run message inbox, pending permission/question coordinators) and the
//! `AgentLoop` that drives model ↔ tool rounds for the main agent and every
//! spawned sub-agent (explorer / worker).
//!
//! Message flow:
//! ```txt
//! drive_run → seed history from messages.jsonl → AgentLoop::run
//!   round: stream LLM deltas → assistant RunMessage (+ throttled SSE)
//!          → execute tool calls (permission-gated) → tool RunMessages
//!          → fold queued mid-run user messages → next round
//! terminal: status_changed + run_completed / run_failed
//! ```

pub mod agent_roles;
pub mod agents;
pub mod agents_md;
pub mod apply_patch;
pub mod command_safety;
pub mod compact;
pub mod context_remaining;
pub mod env_context;
mod hook_calls;
pub mod host_executable;
pub mod llm;
pub mod network_access;
pub mod patch_progress;
pub mod permission_decision;
pub mod review;
pub mod tool_search;
pub mod tools;
pub mod turn_diff_tracker;

pub use turn_diff_tracker::TurnDiffTracker;

#[cfg(test)]
mod lifecycle_tests;

use crate::db::{Plan, RunMessage, UsageLogEntry};
use permission_decision::PermissionDecision;
use crate::pi_providers::PiProviderManager;
use crate::AppState;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, watch};

const MAX_ROUNDS: usize = 40;
const SUB_AGENT_MAX_ROUNDS: usize = 24;
/// SSE throttling for `message_updated` while tokens stream in.
const STREAM_EMIT_INTERVAL: Duration = Duration::from_millis(60);
/// Persistence throttling for streaming `messages.jsonl` rewrites.
const STREAM_PERSIST_INTERVAL: Duration = Duration::from_millis(250);

const TERMINAL_STATUSES: [&str; 3] = ["done", "failed", "cancelled"];

/// Workspace/dangerous tools executed through the gated executor.
const WORKSPACE_TOOLS: [&str; 10] = [
    "read_file",
    "write_file",
    "edit_file",
    "delete_file",
    "list_directory",
    "create_directory",
    "move_file",
    "search_files",
    "run_command",
    "fetch_url",
];

/// Full (worker) sub-agent toolset: workspace + dangerous tools + the Codex
/// patch/exec harness.
fn worker_tool_defs() -> Vec<Value> {
    let mut names: Vec<&str> = WORKSPACE_TOOLS.to_vec();
    names.extend_from_slice(&["apply_patch", "exec_command", "write_stdin"]);
    tools::filter_tool_defs(&names)
}

/// Main-agent tool names for the run's mode: `chat` exposes only read-only
/// bookkeeping tools, `plan` adds planning/research/delegation tools, and
/// every other mode gets the full toolset.
fn tool_names_for_mode(mode: &str) -> Vec<&'static str> {
    let chat_tools: &[&str] = &[
        "read_file", "list_directory", "search_files", "view_image",
        "set_chat_title", "remember", "load_skill", "compact_context",
        "ask_user_question", "request_user_input",
        "list_mcp_resources", "read_mcp_resource", "get_context_remaining", "host_executable",
    ];
    match mode {
        "chat" => chat_tools.to_vec(),
        "plan" => {
            let mut names: Vec<&str> = chat_tools.to_vec();
            names.extend_from_slice(&[
                "update_plan", "fetch_url", "spawn_agent", "send_message", "followup_task", "wait_agent", "interrupt_agent",
                "close_agent", "list_agents", "spawn_agents",
            ]);
            names
        }
        _ => {
            let mut names: Vec<&str> = WORKSPACE_TOOLS.to_vec();
            names.extend_from_slice(&[
                "set_chat_title", "update_plan", "remember", "load_skill", "ask_user_question",
                "apply_patch", "exec_command", "write_stdin", "view_image", "request_user_input",
                "compact_context", "spawn_agent", "send_message", "followup_task", "wait_agent", "interrupt_agent", "close_agent",
                "list_agents", "list_mcp_resources", "read_mcp_resource", "get_context_remaining", "host_executable",
            ]);
            names.push("spawn_agents");
            names
        }
    }
}

// ----------------------------------------------------------------------
// Per-run coordination
// ----------------------------------------------------------------------

/// Snapshot of the tool call awaiting a permission decision.
pub struct PendingPermission {
    pub tool_call: Value,
    pub preview: Option<Value>,
}

pub struct PermissionSlot {
    pub pending: Option<PendingPermission>,
    pub sender: Option<oneshot::Sender<String>>,
    /// Id of the prompt that owns the slot (see [`RunHandle::ask_permission`]),
    /// so a late cleanup never clears a newer prompt.
    prompt_id: u64,
}

impl Default for PermissionSlot {
    fn default() -> Self {
        Self { pending: None, sender: None, prompt_id: 0 }
    }
}

/// One interactive permission prompt for [`RunHandle::ask_permission`].
pub(crate) struct PermissionPrompt {
    pub tool_call: Value,
    pub preview: Option<Value>,
    pub agent_role: Option<String>,
    pub agent_name: Option<String>,
    /// When set, the prompt is skipped (`None`) if the flag dropped to false
    /// while it waited for its turn (a network prompt whose tool call ended).
    pub armed: Option<Arc<AtomicBool>>,
}

/// Clears a permission prompt's slot and status however its future ends —
/// answered, cancelled, or dropped mid-wait because the sub-agent was
/// closed/aborted (QA-029).
struct PromptCleanup {
    handle: Arc<RunHandle>,
    db: Arc<crate::db::Db>,
    prompt_id: u64,
}

impl PromptCleanup {
    fn clear(slot: &mut PermissionSlot, prompt_id: u64) {
        if slot.prompt_id == prompt_id {
            slot.pending = None;
            slot.sender = None;
        }
    }
}

impl Drop for PromptCleanup {
    fn drop(&mut self) {
        let prompt_id = self.prompt_id;
        match self.handle.permission.try_lock() {
            Ok(mut slot) => Self::clear(&mut slot, prompt_id),
            Err(_) => {
                // Someone holds the slot for an instant: clear it from a task.
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    let handle = self.handle.clone();
                    runtime.spawn(async move { Self::clear(&mut *handle.permission.lock().await, prompt_id) });
                }
            }
        }
        // Back to `generating` unless the run is being cancelled (the cancel
        // path records the terminal status itself).
        if !self.handle.is_cancelled() {
            let run_id = self.handle.run_id.clone();
            let reverted = self
                .db
                .get_run_full(&run_id)
                .is_some_and(|run| run.status == "awaiting_permission");
            if reverted {
                self.db.patch_run(&run_id, |run| run.status = "generating".into());
                self.handle.emit(json!({ "type": "status_changed", "status": "generating" }));
            }
        }
    }
}

pub struct QuestionSlot {
    /// The `questions` array of the active `ask_user_question` request.
    pub pending: Option<Value>,
    /// Resolves with `{ selections: string[][], notes: string[] }`.
    pub sender: Option<oneshot::Sender<Value>>,
}

impl Default for QuestionSlot {
    fn default() -> Self {
        Self { pending: None, sender: None }
    }
}

/// Shared coordination state of ONE drive of a run (a turn), handed to the
/// facade and the agent loop. Every drive gets a fresh handle: cancellation
/// is final for a handle and is never reset, so a cancelled loop that is
/// still unwinding can never be revived by the next turn (QA-023).
pub struct RunHandle {
    pub run_id: String,
    cancel_flag: Arc<AtomicBool>,
    cancelled_tx: watch::Sender<bool>,
    pub events: tokio::sync::broadcast::Sender<Value>,
    pub inbox: tokio::sync::Mutex<VecDeque<String>>,
    pub permission: tokio::sync::Mutex<PermissionSlot>,
    pub question: tokio::sync::Mutex<QuestionSlot>,
    /// Tools approved with "Allow for Run" (per tool name, not run-wide).
    allow_run_tools: Mutex<std::collections::HashSet<String>>,
    /// Serializes interactive permission prompts: concurrent sub-agents and
    /// network approvals share the single pending-permission slot, so they
    /// queue here (QA-028).
    permission_gate: tokio::sync::Mutex<()>,
    next_prompt_id: std::sync::atomic::AtomicU64,
    /// Named sub-agents spawned by `spawn_agent` plus the parent mailbox.
    pub agents: agents::AgentHub,
    /// Hosts approved with "Allow for Run" during this drive (QA-032).
    pub(crate) approved_hosts: Mutex<std::collections::HashSet<String>>,
    /// Set when a drive owns this handle (`prepare_drive` / `spawn`).
    claimed: AtomicBool,
    /// `true` once the drive task (and its cleanup) has finished.
    finished_tx: watch::Sender<bool>,
    /// Aborts the drive task (last resort when it does not unwind in time).
    drive_abort: Mutex<Option<tokio::task::AbortHandle>>,
    /// `(project, snapshot id)` of this turn's pre-turn git snapshot until
    /// its post-turn state is recorded (QA-030).
    turn_snapshot: Mutex<Option<(PathBuf, String)>>,
}

impl RunHandle {
    fn new(run_id: &str) -> Arc<Self> {
        let (cancelled_tx, _) = watch::channel(false);
        let (finished_tx, _) = watch::channel(false);
        let (events, _) = tokio::sync::broadcast::channel(512);
        Arc::new(Self {
            run_id: run_id.to_string(),
            cancel_flag: Arc::new(AtomicBool::new(false)),
            cancelled_tx,
            events,
            inbox: tokio::sync::Mutex::new(VecDeque::new()),
            permission: tokio::sync::Mutex::new(PermissionSlot::default()),
            question: tokio::sync::Mutex::new(QuestionSlot::default()),
            allow_run_tools: Mutex::new(Default::default()),
            permission_gate: tokio::sync::Mutex::new(()),
            next_prompt_id: std::sync::atomic::AtomicU64::new(1),
            agents: agents::AgentHub::default(),
            approved_hosts: Mutex::new(std::collections::HashSet::new()),
            claimed: AtomicBool::new(false),
            finished_tx,
            drive_abort: Mutex::new(None),
            turn_snapshot: Mutex::new(None),
        })
    }

    /// Records the post-turn tree of this turn's undo snapshot (once).
    async fn record_turn_end(&self) {
        let Some((project, snapshot_id)) = lock_ignoring_poison(&self.turn_snapshot).take() else { return };
        if let Err(err) = crate::api::projects::record_turn_end_for_path(&project, &snapshot_id).await {
            tracing::warn!("Could not record the post-turn snapshot of run {}: {err}", self.run_id);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }

    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
        self.cancelled_tx.send_replace(true);
    }

    /// Whether this handle's drive has finished (or was never claimed by a
    /// drive: `prepare_drive` / `spawn` claim it).
    pub fn is_finished(&self) -> bool {
        !self.claimed.load(Ordering::SeqCst) || *self.finished_tx.borrow()
    }

    fn mark_finished(&self) {
        self.finished_tx.send_replace(true);
    }

    /// Waits up to `timeout` for the drive to finish; `true` when it did.
    pub async fn wait_finished(&self, timeout: Duration) -> bool {
        if self.is_finished() {
            return true;
        }
        let mut rx = self.finished_tx.subscribe();
        tokio::time::timeout(timeout, async {
            while !*rx.borrow_and_update() {
                if rx.changed().await.is_err() {
                    return;
                }
            }
        })
        .await
        .is_ok()
    }

    /// Aborts the drive task; its supervisor still runs the cleanup.
    fn abort_drive(&self) {
        if let Some(abort) = lock_ignoring_poison(&self.drive_abort).as_ref() {
            abort.abort();
        }
    }

    /// Shows one permission prompt and waits for the decision. Every prompt
    /// (tool calls of the main agent and sub-agents, network approvals) goes
    /// through here: prompts queue on `permission_gate`, own the slot by id,
    /// and a drop guard clears the slot and restores the status even when
    /// the waiting future is aborted. `None` means cancelled / skipped.
    pub(crate) async fn ask_permission(
        self: &Arc<Self>,
        db: &Arc<crate::db::Db>,
        prompt: PermissionPrompt,
    ) -> Option<String> {
        let gate = tokio::select! {
            gate = self.permission_gate.lock() => gate,
            _ = self.cancelled() => return None,
        };
        if prompt.armed.as_ref().is_some_and(|armed| !armed.load(Ordering::SeqCst)) || self.is_cancelled() {
            return None;
        }
        let prompt_id = self.next_prompt_id.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = oneshot::channel::<String>();
        {
            let mut slot = self.permission.lock().await;
            slot.pending = Some(PendingPermission { tool_call: prompt.tool_call.clone(), preview: prompt.preview.clone() });
            slot.sender = Some(sender);
            slot.prompt_id = prompt_id;
        }
        let cleanup = PromptCleanup { handle: self.clone(), db: db.clone(), prompt_id };
        db.patch_run(&self.run_id, |run| run.status = "awaiting_permission".into());
        self.emit(json!({ "type": "status_changed", "status": "awaiting_permission" }));
        self.emit(json!({
            "type": "permission_requested",
            "runId": self.run_id,
            "toolCall": prompt.tool_call,
            "preview": prompt.preview,
            "agentRole": prompt.agent_role,
            "agentName": prompt.agent_name,
        }));
        let decision = tokio::select! {
            decision = receiver => decision.ok(),
            _ = self.cancelled() => None,
        };
        drop(cleanup);
        drop(gate);
        decision
    }

    /// Completes when cancellation fires (for `tokio::select!`).
    pub async fn cancelled(&self) {
        let mut rx = self.cancelled_tx.subscribe();
        loop {
            if *rx.borrow() {
                return;
            }
            if rx.changed().await.is_err() {
                return;
            }
        }
    }

    pub fn emit(&self, event: Value) {
        // Dropping when no one listens (or the buffer lags) is fine.
        let _ = self.events.send(event);
    }

    /// Records "Allow for Run" for one tool.
    pub fn allow_tool_for_run(&self, tool: &str) {
        self.allow_run_tools.lock().expect("allow_run lock poisoned").insert(tool.to_string());
    }

    /// Whether `tool` was approved with "Allow for Run" earlier in this run.
    pub fn tool_allowed_for_run(&self, tool: &str) -> bool {
        self.allow_run_tools.lock().expect("allow_run lock poisoned").contains(tool)
    }

    /// `{ runId, toolCall, preview }` of the pending permission, if any.
    pub async fn pending_permission(&self) -> Option<Value> {
        let slot = self.permission.lock().await;
        slot.pending.as_ref().map(|pending| {
            json!({
                "runId": self.run_id,
                "toolCall": pending.tool_call,
                "preview": pending.preview,
            })
        })
    }

    /// `{ runId, questions }` of the pending question, if any.
    pub async fn pending_question(&self) -> Option<Value> {
        let slot = self.question.lock().await;
        slot.pending.as_ref().map(|questions| {
            json!({
                "runId": self.run_id,
                "questions": questions,
            })
        })
    }
}

/// How long `cancel_run` / a new turn waits for a cancelled drive to unwind
/// before aborting its task.
pub const DRIVE_UNWIND_GRACE: Duration = Duration::from_secs(5);

/// Registry of live run handles.
pub struct Orchestrator {
    runs: Mutex<HashMap<String, Arc<RunHandle>>>,
    /// Exec sessions per run id. They outlive a `done` turn (a dev server the
    /// agent started stays reachable via `write_stdin` next turn) and are
    /// killed when a turn of the run is cancelled, fails or panics, or when
    /// [`Orchestrator::kill_exec_sessions`] is called (QA-027).
    exec_sessions: Mutex<HashMap<String, std::collections::HashSet<String>>>,
}

impl Default for Orchestrator {
    fn default() -> Self {
        Self::new()
    }
}

fn lock_ignoring_poison<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Orchestrator {
    pub fn new() -> Self {
        Self { runs: Mutex::new(HashMap::new()), exec_sessions: Mutex::new(HashMap::new()) }
    }

    /// The live handle for a run, if the orchestrator knows about it.
    pub fn handle(&self, run_id: &str) -> Option<Arc<RunHandle>> {
        lock_ignoring_poison(&self.runs).get(run_id).cloned()
    }

    /// Gets or creates the handle for a run.
    pub fn ensure_handle(&self, run_id: &str) -> Arc<RunHandle> {
        let mut runs = lock_ignoring_poison(&self.runs);
        runs.entry(run_id.to_string()).or_insert_with(|| RunHandle::new(run_id)).clone()
    }

    /// Forgets a finished run's handle (its terminal state lives in the Db).
    pub fn drop_handle(&self, run_id: &str) {
        lock_ignoring_poison(&self.runs).remove(run_id);
    }

    /// Forgets `handle` only if it is still the run's current handle, so a
    /// finishing old drive never removes the handle of a newer drive.
    fn drop_own_handle(&self, handle: &Arc<RunHandle>) {
        let mut runs = lock_ignoring_poison(&self.runs);
        if runs.get(&handle.run_id).is_some_and(|current| Arc::ptr_eq(current, handle)) {
            runs.remove(&handle.run_id);
        }
    }

    /// Whether a drive of this run is in flight in this process and has not
    /// been cancelled.
    pub fn is_driving(&self, run_id: &str) -> bool {
        self.handle(run_id).is_some_and(|handle| !handle.is_finished() && !handle.is_cancelled())
    }

    /// A fresh handle for the next drive of `run_id`. A previous handle that
    /// is still unwinding (cancelled) is waited for up to `grace`, then its
    /// task is aborted, so two drive loops never share a run (QA-022/023).
    /// Fails when a live, uncancelled drive owns the run.
    pub async fn prepare_drive(&self, run_id: &str, grace: Duration) -> Result<Arc<RunHandle>, String> {
        loop {
            let previous = {
                let mut runs = lock_ignoring_poison(&self.runs);
                match runs.get(run_id) {
                    Some(previous) => previous.clone(),
                    None => {
                        let fresh = RunHandle::new(run_id);
                        fresh.claimed.store(true, Ordering::SeqCst);
                        runs.insert(run_id.to_string(), fresh.clone());
                        return Ok(fresh);
                    }
                }
            };
            if !previous.is_finished() {
                if !previous.is_cancelled() {
                    return Err("Run is still generating; queue the message instead.".to_string());
                }
                self.stop_drive(&previous, grace).await;
            }
            // Swap in a fresh, claimed handle unless another caller already
            // replaced `previous` meanwhile (then re-check that one).
            let mut runs = lock_ignoring_poison(&self.runs);
            let unchanged = runs.get(run_id).is_none_or(|current| Arc::ptr_eq(current, &previous));
            if unchanged {
                let fresh = RunHandle::new(run_id);
                fresh.claimed.store(true, Ordering::SeqCst);
                runs.insert(run_id.to_string(), fresh.clone());
                return Ok(fresh);
            }
        }
    }

    /// Waits for a (cancelled) drive to finish, aborting it after `grace`.
    pub async fn stop_drive(&self, handle: &Arc<RunHandle>, grace: Duration) {
        if handle.wait_finished(grace).await {
            return;
        }
        tracing::warn!("Run {} did not unwind within {grace:?}; aborting its drive task", handle.run_id);
        handle.abort_drive();
        handle.wait_finished(Duration::from_secs(2)).await;
    }

    /// Emits a `status_changed` event on a live handle, if one exists.
    pub fn emit_status(&self, run_id: &str, status: &str) {
        if let Some(handle) = self.handle(run_id) {
            handle.emit(json!({ "type": "status_changed", "status": status }));
        }
    }

    fn track_exec_session(&self, run_id: &str, session_id: &str) {
        lock_ignoring_poison(&self.exec_sessions)
            .entry(run_id.to_string())
            .or_default()
            .insert(session_id.to_string());
    }

    fn untrack_exec_session(&self, run_id: &str, session_id: &str) {
        let mut sessions = lock_ignoring_poison(&self.exec_sessions);
        if let Some(ids) = sessions.get_mut(run_id) {
            ids.remove(session_id);
            if ids.is_empty() {
                sessions.remove(run_id);
            }
        }
    }

    /// Exec session ids started by a run that are still tracked.
    pub fn exec_sessions(&self, run_id: &str) -> Vec<String> {
        lock_ignoring_poison(&self.exec_sessions)
            .get(run_id)
            .map(|ids| ids.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Kills (process tree / Job Object) and forgets every exec session the
    /// run started. Used on cancel, failure, panic and run deletion.
    pub async fn kill_exec_sessions(&self, processes: &locagens_execpolicy::processes::ProcessManager, run_id: &str) {
        let ids = lock_ignoring_poison(&self.exec_sessions).remove(run_id).unwrap_or_default();
        for id in ids {
            processes.kill(&id).await;
            processes.drop_session(&id);
        }
    }

    /// Spawns the drive task for a run that already has its initial user
    /// message persisted. A supervisor awaits the task: a panic marks the
    /// run `failed` (QA-024), and every ending (normal, panic, abort) runs
    /// the same cleanup — sub-agents closed, exec sessions killed unless the
    /// turn completed, handle dropped.
    pub fn spawn(self: &Arc<Self>, state: Arc<AppState>, run_id: &str) {
        let handle = self.ensure_handle(run_id);
        let drive = drive_run(state.clone(), handle.clone());
        self.supervise(state, handle, drive);
    }

    /// Runs `drive` as the drive task of `handle` under the supervisor.
    fn supervise(
        self: &Arc<Self>,
        state: Arc<AppState>,
        handle: Arc<RunHandle>,
        drive: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        handle.claimed.store(true, Ordering::SeqCst);
        let drive = tokio::spawn(drive);
        *lock_ignoring_poison(&handle.drive_abort) = Some(drive.abort_handle());
        let orchestrator = self.clone();
        tokio::spawn(async move {
            if let Err(err) = drive.await {
                if err.is_panic() {
                    let reason = panic_message(err.into_panic());
                    tracing::error!("Run {} drive task panicked: {reason}", handle.run_id);
                    record_drive_panic(&state, &handle, &reason);
                }
            }
            orchestrator.finish_drive(&state, &handle).await;
        });
    }

    async fn finish_drive(&self, state: &AppState, handle: &Arc<RunHandle>) {
        handle.agents.shutdown();
        let completed = state.db.get_run_full(&handle.run_id).is_some_and(|run| run.status == "done");
        if !completed {
            self.kill_exec_sessions(&state.processes, &handle.run_id).await;
        }
        // Normally recorded by the drive; this covers panics and aborts.
        handle.record_turn_end().await;
        self.drop_own_handle(handle);
        handle.mark_finished();
    }
}

/// The text of a task panic payload.
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| text.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Lands `failed` for a run whose drive task panicked (unless it already
/// reached a terminal state).
fn record_drive_panic(state: &AppState, handle: &RunHandle, reason: &str) {
    let terminal = state
        .db
        .get_run_full(&handle.run_id)
        .is_some_and(|run| TERMINAL_STATUSES.contains(&run.status.as_str()));
    if terminal {
        return;
    }
    let error = format!("Internal error: the run's agent loop crashed ({reason}).");
    state.db.patch_run(&handle.run_id, |run| {
        run.status = "failed".into();
        run.error_message = Some(error.clone());
    });
    handle.emit(json!({ "type": "status_changed", "status": "failed" }));
    handle.emit(json!({ "type": "run_failed", "errorMessage": error }));
}

// ----------------------------------------------------------------------
// Errors
// ----------------------------------------------------------------------

#[derive(Debug)]
pub enum AgentError {
    Cancelled,
    Failed(String),
}

// ----------------------------------------------------------------------
// History projection (RunMessage → LlmMessage)
// ----------------------------------------------------------------------

/// Projects persisted messages into provider-neutral LLM messages. System and
/// sub-agent messages are skipped (sub-agent output was folded into the
/// architect's tool responses); tool responses pair with their tool call ids,
/// which are persisted in the tool message's `rawResponse`.
pub fn seed_history(messages: &[RunMessage]) -> Vec<llm::LlmMessage> {
    let mut history = Vec::new();
    for message in messages {
        if message.agent_role.is_some() {
            continue;
        }
        match message.role.as_str() {
            "system" => continue,
            "user" => history.push(llm::LlmMessage {
                role: "user".into(),
                content: message.content.clone(),
                ..Default::default()
            }),
            "assistant" => {
                let tool_calls = message
                    .raw_response
                    .as_deref()
                    .and_then(|raw| serde_json::from_str::<Vec<Value>>(raw).ok())
                    .filter(|calls| !calls.is_empty());
                history.push(llm::LlmMessage {
                    role: "assistant".into(),
                    content: message.content.clone(),
                    tool_calls,
                    ..Default::default()
                });
            }
            "tool" => history.push(llm::LlmMessage {
                role: "tool".into(),
                content: message.content.clone(),
                tool_call_id: message.raw_response.clone(),
                ..Default::default()
            }),
            _ => continue,
        }
    }
    history
}

// ----------------------------------------------------------------------
// Endpoint & cost resolution
// ----------------------------------------------------------------------

fn provider_display_name(providers: &[crate::pi_providers::ProviderMetadata], provider_id: &str) -> String {
    providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .map(|provider| provider.display_name.clone())
        .unwrap_or_else(|| provider_id.to_string())
}

/// Whether the model accepts an OpenAI `reasoning_effort` parameter.
fn reasoning_supported(model_id: &str, info: Option<&crate::pi_providers::ModelInfo>) -> bool {
    if let Some(true) = info.and_then(|info| info.reasoning) {
        return true;
    }
    let id = model_id.to_lowercase();
    ["o1", "o3", "o4", "gpt-5", "glm-5", "deepseek-r", "reasoner", "qwq", "qwen3", "thinking", "gemini-2.5", "gemini-3"]
        .iter()
        .any(|marker| id.starts_with(marker) || id.contains(marker))
}

/// Builds a resolved streaming endpoint from the `pi` provider engine.
async fn resolve_endpoint(
    pm: &PiProviderManager,
    provider_id: &str,
    model: &str,
    reasoning_effort: Option<&str>,
    session_id: &str,
) -> Result<llm::Endpoint, String> {
    let (mut base_url, mut api) = pm.resolve_endpoint(provider_id);
    if base_url.is_empty() {
        return Err(format!("No baseUrl configured for provider \"{provider_id}\"."));
    }
    let api_key = pm.resolve_api_key(provider_id, None).await.unwrap_or_default();
    if api_key.is_empty() && provider_id != "ollama" {
        return Err(format!(
            "No API key available for provider \"{provider_id}\". Add one in Settings → Providers."
        ));
    }
    // Per-model overrides (models.json / models-store.json) win over the
    // provider-level defaults — e.g. opencode-go routes each model family to
    // its own inference endpoint.
    let info = pm.model_info(provider_id, model);
    if let Some(model_base) = info.as_ref().and_then(|info| info.base_url.clone()).filter(|url| !url.is_empty()) {
        base_url = model_base;
    }
    if let Some(model_api) = info.as_ref().and_then(|info| info.api.clone()).filter(|api| !api.is_empty()) {
        api = model_api;
    }
    let quirk = llm::ProviderQuirk::detect(provider_id, &base_url, &api_key);
    if quirk == llm::ProviderQuirk::GithubCopilot {
        // The chat endpoint depends on the account: it is part of the token.
        base_url = pm.copilot_base_url(&api_key);
    }
    Ok(llm::Endpoint {
        base_url,
        api,
        api_key,
        session_id: session_id.to_string(),
        model: model.to_string(),
        reasoning_effort: reasoning_effort.filter(|e| !e.is_empty()).map(str::to_string),
        max_tokens: info.as_ref().and_then(|info| info.max_tokens),
        reasoning_supported: reasoning_supported(model, info.as_ref()),
        quirk,
    })
}

/// Computes the cost of one call: the provider's own reported cost wins,
/// then `pi`'s per-million model pricing.
fn cost_of(usage: &llm::Usage, info: Option<&crate::pi_providers::ModelInfo>) -> f64 {
    if let Some(provider_cost) = usage.provider_cost.filter(|cost| *cost > 0.0) {
        return provider_cost;
    }
    let Some(cost) = info.and_then(|info| info.cost.as_ref()) else {
        return 0.0;
    };
    let per_million = |keys: &[&str]| -> f64 {
        keys.iter()
            .find_map(|key| cost.get(*key).and_then(Value::as_f64))
            .unwrap_or(0.0)
    };
    let input = per_million(&["inputPerMillion", "input", "promptPerMillion"]);
    let output = per_million(&["outputPerMillion", "output", "completionPerMillion"]);
    let cache_read = per_million(&["cacheReadPerMillion", "cacheRead"]);
    let cache_write = per_million(&["cacheWritePerMillion", "cacheWrite"]);
    (input * usage.input_tokens as f64
        + output * usage.output_tokens as f64
        + cache_read * usage.cache_read_tokens as f64
        + cache_write * usage.cache_write_tokens as f64)
        / 1_000_000.0
}

// ----------------------------------------------------------------------
// System prompts
// ----------------------------------------------------------------------

fn memories_block(state: &AppState, project_path: &str) -> String {
    let all = state.db.list_memories();
    let memories: Vec<&crate::db::Memory> = all
        .iter()
        .filter(|memory| memory.scope == "global" || (memory.scope == "project" && memory.project_path == project_path))
        .collect();
    if memories.is_empty() {
        return String::new();
    }
    let mut block = String::from("\n\n## Long-term memory\n");
    for memory in memories {
        block.push_str(&format!("- [{}] {}\n", memory.category, memory.content));
    }
    block
}

fn mode_rules(mode: &str) -> String {
    match mode {
        "chat" => "## Mode: chat\nConversation only. Do not proactively touch the workspace, run commands, or start sub-agents; answer from context and ask before acting.".to_string(),
        "plan" => "## Mode: plan\nPlan only. Do NOT modify files or execute commands. Research (read/list/search) is allowed; communicate the plan as text and maintain it via update_plan.".to_string(),
        "ask_permissions" => "## Mode: ask permissions\nEvery tool call needs user approval unless a standing grant covers it; file mutations additionally show a diff preview. Make each tool call self-explanatory.".to_string(),
        "auto" => "## Mode: full access\nWork autonomously: apply edits and run commands without waiting for approval. Still avoid destructive actions unless explicitly requested.".to_string(),
        // accept_edits
        _ => "## Mode: build\nApply file edits directly (write_file/edit_file/move_file need no approval). Running commands and network fetches still pause for approval unless granted.".to_string(),
    }
}

fn system_prompt(state: &AppState, run: &crate::db::Run) -> String {
    // `--system-prompt` replaces the base prompt; `--append-system-prompt`
    // appends to whatever base ends up being used.
    let mut prompt = if run.system_prompt.as_deref().map(str::trim).map(str::is_empty) == Some(false)
        || run.append_system_prompt.is_some()
    {
        let mut prompt = match run.system_prompt.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(custom) => custom.to_string(),
            None => base_system_prompt(state, run),
        };
        if let Some(append) = run.append_system_prompt.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            prompt.push_str("\n\n");
            prompt.push_str(append);
        }
        prompt
    } else {
        base_system_prompt(state, run)
    };
    append_plugin_prompts(&mut prompt, &run.project_path);
    prompt
}

/// Appends the `systemPrompt` sections of every enabled plugin.
fn append_plugin_prompts(prompt: &mut String, project_path: &str) {
    let prompts = crate::services::plugins::enabled_system_prompts(Some(project_path));
    if prompts.is_empty() {
        return;
    }
    prompt.push_str("\n\n## Active plugins\n");
    for (name, text) in prompts {
        prompt.push_str(&format!("\n### {name}\n{text}\n"));
    }
}

fn base_system_prompt(state: &AppState, run: &crate::db::Run) -> String {
    let project = if run.project_path.is_empty() {
        "No project folder selected."
    } else {
        "Workspace root (all file tool paths are relative to it):"
    };
    format!(
        "You are the Locagens coding agent, working inside the user's project.\n\n\
{project} {path}\n{project_name}\n\n{mode}\n{agents_md}\n{memories}\n\n\
## Working style\n\
- Inspect before changing: read the relevant files first.\n\
- Prefer apply_patch/edit_file for small changes; write_file for new files.\n\
- Keep answers concise and concrete; summarize what you did.\n\
- Use update_plan for multi-step work so the user can follow progress.\n\
- Fan independent research or implementation sub-tasks out via spawn_agents / spawn_agent (explorer / worker).\n\
- Use ask_user_question when a decision genuinely needs the user.\n",
        project = project,
        path = run.project_path,
        project_name = {
            let name = &run.project_name;
            if name.is_empty() { String::new() } else { format!("(project: {name})") }
        },
        mode = mode_rules(&run.mode),
        agents_md = agents_md_block(&run.project_path),
        memories = memories_block(state, &run.project_path),
    )
}

/// Hierarchical AGENTS.md docs (Codex discovery: git root → cwd, 32 KiB
/// budget, AGENTS.override.md > AGENTS.md > CLAUDE.md > .agents.md).
fn agents_md_block(project_path: &str) -> String {
    if project_path.trim().is_empty() {
        return String::new();
    }
    let cwd = PathBuf::from(project_path);
    let (docs, sources) = agents_md::load_agents_md_with(&cwd, &cwd, &crate::config_runtime::doc_options());
    if docs.trim().is_empty() {
        return String::new();
    }
    let source_list = sources
        .iter()
        .map(|source| source.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "\n## Project instructions (AGENTS.md hierarchy from {source_list})\n\n{docs}\n"
    )
}

fn sub_agent_system_prompt(role: &str, title: &str, task: &str, project_path: &str, readonly: bool) -> String {
    let capability = if readonly {
        "You are read-only: you may read files, list directories and search, but never modify anything."
    } else {
        "You have full workspace tools: read, write, edit, move and search files; run commands and fetch URLs only when genuinely needed."
    };
    format!(
        "You are a {role} sub-agent (\"{title}\") spawned inside the Locagens orchestrator.\n\
Workspace root: {project_path}\n\n{capability}\n\n\
Your assigned task:\n{task}\n\n\
Work independently and completely; do not ask the user questions. When done, reply with a concise final summary of what you did and any important outcomes — it is returned to the coordinating agent as your task result. You may later receive follow-up tasks or messages; treat them as continuing this same job.\n"
    )
}

// ----------------------------------------------------------------------
// Agent loop
// ----------------------------------------------------------------------

/// Drives model ↔ tool rounds for one agent (main or sub-agent).
struct AgentLoop {
    state: Arc<AppState>,
    handle: Arc<RunHandle>,
    run_id: String,
    project_path: PathBuf,
    mode: String,
    provider_id: String,
    provider_display_name: String,
    model: String,
    reasoning_effort: Option<String>,
    agent_role: Option<String>,
    agent_name: Option<String>,
    history: Vec<llm::LlmMessage>,
    tools: Vec<Value>,
    system: String,
    /// Remaining delegation depth (1 = may spawn sub-agents).
    depth_left: u32,
    /// Aggregate workspace diff across this turn.
    turn_diff: Arc<TurnDiffTracker>,
    /// Inbox of a persistent sub-agent (`send_message` / `followup_task`).
    agent_inbox: Option<mpsc::UnboundedReceiver<agents::AgentCommand>>,
    extras: LoopExtras,
}

/// Optional per-loop state kept out of the main constructor noise.
#[derive(Default)]
struct LoopExtras {
    /// MCP tool specs held back behind `tool_search` (see `tool_search.rs`).
    deferred_tools: Vec<Value>,
    /// Raised by `interrupt_agent` to end this agent's current turn only.
    interrupt: Option<Arc<AtomicBool>>,
    /// Read-only sub-agent (explorer roles): never mutates or executes,
    /// whatever the run mode.
    readonly: bool,
}

impl AgentLoop {
    fn interrupted(&self) -> bool {
        self.extras.interrupt.as_ref().is_some_and(|flag| flag.load(Ordering::SeqCst))
    }

    /// True when the whole run was cancelled or this agent was interrupted.
    fn stop_requested(&self) -> bool {
        self.handle.is_cancelled() || self.interrupted()
    }

    /// Resolves once the run is cancelled or this agent is interrupted (for
    /// `tokio::select!` around long awaits).
    async fn stop_signal(&self) {
        match self.extras.interrupt.as_ref() {
            Some(flag) => tokio::select! {
                _ = self.handle.cancelled() => {}
                _ = agents::interrupt_raised(flag) => {}
            },
            None => self.handle.cancelled().await,
        }
    }

    fn check_cancel(&self) -> Result<(), AgentError> {
        if self.stop_requested() {
            Err(AgentError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// `get_context_remaining`: window from the provider catalog, usage from
    /// the auto-compactor's own estimate.
    fn context_remaining(&self) -> String {
        let window = self.state.pi.model_info(&self.provider_id, &self.model).and_then(|info| info.context_window);
        let budget = context_remaining::ContextBudget::measure(&self.history, window);
        serde_json::to_string_pretty(&budget.to_json()).unwrap_or_default()
    }

    fn new_message(&self, role: &str, content: &str) -> RunMessage {
        RunMessage {
            id: format!("msg-{}", uuid::Uuid::new_v4()),
            run_id: self.run_id.clone(),
            role: role.to_string(),
            agent_role: self.agent_role.clone(),
            agent_name: self.agent_name.clone(),
            provider_id: if role == "assistant" { Some(self.provider_id.clone()) } else { None },
            provider_display_name: if role == "assistant" { Some(self.provider_display_name.clone()) } else { None },
            model: if role == "assistant" { Some(self.model.clone()) } else { None },
            content: content.to_string(),
            created_at: crate::util::now_iso(),
            ..Default::default()
        }
    }

    fn persist_and_emit(&self, message: &RunMessage) {
        self.state.db.append_message(message.clone());
        self.handle.emit(json!({
            "type": "message_created",
            "message": serde_json::to_value(message).unwrap_or(Value::Null),
        }));
    }

    /// Runs the loop until the model produces a final answer.
    async fn run(&mut self, max_rounds: usize) -> Result<String, AgentError> {
        let mut last_assistant_content = String::new();
        let mut stop_attempts = 0u32;
        for round in 1..=max_rounds {
            self.check_cancel()?;
            // Codex-style auto compaction: when the estimated input tokens
            // exceed the threshold, summarize older history before the call.
            if self.agent_role.is_none() {
                self.maybe_auto_compact().await?;
            }
            if self.agent_role.is_none() {
                self.handle.emit(json!({ "type": "round_started", "round": round }));
            }

            let endpoint = resolve_endpoint(
                &self.state.pi,
                &self.provider_id,
                &self.model,
                self.reasoning_effort.as_deref(),
                &self.run_id,
            )
            .await
            .map_err(AgentError::Failed)?;

            // Assistant placeholder, streamed live.
            let mut message = self.new_message("assistant", "");
            self.persist_and_emit(&message);
            let message_id = message.id.clone();

            let mut content = String::new();
            let mut reasoning = String::new();
            let mut last_emit = Instant::now() - Duration::from_secs(1);
            let mut last_persist = Instant::now() - Duration::from_secs(1);

            // Live `apply_patch` progress while the call's arguments stream.
            let mut patch_streams: HashMap<usize, patch_progress::PatchArgsStream> = HashMap::new();
            let mut last_patch_emit = Instant::now() - Duration::from_secs(1);

            let http = endpoint_http(&self.state);
            let stream = {
                let mut on_delta = |content_delta: &str, reasoning_delta: &str| {
                    content.push_str(content_delta);
                    reasoning.push_str(reasoning_delta);
                    message.content = content.clone();
                    if !reasoning.is_empty() {
                        message.reasoning_content = Some(reasoning.clone());
                    }
                    if last_emit.elapsed() >= STREAM_EMIT_INTERVAL {
                        self.handle.emit(json!({
                            "type": "message_updated",
                            "message": serde_json::to_value(&message).unwrap_or(Value::Null),
                        }));
                        last_emit = Instant::now();
                    }
                    if last_persist.elapsed() >= STREAM_PERSIST_INTERVAL {
                        self.state
                            .db
                            .stream_message(&self.run_id, &message_id, &content, Some(&reasoning));
                        last_persist = Instant::now();
                    }
                };
                let mut on_tool_args = |progress: &llm::ToolArgsProgress| {
                    let patch = patch_streams.entry(progress.index).or_default();
                    let changed = patch.update(&progress.arguments);
                    if changed && (patch.ended() || last_patch_emit.elapsed() >= STREAM_EMIT_INTERVAL) {
                        last_patch_emit = Instant::now();
                        self.handle.emit(patch.event(
                            &self.run_id,
                            &message_id,
                            &progress.call_id,
                            self.agent_name.as_deref(),
                        ));
                    }
                };
                let should_stop = || self.stop_requested();
                let call = llm::stream_chat_observed(
                    &http,
                    &endpoint,
                    &self.system,
                    &self.history,
                    &self.tools,
                    &mut on_delta,
                    &mut on_tool_args,
                    &should_stop,
                );
                // Race the whole call (request, retries, stream) against
                // cancellation so a stalled provider cannot pin a cancelled loop.
                tokio::select! {
                    stream = call => stream,
                    _ = self.stop_signal() => return Err(AgentError::Cancelled),
                }
            };

            let outcome = match stream {
                Ok(outcome) => outcome,
                Err(err) if err.to_string() == "cancelled" || self.stop_requested() => {
                    return Err(AgentError::Cancelled)
                }
                Err(err) => return Err(AgentError::Failed(err.to_string())),
            };
            self.check_cancel()?;

            let tool_calls = outcome.tool_calls;
            message.content = outcome.content.clone();
            if !outcome.reasoning.is_empty() {
                message.reasoning_content = Some(outcome.reasoning.clone());
            }
            if !tool_calls.is_empty() {
                message.raw_response = Some(serde_json::to_string(&tool_calls).unwrap_or_default());
            }
            self.state.db.update_message_full(
                &self.run_id,
                &message_id,
                &message.content,
                message.reasoning_content.as_deref(),
                message.raw_response.as_deref(),
            );
            self.handle.emit(json!({
                "type": "message_updated",
                "message": serde_json::to_value(&message).unwrap_or(Value::Null),
            }));

            // Usage accounting (main and sub-agent roles are both logged).
            if let Some(usage) = outcome.usage {
                let info = self.state.pi.model_info(&self.provider_id, &self.model);
                self.state.db.append_usage(UsageLogEntry {
                    id: 0,
                    run_id: self.run_id.clone(),
                    agent_role: Some(self.agent_role.as_deref().unwrap_or("planner").to_string()),
                    provider_id: self.provider_id.clone(),
                    model: self.model.clone(),
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cache_read_tokens: usage.cache_read_tokens,
                    cache_write_tokens: usage.cache_write_tokens,
                    cache_hit_rate: if usage.input_tokens > 0 {
                        usage.cache_read_tokens as f64 * 100.0 / usage.input_tokens as f64
                    } else {
                        0.0
                    },
                    cost: cost_of(&usage, info.as_ref()),
                    created_at: crate::util::now_iso(),
                    duration_ms: None,
                });
            }

            if tool_calls.is_empty() {
                // Final answer. The main agent's Stop hook may ask it to continue.
                if self.agent_role.is_none() && self.stop_hook_continues(&message.content, &mut stop_attempts).await {
                    continue;
                }
                return Ok(message.content.clone());
            }
            last_assistant_content = message.content.clone();

            self.history.push(llm::LlmMessage {
                role: "assistant".into(),
                content: message.content.clone(),
                tool_calls: Some(tool_calls.clone()),
                ..Default::default()
            });

            for call in &tool_calls {
                self.check_cancel()?;
                let call_id = call.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                let name = call.pointer("/function/name").and_then(Value::as_str).unwrap_or_default().to_string();
                let arguments = call.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}");
                let args: Value = serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));

                // Lifecycle hooks: PreToolUse may block the call or rewrite
                // its input before anything executes.
                let pre = self.run_pre_tool_hook(&call_id, &name, &args).await;
                let result = if pre.blocked {
                    json!({
                        "ok": false,
                        "error": format!(
                            "Blocked by PreToolUse hook: {}",
                            pre.reason.unwrap_or_else(|| "blocked".to_string())
                        ),
                    })
                    .to_string()
                } else {
                    let hook_args = pre.updated_input.clone().unwrap_or_else(|| args.clone());
                    match self.dispatch_tool(&call_id, &name, &hook_args).await {
                        Ok(result) => result,
                        Err(AgentError::Cancelled) => return Err(AgentError::Cancelled),
                        Err(AgentError::Failed(err)) => format!("Tool error: {err}"),
                    }
                };

                // PostToolUse may append context to the tool response.
                let result = self.run_post_tool_hook(&call_id, &name, &args, result).await;

                let mut tool_message = self.new_message("tool", &result);
                tool_message.raw_response = Some(call_id.clone());
                self.persist_and_emit(&tool_message);
                self.history.push(llm::LlmMessage {
                    role: "tool".into(),
                    content: result,
                    tool_call_id: Some(call_id),
                    ..Default::default()
                });
            }

            // Fold queued mid-run user messages into the conversation.
            let mut inbox = self.handle.inbox.lock().await;
            let queued: Vec<String> = inbox.drain(..).collect();
            drop(inbox);
            // Interactive sub-agents also fold their own `send_input` queue.
            let mut agent_queued: Vec<String> = Vec::new();
            if let Some(agent_inbox) = self.agent_inbox.as_mut() {
                while let Ok(command) = agent_inbox.try_recv() {
                    agent_queued.push(command.into_text());
                }
            }
            for queued_content in queued.into_iter().chain(agent_queued) {
                let user_message = self.new_message("user", &queued_content);
                self.persist_and_emit(&user_message);
                self.history.push(llm::LlmMessage {
                    role: "user".into(),
                    content: queued_content,
                    ..Default::default()
                });
            }
        }
        // Round budget exhausted: return the last assistant text so the run
        // still completes gracefully.
        Ok(last_assistant_content)
    }

    // ------------------------------------------------------------------
    // Lifecycle hooks (PreToolUse / PostToolUse / Stop)
    // ------------------------------------------------------------------

    fn hook_payload(&self, event: &str, extra: Value) -> Value {
        let mut payload = json!({
            "session_id": self.run_id,
            "cwd": self.project_path.display().to_string(),
            "hook_event_name": event,
        });
        if let (Some(obj), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                obj.insert(key.clone(), value.clone());
            }
        }
        payload
    }

    /// Runs `PreToolUse` hooks; a blocked outcome suppresses the tool call.
    async fn run_pre_tool_hook(&self, call_id: &str, name: &str, args: &Value) -> crate::services::hooks::HookOutcome {
        let payload = self.hook_payload(
            "PreToolUse",
            json!({
                "tool_name": name,
                "tool_input": args,
                "tool_use_id": call_id,
            }),
        );
        crate::services::hooks::run_hook_event("PreToolUse", &self.project_path, &payload).await
    }

    /// Runs `PostToolUse` hooks and appends any additional context to the
    /// tool response.
    async fn run_post_tool_hook(&self, call_id: &str, name: &str, args: &Value, mut result: String) -> String {
        let payload = self.hook_payload(
            "PostToolUse",
            json!({
                "tool_name": name,
                "tool_input": args,
                "tool_response": result,
                "tool_use_id": call_id,
            }),
        );
        let outcome = crate::services::hooks::run_hook_event("PostToolUse", &self.project_path, &payload).await;
        if let Some(context) = outcome.context_joined() {
            result.push_str(&format!("\n[hook context]: {context}"));
        }
        if outcome.blocked {
            result.push_str(&format!("\n[hook feedback]: {}", outcome.reason.unwrap_or_else(|| "blocked".into())));
        }
        result
    }

    /// Whether `name` was offered to this agent: an advertised tool spec, a
    /// deferred MCP spec, or `tool_search` while specs are deferred.
    fn tool_is_offered(&self, name: &str) -> bool {
        let named = |tool: &Value| tool.pointer("/function/name").and_then(Value::as_str) == Some(name);
        self.tools.iter().any(named)
            || self.extras.deferred_tools.iter().any(named)
            || (name == tool_search::TOOL_SEARCH_NAME && !self.extras.deferred_tools.is_empty())
    }

    /// Executes one tool call, routing interactive/delegation tools to the
    /// orchestrator and workspace tools through permission gating.
    async fn dispatch_tool(&mut self, call_id: &str, name: &str, args: &Value) -> Result<String, AgentError> {
        // Models do call tools they were never offered: only the advertised
        // set (plus deferred MCP specs behind `tool_search`) may run.
        if !self.tool_is_offered(name) {
            return Err(AgentError::Failed(format!(
                "Unknown tool \"{name}\": it is not available to this agent."
            )));
        }
        let readonly_mode = if self.extras.readonly { "plan" } else { self.mode.as_str() };
        if tools::is_forbidden_in_readonly_mode(readonly_mode, name) {
            let error = if self.extras.readonly {
                format!("Tool '{name}' is not allowed for a read-only agent.")
            } else {
                format!("Tool '{name}' is not allowed in '{}' mode.", self.mode)
            };
            return Ok(serde_json::json!({ "ok": false, "error": error }).to_string());
        }
        match name {
            "set_chat_title" => {
                let title = args.get("title").and_then(Value::as_str).unwrap_or_default().trim().to_string();
                if title.is_empty() {
                    return Err(AgentError::Failed("set_chat_title: title is empty.".into()));
                }
                self.state.db.patch_run(&self.run_id, |run| run.title = title.clone());
                self.handle.emit(json!({ "type": "run_title_changed", "runId": self.run_id, "title": title }));
                Ok("Chat title updated.".to_string())
            }
            "update_plan" => self.update_plan(args).await,
            "remember" => {
                let scope = args.get("scope").and_then(Value::as_str).unwrap_or("project");
                let category = args.get("category").and_then(Value::as_str).unwrap_or("project");
                let content = args.get("content").and_then(Value::as_str).unwrap_or_default().trim().to_string();
                if content.is_empty() {
                    return Err(AgentError::Failed("remember: content is empty.".into()));
                }
                self.state
                    .db
                    .create_memory(scope, &self.project_path.display().to_string(), category, &content)
                    .map_err(|err| AgentError::Failed(err.to_string()))?;
                Ok(format!("Remembered ({scope}/{category})."))
            }
            "load_skill" => {
                let name = args.get("name").and_then(Value::as_str).unwrap_or_default();
                let skill = crate::services::skills::list(Some(&self.project_path.display().to_string()))
                    .into_iter()
                    .find(|skill| skill.name == name);
                match skill {
                    Some(skill) => Ok(skill.body.unwrap_or_default()),
                    None => Err(AgentError::Failed(format!("Skill \"{name}\" not found."))),
                }
            }
            "ask_user_question" => self.ask_user_question(args).await,
            "request_user_input" => {
                // Codex schema normalization: every question gets a header and
                // default options so the existing QuestionCard flow can render it.
                let normalized = normalize_request_user_input(args);
                self.ask_user_question(&normalized).await
            }
            "compact_context" => self.compact_context_tool().await,
            "spawn_agents" if self.depth_left > 0 => self.spawn_agents_tool(args).await,
            name if agents::is_agent_tool(name) && self.depth_left > 0 => self.dispatch_agent_tool(name, args).await,
            tool_search::TOOL_SEARCH_NAME => {
                tool_search::apply_search(&mut self.tools, &self.extras.deferred_tools, args).map_err(AgentError::Failed)
            }
            "get_context_remaining" => Ok(self.context_remaining()),
            "host_executable" => host_executable::execute(args).map_err(AgentError::Failed),
            "view_image" => tools::execute(&self.project_path, "view_image", args)
                .await
                .map_err(AgentError::Failed),
            "list_mcp_resources" => {
                let server = str_arg(args, "server").or_else(|| str_arg(args, "server_filter"));
                let cursor = str_arg(args, "cursor");
                crate::services::mcp::list_resources(
                    Some(&self.project_path.display().to_string()),
                    server,
                    cursor,
                )
                .await
                .map_err(AgentError::Failed)
            }
            "read_mcp_resource" => {
                let server = str_arg(args, "server").unwrap_or_default();
                let uri = str_arg(args, "uri").unwrap_or_default();
                if server.trim().is_empty() || uri.trim().is_empty() {
                    return Err(AgentError::Failed("read_mcp_resource: server and uri are required.".into()));
                }
                crate::services::mcp::read_resource(
                    Some(&self.project_path.display().to_string()),
                    server,
                    uri,
                )
                .await
                .map_err(AgentError::Failed)
            }
            name if name.starts_with("mcp__") => {
                // Qualified MCP tools are blocked in read-only modes; they
                // route through the gated executor elsewhere (permission is
                // required in ask_permissions mode via needs_permission).
                if self.mode == "chat" || self.mode == "plan" {
                    Ok(json!({
                        "ok": false,
                        "error": format!("MCP tools are not available in '{}' mode.", self.mode),
                    })
                    .to_string())
                } else {
                    self.gated_execute(call_id, name, args).await
                }
            }
            "exec_command" | "apply_patch" => self.gated_execute(call_id, name, args).await,
            "write_stdin" => self.write_stdin_tool(args).await,
            name if WORKSPACE_TOOLS.contains(&name) => self.gated_execute(call_id, name, args).await,
            other => Err(AgentError::Failed(format!("Unknown tool \"{other}\"."))),
        }
    }

    /// Loads the merged Starlark `.rules` exec policy for this run: every
    /// agent-directory `rules` folder plus, for a trusted project folder
    /// only, `<project>/.locagens/rules` (an untrusted repo could otherwise
    /// ship `allow` rules for any command).
    fn load_exec_policy(&self) -> locagens_execpolicy::rules::Policy {
        let mut dirs: Vec<PathBuf> = locagens_config::paths::agent_dir_chain()
            .into_iter()
            .map(|dir| dir.join("rules"))
            .collect();
        if crate::services::project_trust::is_trusted(&self.project_path) {
            dirs.push(self.project_path.join(".locagens").join("rules"));
        }
        locagens_execpolicy::rules::Policy::load_from_dirs(&dirs)
    }

    /// Executes one gated tool after permission decisions have been made.
    async fn execute_tool(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "exec_command" => self.exec_session(args).await,
            "run_command" => {
                let (network, _approval_window) = self.command_network_policy(args);
                tools::run_command_with_network(&self.project_path, &self.mode, args, Some(network)).await
            }
            "apply_patch" => self.apply_patch_tool(args).await,
            name if name.starts_with("mcp__") => {
                let (server_key, tool_name) = crate::services::mcp::parse_qualified_mcp_tool(name)
                    .ok_or_else(|| format!("Invalid qualified MCP tool name: {name}"))?;
                crate::services::mcp::call_mcp_tool(
                    Some(&self.project_path.display().to_string()),
                    &server_key,
                    &tool_name,
                    args,
                )
                .await
            }
            _ => tools::execute_with_mode(&self.project_path, &self.mode, name, args).await,
        }
    }

    /// `apply_patch` tool: snapshots turn-diff baselines, applies the patch
    /// and returns Codex's summary plus the unified diff.
    async fn apply_patch_tool(&self, args: &Value) -> Result<String, String> {
        let patch_text = args
            .get("input")
            .and_then(Value::as_str)
            .or_else(|| args.get("patch").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        if patch_text.trim().is_empty() {
            return Err("apply_patch: patch text is empty.".to_string());
        }
        let parsed = apply_patch::parse_patch(&patch_text).map_err(|err| err.to_string())?;
        let affected = apply_patch::affected_paths(&self.project_path, &parsed)?;
        // Snapshot baselines before anything mutates the disk.
        for relative in affected.all() {
            if let Ok(path) = tools::safe_path(&self.project_path, &relative) {
                self.turn_diff.snapshot(&path);
            }
        }
        let (summary, diff, _affected) = apply_patch::apply_parsed(&self.project_path, &parsed)?;
        self.emit_turn_diff();
        let mut response = summary;
        if !diff.is_empty() {
            response.push_str("\n\nUnified diff preview:\n");
            response.push_str(&diff.chars().take(12_000).collect::<String>());
        }
        Ok(response)
    }

    /// Emits `turn_diff_updated` with the aggregate diff for this turn.
    fn emit_turn_diff(&self) {
        let diff = self.turn_diff.unified_diff(&self.project_path);
        if diff.is_empty() {
            return;
        }
        self.handle.emit(json!({
            "type": "turn_diff_updated",
            "diff": diff,
            "filesChanged": self.turn_diff.changed_files(&self.project_path),
        }));
    }

    /// Snapshots turn-diff baselines for a mutating tool call.
    fn snapshot_turn_diff(&self, name: &str, args: &Value) {
        if !tools::MUTATION_TOOLS.contains(&name) || name == "apply_patch" {
            return;
        }
        let mut paths: Vec<String> = Vec::new();
        if let Some(path) = args.get("path").and_then(Value::as_str) {
            paths.push(path.to_string());
        }
        if let Some(from) = args.get("from").and_then(Value::as_str) {
            paths.push(from.to_string());
        }
        if let Some(to) = args.get("to").and_then(Value::as_str) {
            paths.push(to.to_string());
        }
        for relative in paths {
            if let Ok(path) = tools::safe_path(&self.project_path, &relative) {
                self.turn_diff.snapshot(&path);
            }
        }
    }

    /// Runs one exec session (`exec_command`): spawns via the ProcessManager,
    /// waits up to `yield_time_ms` for completion and either returns the
    /// result or hands back an interactive session id.
    async fn exec_session(&self, args: &Value) -> Result<String, String> {
        let command = args
            .get("cmd")
            .or_else(|| args.get("command"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if command.is_empty() {
            return Err("exec_command: cmd is empty.".to_string());
        }
        let workdir = match str_arg(args, "workdir") {
            Some(workdir) if !workdir.trim().is_empty() => {
                let path = tools::safe_path(&self.project_path, workdir)?;
                if !path.is_dir() {
                    return Err(format!("exec_command: workdir {workdir:?} is not a directory."));
                }
                path
            }
            _ => self.project_path.clone(),
        };
        let yield_ms = args
            .get("yield_time_ms")
            .and_then(Value::as_u64)
            .unwrap_or(10_000)
            .clamp(500, 60_000);
        let max_chars = args
            .get("max_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(20_000)
            .clamp(1_000, 200_000) as usize * 4;

        let tty = args.get("tty").and_then(Value::as_bool).unwrap_or(false);
        let shell = str_arg(args, "shell")
            .map(str::trim)
            .filter(|shell| !shell.is_empty())
            .map(str::to_string);
        let login = args.get("login").and_then(Value::as_bool).unwrap_or(false);
        // Unknown hosts may prompt only while this call waits (see network_access).
        let (network, _approval_window) = self.command_network_policy(args);
        let options = locagens_execpolicy::processes::SpawnOptions {
            stdin_pipe: true,
            tty,
            env_clean: true,
            shell,
            login,
            env: Some(tools::exec_session_env()),
            sandbox: Some(tools::command_sandbox_spec(
                &self.mode,
                &self.project_path,
                network,
                &network_access::requested_domains(args),
            )),
        };
        let info = self
            .state
            .processes
            .spawn_with_options(&command, workdir, options);
        // Tracked per run so cancel / failure kills it (QA-027).
        self.state.orchestrator.track_exec_session(&self.run_id, &info.id);
        let started = std::time::Instant::now();
        self.wait_exec_session(&info.id, yield_ms).await.map_err(|_| "cancelled".to_string())?;
        let poll = self.state.processes.poll_output(&info.id, max_chars).unwrap_or(
            locagens_execpolicy::processes::ExecPoll { output: String::new(), status: "exited".into(), exit_code: None },
        );
        if poll.status == "exited" {
            self.forget_exec_session(&info.id);
            return Ok(serde_json::json!({
                "exit_code": poll.exit_code.unwrap_or(-1),
                "output": poll.output,
                "wall_time_ms": started.elapsed().as_millis() as u64,
            })
            .to_string());
        }
        Ok(serde_json::json!({
            "session_id": info.id,
            "status": "running",
            "output": poll.output,
        })
        .to_string())
    }

    /// Waits until the session's process exits or `yield_ms` passes, without
    /// consuming output (it is read once afterwards, so nothing is lost).
    /// Checks for cancellation / interrupt every 100 ms (QA-023).
    async fn wait_exec_session(&self, session_id: &str, yield_ms: u64) -> Result<(), AgentError> {
        let started = std::time::Instant::now();
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            self.check_cancel()?;
            let running = self.state.processes.is_running(session_id).unwrap_or(false);
            if !running {
                // Let the pipe readers flush the last chunks after the exit.
                tokio::time::sleep(Duration::from_millis(50)).await;
                return Ok(());
            }
            if started.elapsed().as_millis() as u64 >= yield_ms {
                return Ok(());
            }
        }
    }

    /// Drops a finished exec session and stops tracking it for the run.
    fn forget_exec_session(&self, session_id: &str) {
        self.state.processes.drop_session(session_id);
        self.state.orchestrator.untrack_exec_session(&self.run_id, session_id);
    }

    /// `write_stdin`: feeds characters to a running exec session and polls
    /// its latest output. Only sessions this run started are reachable;
    /// approving an interactive `exec_command` (a shell, a REPL) approves
    /// what is later typed into it.
    async fn write_stdin_tool(&self, args: &Value) -> Result<String, AgentError> {
        let session_id = str_arg(args, "session_id").unwrap_or_default().to_string();
        if session_id.is_empty() {
            return Err(AgentError::Failed("write_stdin: session_id is empty.".into()));
        }
        if !self.state.orchestrator.exec_sessions(&self.run_id).contains(&session_id) {
            return Err(AgentError::Failed(format!("write_stdin: unknown session {session_id}.")));
        }
        if let Some(chars) = str_arg(args, "chars") {
            let sent = self.state.processes.write_stdin(&session_id, chars).await;
            if !sent {
                return Err(AgentError::Failed(format!(
                    "write_stdin: session {session_id} has no writable stdin (unknown or already exited)."
                )));
            }
        }
        let yield_ms = args
            .get("yield_time_ms")
            .and_then(Value::as_u64)
            .unwrap_or(2_000)
            .clamp(100, 60_000);
        let max_chars = args
            .get("max_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(20_000)
            .clamp(1_000, 200_000) as usize * 4;
        if self.state.processes.is_running(&session_id).is_none() {
            return Err(AgentError::Failed(format!("write_stdin: unknown session {session_id}.")));
        }
        self.wait_exec_session(&session_id, yield_ms).await?;
        let Some(poll) = self.state.processes.poll_output(&session_id, max_chars) else {
            return Err(AgentError::Failed(format!("write_stdin: unknown session {session_id}.")));
        };
        if poll.status == "exited" {
            self.forget_exec_session(&session_id);
            return Ok(serde_json::json!({
                "session_id": session_id,
                "status": "exited",
                "exit_code": poll.exit_code.unwrap_or(-1),
                "output": poll.output,
            })
            .to_string());
        }
        Ok(serde_json::json!({
            "session_id": session_id,
            "status": "running",
            "output": poll.output,
        })
        .to_string())
    }

    /// Manual context compaction (`compact_context` tool).
    async fn compact_context_tool(&mut self) -> Result<String, AgentError> {
        let before = self.history.len();
        self.run_compaction("manual").await?;
        Ok(format!(
            "Context compacted: {before} messages → {} messages. Older conversation was replaced by a summary block.",
            self.history.len()
        ))
    }

    /// Codex-style compaction: summarize older history with the
    /// SUMMARIZATION_PROMPT and replace it with a summary block.
    async fn maybe_auto_compact(&mut self) -> Result<(), AgentError> {
        if !compact::should_auto_compact(&self.history) {
            return Ok(());
        }
        self.run_compaction("auto").await
    }

    async fn run_compaction(&mut self, trigger: &str) -> Result<(), AgentError> {
        if self.pre_compact_blocks(trigger).await {
            return Ok(());
        }
        let Some((system, transcript)) = compact::summarization_request(&self.history, compact::DEFAULT_KEEP_LAST_MESSAGES)
        else {
            return Ok(());
        };
        let endpoint = resolve_endpoint(
            &self.state.pi,
            &self.provider_id,
            &self.model,
            self.reasoning_effort.as_deref(),
            &self.run_id,
        )
        .await
        .map_err(AgentError::Failed)?;
        let messages = vec![llm::LlmMessage {
            role: "user".into(),
            content: transcript,
            ..Default::default()
        }];
        let http = endpoint_http(&self.state);
        let mut ignore_deltas = |_content_delta: &str, _reasoning_delta: &str| {};
        let should_stop = || self.handle.is_cancelled();
        let summary = llm::stream_chat(&http, &endpoint, &system, &messages, &[], &mut ignore_deltas, &should_stop);
        let summary = tokio::select! {
            summary = summary => summary,
            _ = self.handle.cancelled() => return Err(AgentError::Cancelled),
        };
        let outcome = summary
        .map_err(|err| {
            if err.to_string() == "cancelled" || self.handle.is_cancelled() {
                AgentError::Cancelled
            } else {
                AgentError::Failed(format!("compact_context: {err}"))
            }
        })?;
        let estimated_tokens = compact::estimate_history_tokens(&self.history);
        let messages_before = self.history.len();
        self.history = compact::apply_compaction(&self.history, &outcome.content, compact::DEFAULT_KEEP_LAST_MESSAGES);
        self.handle.emit(json!({
            "type": "context_compacted",
            "estimatedTokensBefore": estimated_tokens,
            "messagesBefore": messages_before,
            "messagesAfter": self.history.len(),
            "summary": outcome.content,
        }));
        self.post_compact_hook(trigger).await;
        Ok(())
    }

    async fn update_plan(&self, args: &Value) -> Result<String, AgentError> {
        // Accepts BOTH schemas:
        // - Codex:   { explanation?, plan: [{ step, status }] }
        // - Locagens: { title, body?, tasks: [{ text, status }] }
        let (title, body, tasks) = normalize_plan_args(args);
        if title.is_empty() {
            return Err(AgentError::Failed("update_plan: title is empty.".into()));
        }

        let existing = self.state.db.get_plan(&self.run_id);
        let now = crate::util::now_iso();
        let plan = match existing {
            Some(mut plan) => {
                plan.title = title;
                plan.body = body;
                plan.tasks = tasks;
                plan.version += 1;
                plan.updated_at = now;
                plan
            }
            None => Plan {
                id: format!("plan-{}", uuid::Uuid::new_v4()),
                run_id: self.run_id.clone(),
                title,
                body,
                tasks,
                status: "active".to_string(),
                version: 1,
                created_at: now.clone(),
                updated_at: now,
            },
        };
        self.state.db.set_plan(plan.clone());
        self.handle.emit(json!({
            "type": "plan_updated",
            "plan": serde_json::to_value(&plan).unwrap_or(Value::Null),
        }));
        Ok("Plan updated.".to_string())
    }

    async fn ask_user_question(&self, args: &Value) -> Result<String, AgentError> {
        let questions = args.get("questions").cloned().unwrap_or(Value::Null);
        let valid = questions
            .as_array()
            .map(|items| !items.is_empty() && items.iter().all(|q| q.get("question").is_some() && q.get("options").map(Value::is_array).unwrap_or(false)))
            .unwrap_or(false);
        if !valid {
            return Err(AgentError::Failed(
                "ask_user_question: questions must be a non-empty array of {question, header, options}."
                    .to_string(),
            ));
        }

        // Pause the run until the user answers via POST /api/runs/:id/answer.
        self.notification_hook("elicitation_dialog", "Locagens is waiting for your answer").await;
        self.state.db.patch_run(&self.run_id, |run| run.status = "awaiting_input".into());
        self.handle.emit(json!({ "type": "status_changed", "status": "awaiting_input" }));
        self.handle.emit(json!({
            "type": "question_requested",
            "runId": self.run_id,
            "questions": questions,
        }));

        let (sender, receiver) = oneshot::channel::<Value>();
        {
            let mut slot = self.handle.question.lock().await;
            slot.pending = Some(questions.clone());
            slot.sender = Some(sender);
        }
        let answer = tokio::select! {
            answer = receiver => answer.ok(),
            _ = self.handle.cancelled() => None,
        };
        {
            let mut slot = self.handle.question.lock().await;
            slot.pending = None;
            slot.sender = None;
        }
        if answer.is_none() {
            return Err(AgentError::Cancelled);
        }

        self.state.db.patch_run(&self.run_id, |run| run.status = "generating".into());
        self.handle.emit(json!({ "type": "status_changed", "status": "generating" }));

        Ok(format!(
            "The user answered your questions:\n{}",
            serde_json::to_string_pretty(&answer.unwrap()).unwrap_or_else(|_| "{}".into())
        ))
    }

    // -- Permission gating -------------------------------------------------

    /// Network allowlist + live domain approval for a sandboxed command.
    fn command_network_policy(
        &self,
        args: &Value,
    ) -> (locagens_execpolicy::network_proxy::NetworkPolicy, network_access::ApprovalWindow) {
        network_access::command_network_policy(&self.state, &self.handle, &self.project_path, args)
    }

    fn standing_grant(&self, name: &str, args: &Value) -> bool {
        let project_path = self.project_path.display().to_string();
        let command = extract_command_str(args);
        let domains = sorted_domains(args);
        self.state
            .db
            .list_permissions()
            .iter()
            .any(|rule| standing_grant_matches(rule, name, command, &domains, &project_path))
    }

    async fn gated_execute(&self, call_id: &str, name: &str, args: &Value) -> Result<String, AgentError> {
        // Codex command safety + Starlark `.rules` execpolicy: under
        // accept_edits (on-request + workspace-write) allow-listed commands
        // run without a prompt; forbidden rules block outright. Requested
        // network domains are only granted through a prompt (or a matching
        // standing grant), never by the known-safe bypass.
        let is_command = name == "run_command" || name == "exec_command";
        let mut dangerous = false;
        if is_command {
            let cmd_str = command_safety::command_from_args(args);
            dangerous = command_safety::is_dangerous_command(&cmd_str);
            let policy = self.load_exec_policy();
            let requests_network = !sorted_domains(args).is_empty();
            match command_safety::command_bypasses_prompt_with_policy(&self.mode, &cmd_str, Some(&policy)) {
                Ok(true) if !requests_network => {
                    return self.execute_tool(name, args).await.map_err(AgentError::Failed)
                }
                Ok(_) => {}
                Err(reason) => {
                    return Ok(json!({
                        "ok": false,
                        "error": format!("Command blocked by execpolicy rule: {reason}")
                    })
                    .to_string());
                }
            }
        }
        // Outside Full Access, writes to protected paths (`.git`, `.locagens`,
        // … — what the command sandbox also protects) and dangerous commands
        // always prompt: no standing grant or "Allow for Run" covers them,
        // and the answer approves this one call only.
        let protected = if self.mode == "auto" {
            None
        } else {
            tools::protected_mutation_target(&self.project_path, name, args)
        };
        let must_prompt = protected.is_some() || (dangerous && self.mode != "auto");
        if !must_prompt
            && (!tools::needs_permission(&self.mode, name)
                || self.handle.tool_allowed_for_run(name)
                || self.standing_grant(name, args))
        {
            return self.execute_with_turn_diff(name, args).await;
        }

        // Pause for an interactive decision: status → awaiting_permission,
        // permission_requested SSE, then wait for POST /api/runs/:id/permission.
        let tool_call = json!({
            "id": call_id,
            "function": {
                "name": name,
                "arguments": serde_json::to_string(args).unwrap_or_else(|_| "{}".into()),
            }
        });
        let mut preview = tools::build_preview(&self.project_path, name, args);
        if let (Some(path), Some(Value::Object(fields))) = (protected.as_ref(), preview.as_mut()) {
            fields.insert("protectedPath".into(), json!(path));
        }
        match self.permission_hook(call_id, name, args).await {
            hook_calls::PermissionVerdict::Allow => return self.execute_with_turn_diff(name, args).await,
            hook_calls::PermissionVerdict::Deny(reason) => return Ok(format!("Denied by PermissionRequest hook: {reason}")),
            hook_calls::PermissionVerdict::Ask => {}
        }
        // One shared prompt path: queues behind other sub-agents' and network
        // prompts, and its slot/status are restored even if this future is
        // aborted (`close_agent` mid-prompt).
        self.notification_hook("permission_prompt", &format!("Locagens needs permission to run {name}")).await;
        let prompt = PermissionPrompt {
            tool_call,
            preview,
            agent_role: self.agent_role.clone(),
            agent_name: self.agent_name.clone(),
            armed: None,
        };
        let Some(decision) = self.handle.ask_permission(&self.state.db, prompt).await else {
            return Err(AgentError::Cancelled);
        };
        if self.stop_requested() {
            return Err(AgentError::Cancelled);
        }

        // Standing grants are persisted here only (QA-031), never by the facade.
        // Unknown answers deny (QA-006); protected / dangerous calls are
        // approved once, never as a standing or run-wide grant.
        let mut decision = PermissionDecision::parse_or_deny(&decision);
        if must_prompt && decision.allows() {
            decision = PermissionDecision::AllowOnce;
        }
        match decision {
            PermissionDecision::Deny => Ok("The user denied permission for this tool call.".to_string()),
            PermissionDecision::AllowRun => {
                self.handle.allow_tool_for_run(name);
                self.execute_with_turn_diff(name, args).await
            }
            PermissionDecision::AllowProject | PermissionDecision::AllowAlways => {
                // Standing grants live in `permissions.json` under the
                // Locagens home and match the exact command + domains
                // (`standing_grant_matches`); nothing is written into the
                // project folder.
                self.state.db.upsert_permission(
                    decision.standing_scope().unwrap_or("project"),
                    &self.project_path.display().to_string(),
                    name,
                    extract_command_str(args),
                    sorted_domains(args),
                    decision.as_str(),
                );
                self.execute_with_turn_diff(name, args).await
            }
            PermissionDecision::AllowOnce => self.execute_with_turn_diff(name, args).await,
        }
    }

    /// Executes a gated tool, snapshotting turn-diff baselines for mutating
    /// tools first and emitting `turn_diff_updated` afterwards.
    async fn execute_with_turn_diff(&self, name: &str, args: &Value) -> Result<String, AgentError> {
        self.snapshot_turn_diff(name, args);
        let result = self.execute_tool(name, args).await;
        if result.is_ok() {
            self.emit_turn_diff();
        }
        result.map_err(AgentError::Failed)
    }
}

/// Shared HTTP client for streaming calls (see [`llm::shared_http_client`]:
/// connection reuse, connect timeout, per-read idle timeout).
fn endpoint_http(_state: &AppState) -> reqwest::Client {
    llm::shared_http_client()
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

/// Normalizes both `update_plan` schemas into `(title, body, tasks)` where
/// tasks are Locagens `{ text, status }` objects.
fn normalize_plan_args(args: &Value) -> (String, Option<String>, Value) {
    let items = args
        .get("tasks")
        .or_else(|| args.get("plan"))
        .cloned()
        .unwrap_or(json!([]));
    let mut first_text: Option<String> = None;
    let tasks = match items {
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .filter_map(|item| match item {
                    Value::String(text) => {
                        let text = text.trim().to_string();
                        if text.is_empty() {
                            None
                        } else {
                            first_text.get_or_insert_with(|| text.clone());
                            Some(json!({ "text": text, "status": "pending" }))
                        }
                    }
                    Value::Object(_) => {
                        let text = item
                            .get("text")
                            .or_else(|| item.get("step"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .trim()
                            .to_string();
                        if text.is_empty() {
                            None
                        } else {
                            first_text.get_or_insert_with(|| text.clone());
                            let status = item
                                .get("status")
                                .and_then(Value::as_str)
                                .unwrap_or("pending")
                                .to_string();
                            let status = ["pending", "in_progress", "completed"]
                                .iter()
                                .any(|valid| *valid == status)
                                .then_some(status.clone())
                                .unwrap_or_else(|| "pending".to_string());
                            Some(json!({ "text": text, "status": status }))
                        }
                    }
                    _ => None,
                })
                .collect(),
        ),
        _ => json!([]),
    };
    let title = args
        .get("title")
        .or_else(|| args.get("summary"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
        .or_else(|| first_text.take())
        .unwrap_or_default();
    let body = args
        .get("body")
        .or_else(|| args.get("explanation"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|body| !body.trim().is_empty());
    (title, body, tasks)
}

/// Normalizes Codex `request_user_input` questions for the QuestionCard flow:
/// every question ends up with a header and at least two options.
fn normalize_request_user_input(args: &Value) -> Value {
    let questions = args.get("questions").cloned().unwrap_or(json!([]));
    let normalized = questions
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|question| {
                    let header = question
                        .get("header")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|h| !h.is_empty())
                        .unwrap_or("Choice")
                        .to_string();
                    let options = match question.get("options") {
                        Some(options) if options.is_array() && !options.as_array().unwrap().is_empty() => {
                            options.clone()
                        }
                        _ => json!([{ "label": "Yes" }, { "label": "No" }]),
                    };
                    let mut question = question.clone();
                    let object = question.as_object_mut().unwrap();
                    object.insert("header".into(), json!(header));
                    object.insert("options".into(), options);
                    question
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({ "questions": normalized })
}

/// The command string of a `run_command`/`exec_command` call: `command`
/// first, Codex's `cmd` as fallback.
fn extract_command_str(args: &Value) -> &str {
    args.get("command")
        .or_else(|| args.get("cmd"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn sorted_domains(args: &Value) -> Vec<String> {
    args.get("network_domains")
        .or_else(|| args.get("networkDomains"))
        .and_then(Value::as_array)
        .map(|items| {
            let mut domains: Vec<String> = items.iter().filter_map(Value::as_str).map(str::to_string).collect();
            domains.sort();
            domains
        })
        .unwrap_or_default()
}

/// Whether a persisted permission rule covers this exact tool call.
fn standing_grant_matches(
    rule: &crate::db::PermissionRule,
    tool: &str,
    command: &str,
    domains: &[String],
    project_path: &str,
) -> bool {
    if !rule.status.starts_with("allow") || rule.tool != tool {
        return false;
    }
    let scope_ok = rule.scope == "global" || (rule.scope == "project" && rule.project_path == project_path);
    if !scope_ok {
        return false;
    }
    if tool == "run_command" || tool == "exec_command" {
        // An empty rule command used to be saved for exec_command (its args
        // use `cmd`); never let it act as a wildcard.
        if rule.command.is_empty() || command.is_empty() || rule.command != command {
            return false;
        }
        if !rule.network_domains.is_empty() {
            let mut rule_domains = rule.network_domains.clone();
            rule_domains.sort();
            if rule_domains != domains {
                return false;
            }
        }
    }
    true
}

// ----------------------------------------------------------------------
// Sub-agent execution (owned spec so agents can run in background tasks)
// ----------------------------------------------------------------------

/// Everything needed to run one sub-agent — owned so it can be moved into a
/// spawned tokio task (`spawn_agent`).
struct SubAgentSpec {
    state: Arc<AppState>,
    handle: Arc<RunHandle>,
    run_id: String,
    project_path: PathBuf,
    mode: String,
    provider_id: String,
    model: String,
    reasoning_effort: Option<String>,
    /// Role name (`explorer`, `worker` or a custom `agents/*.toml` role);
    /// becomes `agentRole` on the agent's messages.
    role: String,
    /// Extra role instructions appended to the sub-agent system prompt.
    instructions: Option<String>,
    title: String,
    task: String,
    readonly: bool,
    depth_left: u32,
}

/// Builds the loop of one sub-agent (history seeded with its task). The
/// persistent agents in `agents.rs` keep it alive across turns.
async fn build_sub_loop(
    spec: SubAgentSpec,
    agent_inbox: Option<mpsc::UnboundedReceiver<agents::AgentCommand>>,
    turn_diff: Arc<TurnDiffTracker>,
    interrupt: Option<Arc<AtomicBool>>,
) -> AgentLoop {
    let providers = spec.state.pi.list_providers().await;
    let project_display = spec.project_path.display().to_string();
    let title = spec.title.clone();
    let task = spec.task.clone();
    let mut system = sub_agent_system_prompt(&spec.role, &title, &task, &project_display, spec.readonly);
    if let Some(instructions) = spec.instructions.as_deref() {
        system.push_str("\nRole instructions:\n");
        system.push_str(instructions);
        system.push('\n');
    }
    AgentLoop {
        state: spec.state,
        handle: spec.handle,
        run_id: spec.run_id,
        project_path: spec.project_path,
        mode: spec.mode,
        provider_display_name: provider_display_name(&providers, &spec.provider_id),
        provider_id: spec.provider_id,
        model: spec.model,
        reasoning_effort: spec.reasoning_effort,
        agent_role: Some(spec.role),
        agent_name: Some(spec.title.clone()),
        history: vec![llm::LlmMessage {
            role: "user".into(),
            content: spec.task.clone(),
            ..Default::default()
        }],
        tools: if spec.readonly { tools::readonly_tool_defs(false) } else { worker_tool_defs() },
        system,
        depth_left: spec.depth_left,
        turn_diff,
        agent_inbox,
        extras: LoopExtras { interrupt, readonly: spec.readonly, ..Default::default() },
    }
}

// ----------------------------------------------------------------------
// Drive task
// ----------------------------------------------------------------------

/// The spawned task that owns one run: seeds history, drives the loop and
/// lands the terminal status + events.
/// Handle cleanup (sub-agents, exec sessions, handle removal) is done by the
/// supervisor in [`Orchestrator::spawn`], so it also runs after a panic.
async fn drive_run(state: Arc<AppState>, handle: Arc<RunHandle>) {
    let run_id = handle.run_id.clone();
    if handle.is_cancelled() {
        return;
    }

    let Some(run) = state.db.get_run_full(&run_id) else {
        return;
    };

    state.db.patch_run(&run_id, |run| {
        run.status = "generating".into();
        run.error_message = None;
    });
    handle.emit(json!({ "type": "run_started", "runId": run_id }));
    handle.emit(json!({ "type": "status_changed", "status": "generating" }));

    let mut history = seed_history(&state.db.list_messages(&run_id));
    let providers = state.pi.list_providers().await;
    let mut session_start_context: Option<String> = None;

    let run_full = state.db.get_run_full(&run_id).unwrap_or_else(|| run.clone());
    let tool_names = tool_names_for_mode(&run_full.mode);

    // Live MCP tools (qualified `mcp__<server>__<tool>` defs) are exposed in
    // every non-readonly mode.
    let mut tools = tools::filter_tool_defs(&tool_names);
    if run_full.mode != "chat" && run_full.mode != "plan" && !run_full.project_path.is_empty() {
        tools.extend(crate::services::mcp::discover_tool_defs(Some(&run_full.project_path)).await);
    }
    // Large MCP tool sets are loaded on demand through `tool_search`.
    let (mut tools, deferred_tools) = tool_search::split_deferred(tools);
    agents::apply_project_roles(&mut tools, Path::new(&run_full.project_path));

    // SessionStart lifecycle hook: additional context is appended to the
    // system prompt after the agent is constructed.
    if !run_full.project_path.is_empty() {
        let session_start = crate::services::hooks::run_hook_event(
            "SessionStart",
            Path::new(&run_full.project_path),
            &json!({
                "session_id": run_id,
                "cwd": run_full.project_path,
                "hook_event_name": "SessionStart",
                "source": if history.len() > 1 { "resume" } else { "startup" },
                "model": run_full.model,
                "permission_mode": run_full.mode,
            }),
        )
        .await;
        if let Some(context) = session_start.context_joined() {
            session_start_context = Some(context);
        }
    }

    // Codex-style <environment_context> block as the leading user message.
    if !run_full.project_path.is_empty() && !history.is_empty() {
        let context = env_context::environment_context_for_run(&run_full.mode, &run_full.project_path);
        history.insert(
            0,
            llm::LlmMessage {
                role: "user".into(),
                content: context,
                ..Default::default()
            },
        );
    }

    let mut agent = AgentLoop {
        handle: handle.clone(),
        run_id: run_id.clone(),
        project_path: PathBuf::from(&run_full.project_path),
        mode: run_full.mode.clone(),
        provider_display_name: provider_display_name(&providers, &run_full.provider_id),
        provider_id: run_full.provider_id.clone(),
        model: run_full.model.clone(),
        reasoning_effort: run_full.reasoning_effort.clone(),
        agent_role: None,
        agent_name: None,
        history,
        tools,
        system: system_prompt(&state, &run_full),
        depth_left: 1,
        state: state.clone(),
        turn_diff: Arc::new(TurnDiffTracker::new()),
        agent_inbox: None,
        extras: LoopExtras { deferred_tools, interrupt: None, readonly: false },
    };
    if let Some(context) = session_start_context {
        agent.system.push_str("\n\n");
        agent.system.push_str(&context);
    }

    // `$skill` mentions from the latest user prompt inject the full skill
    // bodies as context (budgeted so a pile of mentions can't blow context).
    let latest_user_prompt = state
        .db
        .list_messages(&run_id)
        .into_iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.content)
        .unwrap_or_else(|| run_full.task.clone());
    let prompt_block = agent.user_prompt_hook(&latest_user_prompt).await;
    let mentioned =
        crate::services::skills::resolve_mentioned_skills(&run_full.project_path, &latest_user_prompt);
    let mut skill_budget: usize = 16 * 1024;
    let mut skill_context = String::new();
    for skill in mentioned {
        let Some(body) = skill.body.as_deref().filter(|body| !body.trim().is_empty()) else {
            continue;
        };
        let take = locagens_protocol::text::floor_char_boundary(body, skill_budget);
        if take == 0 {
            break;
        }
        skill_context.push_str(&format!(
            "\n<skill_context name=\"{}\">\n",
            skill.name
        ));
        skill_context.push_str(&body[..take]);
        if take < body.len() {
            skill_context.push_str("\n[skill body truncated]");
        }
        skill_context.push_str("\n</skill_context>");
        skill_budget -= take;
        if skill_budget == 0 {
            break;
        }
    }
    if !skill_context.is_empty() {
        agent.system.push_str("\n\n## Mentioned skills\n");
        agent.system.push_str(&skill_context);
    }

    // Pre-turn git baseline (best-effort): lets `/undo` restore the exact
    // pre-turn state. Never fails a non-git run.
    if run_full.mode != "plan" && run_full.mode != "chat" && !run_full.project_path.is_empty() {
        let project = PathBuf::from(&run_full.project_path);
        if let Ok(Some(snapshot)) = crate::api::projects::create_git_snapshot_for_path(&project, &run_id).await {
            *lock_ignoring_poison(&handle.turn_snapshot) = Some((project, snapshot.id));
        }
    }

    let result = match prompt_block {
        Some(reason) => Err(AgentError::Failed(format!("Blocked by UserPromptSubmit hook: {reason}"))),
        None => agent.run(MAX_ROUNDS).await,
    };
    handle.agents.shutdown();
    // Post-turn tree before the terminal status, so an immediate `/undo`
    // already knows which files this turn changed.
    handle.record_turn_end().await;
    hook_calls::session_end_hook(&run_full.project_path, &run_id, &result).await;

    // Land the terminal state unless the route already recorded it (cancel).
    // Drives of one run never overlap (`Orchestrator::prepare_drive`), so
    // the run's status here belongs to this drive.
    let current = state.db.get_run_full(&run_id);
    if current.map(|run| TERMINAL_STATUSES.contains(&run.status.as_str())).unwrap_or(false) {
        return;
    }
    match result {
        Ok(final_output) => {
            let final_output = review::finalize_review_run(&state.db, &run_full, &handle, final_output);
            state.db.patch_run(&run_id, |run| {
                run.status = "done".into();
                run.error_message = None;
            });
            handle.emit(json!({ "type": "status_changed", "status": "done" }));
            handle.emit(json!({ "type": "run_completed", "finalOutput": final_output }));
        }
        Err(AgentError::Cancelled) => {
            state.db.patch_run(&run_id, |run| run.status = "cancelled".into());
            handle.emit(json!({ "type": "status_changed", "status": "cancelled" }));
        }
        Err(AgentError::Failed(err)) => {
            state.db.patch_run(&run_id, |run| {
                run.status = "failed".into();
                run.error_message = Some(err.clone());
            });
            handle.emit(json!({ "type": "status_changed", "status": "failed" }));
            handle.emit(json!({ "type": "run_failed", "errorMessage": err }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{PermissionRule, RunMessage};

    fn message(role: &str, content: &str, raw: Option<&str>, agent_role: Option<&str>) -> RunMessage {
        RunMessage {
            id: format!("m-{}", uuid::Uuid::new_v4()),
            run_id: "r".into(),
            role: role.into(),
            content: content.into(),
            raw_response: raw.map(str::to_string),
            agent_role: agent_role.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn history_seeding_pairs_tool_calls_with_responses() {
        let messages = vec![
            message("user", "do it", None, None),
            message("assistant", "working", Some(r#"[{"id":"c1","function":{"name":"t","arguments":"{}"}}]"#), None),
            message("tool", "result", Some("c1"), None),
            // Sub-agent messages are folded away.
            message("assistant", "sub output", None, Some("coder")),
            message("assistant", "final", None, None),
        ];
        let history = seed_history(&messages);
        assert_eq!(history.len(), 4);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[1].tool_calls.as_ref().unwrap()[0]["id"], "c1");
        assert_eq!(history[2].tool_call_id.as_deref(), Some("c1"));
        assert_eq!(history[3].content, "final");
    }

    #[test]
    fn standing_grants_match_scope_tool_command_and_domains() {
        let rule = PermissionRule {
            id: 1,
            scope: "project".into(),
            project_path: "C:/ws".into(),
            tool: "run_command".into(),
            command: "npm test".into(),
            network_domains: vec!["registry.npmjs.org".into()],
            status: "allow_always".into(),
        };
        assert!(standing_grant_matches(&rule, "run_command", "npm test", &["registry.npmjs.org".to_string()], "C:/ws"));
        assert!(!standing_grant_matches(&rule, "run_command", "npm publish", &[], "C:/ws"));
        assert!(!standing_grant_matches(&rule, "run_command", "npm test", &["evil.com".to_string()], "C:/ws"));
        assert!(!standing_grant_matches(&rule, "run_command", "npm test", &["registry.npmjs.org".to_string()], "C:/other"));
        assert!(!standing_grant_matches(&rule, "fetch_url", "", &[], "C:/ws"));

        let global = PermissionRule {
            scope: "global".into(),
            tool: "fetch_url".into(),
            command: String::new(),
            network_domains: vec![],
            status: "allow_run".into(),
            ..rule.clone()
        };
        assert!(standing_grant_matches(&global, "fetch_url", "", &[], "C:/anywhere"));
        let denied = PermissionRule { status: "deny".into(), ..global };
        assert!(!standing_grant_matches(&denied, "fetch_url", "", &[], "C:/anywhere"));
    }

    #[test]
    fn standing_grants_match_exec_command_by_cmd() {
        let rule = PermissionRule {
            id: 2,
            scope: "global".into(),
            project_path: String::new(),
            tool: "exec_command".into(),
            command: "cargo test".into(),
            network_domains: vec![],
            status: "allow_always".into(),
        };
        // `cmd` argument form matches the saved command.
        assert!(standing_grant_matches(&rule, "exec_command", "cargo test", &[], "C:/ws"));
        assert!(!standing_grant_matches(&rule, "exec_command", "cargo publish", &[], "C:/ws"));
        // A different tool never inherits a command rule.
        assert!(!standing_grant_matches(&rule, "run_command", "cargo test", &[], "C:/ws"));

        // Empty saved command must NOT act as a wildcard for exec_command
        // (the historical bug: exec_command saved `command: ""`).
        let wildcard = PermissionRule { command: String::new(), ..rule.clone() };
        assert!(!standing_grant_matches(&wildcard, "exec_command", "anything", &[], "C:/ws"));
        // And an empty live command never matches either.
        let live = PermissionRule { command: "cargo test".into(), ..rule };
        assert!(!standing_grant_matches(&live, "exec_command", "", &[], "C:/ws"));
    }

    #[test]
    fn extract_command_reads_command_then_cmd() {
        assert_eq!(extract_command_str(&json!({ "command": "npm test" })), "npm test");
        assert_eq!(extract_command_str(&json!({ "cmd": "cargo check" })), "cargo check");
        assert_eq!(extract_command_str(&json!({})), "");
    }

    #[test]
    fn tool_names_follow_mode() {
        let chat = tool_names_for_mode("chat");
        assert!(chat.contains(&"read_file"));
        assert!(chat.contains(&"compact_context"));
        assert!(!chat.contains(&"update_plan"));
        assert!(!chat.contains(&"fetch_url"));
        assert!(!chat.contains(&"spawn_agent"));
        assert!(!chat.contains(&"write_file"));
        assert!(!chat.contains(&"run_command"));
        assert!(!chat.contains(&"exec_command"));
        assert!(!chat.contains(&"write_stdin"));
        assert!(!chat.contains(&"apply_patch"));

        let plan = tool_names_for_mode("plan");
        for chat_tool in chat {
            assert!(plan.contains(&chat_tool), "plan must keep chat tool {chat_tool}");
        }
        assert!(plan.contains(&"update_plan"));
        assert!(plan.contains(&"fetch_url"));
        assert!(plan.contains(&"spawn_agent"));
        assert!(plan.contains(&"spawn_agents"));
        assert!(!plan.contains(&"write_file"));
        assert!(!plan.contains(&"apply_patch"));
        assert!(!plan.contains(&"run_command"));
        assert!(!plan.contains(&"exec_command"));
        assert!(!plan.contains(&"write_stdin"));

        let full = tool_names_for_mode("accept_edits");
        assert!(full.contains(&"write_file"));
        assert!(full.contains(&"exec_command"));
        assert!(full.contains(&"write_stdin"));
        assert!(full.contains(&"apply_patch"));
        assert!(full.contains(&"spawn_agents"));
        let auto = tool_names_for_mode("auto");
        assert!(auto.contains(&"run_command"));
    }

    #[test]
    fn sub_agent_roles_are_read_only_in_readonly_modes() {
        // `spawn_agents` and `spawn_agent` share `resolve_role` (QA-033).
        let roles = agent_roles::builtin_roles();
        assert!(agent_roles::resolve_role(&roles, Some("worker"), "plan").unwrap().readonly);
        assert!(!agent_roles::resolve_role(&roles, Some("worker"), "auto").unwrap().readonly);
        assert_eq!(agent_roles::resolve_role(&roles, None, "auto").unwrap().name, "explorer");
    }

    #[test]
    fn cost_computation_uses_per_million_pricing() {
        let info = crate::pi_providers::ModelInfo {
            id: "m".into(),
            name: "m".into(),
            cost: Some(json!({
                "currency": "USD",
                "inputPerMillion": 3.0,
                "outputPerMillion": 15.0,
                "cacheReadPerMillion": 0.3,
                "cacheWritePerMillion": 3.75
            })),
            ..Default::default()
        };
        let usage = llm::Usage {
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            cache_read_tokens: 500_000,
            cache_write_tokens: 0,
            provider_cost: None,
        };
        let cost = cost_of(&usage, Some(&info));
        assert!((cost - (3.0 + 1.5 + 0.15)).abs() < 1e-9);
        assert_eq!(cost_of(&usage, None), 0.0);
    }

    #[test]
    fn update_plan_normalizes_both_schemas() {
        // Locagens schema.
        let (title, body, tasks) = normalize_plan_args(&json!({
            "title": "Ship it",
            "body": "rationale",
            "tasks": ["one", { "text": "two", "status": "completed" }]
        }));
        assert_eq!(title, "Ship it");
        assert_eq!(body.as_deref(), Some("rationale"));
        assert_eq!(tasks[0]["text"], "one");
        assert_eq!(tasks[0]["status"], "pending");
        assert_eq!(tasks[1]["text"], "two");
        assert_eq!(tasks[1]["status"], "completed");

        // Codex schema: explanation + plan[{step,status}], no title.
        let (title, body, tasks) = normalize_plan_args(&json!({
            "explanation": "why",
            "plan": [
                { "step": "research", "status": "completed" },
                { "step": "implement", "status": "in_progress" },
                { "step": "verify" }
            ]
        }));
        assert_eq!(title, "research");
        assert_eq!(body.as_deref(), Some("why"));
        assert_eq!(tasks[0]["text"], "research");
        assert_eq!(tasks[0]["status"], "completed");
        assert_eq!(tasks[2]["status"], "pending");
    }

    #[test]
    fn request_user_input_normalization_fills_defaults() {
        let normalized = normalize_request_user_input(&json!({
            "questions": [
                { "id": "q1", "question": "Proceed?" },
                { "question": "Which db?", "options": [{ "label": "SQLite" }] }
            ]
        }));
        let questions = normalized["questions"].as_array().unwrap();
        assert_eq!(questions[0]["header"], "Choice");
        assert_eq!(questions[0]["options"][0]["label"], "Yes");
        assert_eq!(questions[0]["question"], "Proceed?");
        assert_eq!(questions[1]["header"], "Choice");
        assert_eq!(questions[1]["options"][0]["label"], "SQLite");
    }

    #[test]
    fn reasoning_support_heuristic() {
        assert!(reasoning_supported("glm-5.3-flash", None));
        assert!(reasoning_supported("gpt-5-mini", None));
        assert!(!reasoning_supported("text-embedding-3", None));
        let info = crate::pi_providers::ModelInfo {
            id: "whatever".into(),
            name: "whatever".into(),
            reasoning: Some(true),
            ..Default::default()
        };
        assert!(reasoning_supported("whatever", Some(&info)));
    }
}
