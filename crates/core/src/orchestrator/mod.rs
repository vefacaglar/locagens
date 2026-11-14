//! Pure Rust agent orchestrator.
//!
//! Owns the per-run handles (cancellation token, event broadcast channel,
//! mid-run message inbox, pending permission/question coordinators) and the
//! `AgentLoop` that drives model ↔ tool rounds for the main agent and every
//! spawned sub-agent (explorer / worker).
//!
//! Message flow:
//! ```txt
//! drive_run → seed history from messages.jsonl → AgentLoop::run
//!   round: stream LLM deltas → assistant RunMessage (+ throttled events)
//!          → execute tool calls (permission-gated) → tool RunMessages
//!          → fold queued mid-run user messages → next round
//! terminal: status_changed + run_completed / run_failed
//! ```

pub mod agent_roles;
pub mod auto_title;
pub mod agents;
pub mod agents_md;
pub mod apply_patch;
pub mod command_safety;
pub mod compact;
pub mod context_remaining;
pub mod env_context;
pub mod history;
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

mod agent_loop;
mod agent_round;
mod drive;
mod exec_session;
mod gating;
mod interactive;
mod prompts;
mod run_handle;

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
/// Throttling for `message_updated` events while tokens stream in.
const STREAM_EMIT_INTERVAL: Duration = Duration::from_millis(60);
/// How often the loop hands streamed text to the store. The store itself
/// writes at most one `message_update` checkpoint every 5 s per message
/// (QA-035), so this only bounds how fresh that checkpoint is.
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
/// bookkeeping tools (no MCP resource tools: they start MCP servers, FR-075), `plan` adds planning/research/delegation tools, and
/// every other mode gets the full toolset.
fn tool_names_for_mode(mode: &str) -> Vec<&'static str> {
    let chat_tools: &[&str] = &[
        "read_file", "list_directory", "search_files", "view_image",
        "set_chat_title", "remember", "load_skill", "compact_context",
        "ask_user_question", "request_user_input",
        "get_context_remaining", "host_executable",
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

#[derive(Default)]
pub struct PermissionSlot {
    pub pending: Option<PendingPermission>,
    pub sender: Option<oneshot::Sender<String>>,
    /// Id of the prompt that owns the slot (see [`RunHandle::ask_permission`]),
    /// so a late cleanup never clears a newer prompt.
    prompt_id: u64,
    /// Sub-agent that asked (`None` for the main agent), for the card.
    agent_role: Option<String>,
    agent_name: Option<String>,
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

#[derive(Default)]
pub struct QuestionSlot {
    /// The `questions` array of the active `ask_user_question` request.
    pub pending: Option<Value>,
    /// Resolves with `{ selections: string[][], notes: string[] }`.
    pub sender: Option<oneshot::Sender<Value>>,
    /// Id of the question that owns the slot, so its cleanup never clears
    /// a newer question.
    question_id: u64,
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
    /// Set (under the inbox lock) once the turn handed back its undelivered
    /// messages; later queueing is refused instead of silently lost.
    pub inbox_closed: AtomicBool,
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
    /// Paths this turn's file tools changed; `None` once a command (shell,
    /// MCP) ran that could have changed anything (FR-008).
    turn_paths: Mutex<Option<std::collections::BTreeSet<String>>>,
    /// Name the chat with a model call on this (first) turn
    /// ([`CreateRunPayload::auto_title`](crate::api::runs::CreateRunPayload)).
    pub auto_title: AtomicBool,
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
    /// [`Orchestrator::kill_exec_sessions`] is called (QA-027). Each carries
    /// its typed-input state for `write_stdin` gating (QA-012).
    exec_sessions: Mutex<HashMap<String, HashMap<String, exec_session::ExecSessionInput>>>,
}

impl Default for Orchestrator {
    fn default() -> Self {
        Self::new()
    }
}

fn lock_ignoring_poison<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
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
/// which are persisted in the tool message's `rawResponse`. The latest
/// compaction marker replaces everything before the message it kept, and the
/// result is [`history::sanitize`]d so a failed, cancelled or compacted turn
/// never makes the next request invalid.
pub fn seed_history(messages: &[RunMessage]) -> Vec<llm::LlmMessage> {
    let mut history = Vec::new();
    let mut from = 0;
    if let Some((index, first_kept)) = compact::latest_marker(messages) {
        history.push(llm::LlmMessage {
            role: "user".into(),
            content: messages[index].content.clone(),
            ..Default::default()
        });
        from = first_kept
            .and_then(|id| messages[..index].iter().position(|message| message.id == id))
            .unwrap_or(index + 1);
    }
    history.extend(messages[from..].iter().filter_map(project_message));
    history::sanitize(history::fold_events(history))
}

/// Role of a persisted note for the model about something that happened
/// between turns (an `/undo`). The UIs do not show it; the history folds
/// it into the next user message ([`history::fold_events`]).
pub const EVENT_ROLE: &str = "event";

/// One persisted main-agent message as an LLM message (`None` for system
/// and sub-agent messages).
fn project_message(message: &RunMessage) -> Option<llm::LlmMessage> {
    let replayed = matches!(message.role.as_str(), "user" | "assistant" | "tool" | EVENT_ROLE);
    (replayed && message.agent_role.is_none()).then(|| to_llm_message(message))
}

/// A persisted message in LLM form: an assistant's `rawResponse` holds its
/// tool calls, a tool message's holds the call id it answers.
fn to_llm_message(message: &RunMessage) -> llm::LlmMessage {
    let base = llm::LlmMessage {
        role: message.role.clone(),
        content: message.content.clone(),
        source_id: Some(message.id.clone()),
        images: message.images.clone(),
        ..Default::default()
    };
    match message.role.as_str() {
        "assistant" => llm::LlmMessage {
            tool_calls: message
                .raw_response
                .as_deref()
                .and_then(|raw| serde_json::from_str::<Vec<Value>>(raw).ok())
                .filter(|calls| !calls.is_empty()),
            thinking_blocks: message
                .thinking_blocks
                .as_ref()
                .and_then(|blocks| blocks.as_array().cloned())
                .filter(|blocks| !blocks.is_empty()),
            ..base
        },
        "tool" => llm::LlmMessage { tool_call_id: message.raw_response.clone(), ..base },
        _ => base,
    }
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

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
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

#[cfg(test)]
mod tests;
