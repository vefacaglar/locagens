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

pub mod llm;
pub mod tools;

use crate::db::{Plan, RunMessage, UsageLogEntry};
use crate::pi_providers::PiProviderManager;
use crate::AppState;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, watch};

const MAX_ROUNDS: usize = 40;
const SUB_AGENT_MAX_ROUNDS: usize = 24;
const MAX_SUB_AGENTS: usize = 4;
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

/// Full (worker) sub-agent toolset: workspace + dangerous tools.
fn worker_tool_defs() -> Vec<Value> {
    tools::filter_tool_defs(&WORKSPACE_TOOLS)
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
}

impl Default for PermissionSlot {
    fn default() -> Self {
        Self { pending: None, sender: None }
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

/// Shared, per-run coordination state handed to routes and the agent loop.
pub struct RunHandle {
    pub run_id: String,
    cancel_flag: Arc<AtomicBool>,
    cancelled_tx: watch::Sender<bool>,
    pub events: tokio::sync::broadcast::Sender<Value>,
    pub inbox: tokio::sync::Mutex<VecDeque<String>>,
    pub permission: tokio::sync::Mutex<PermissionSlot>,
    pub question: tokio::sync::Mutex<QuestionSlot>,
    pub allow_run: AtomicBool,
}

impl RunHandle {
    fn new(run_id: &str) -> Arc<Self> {
        let (cancelled_tx, _) = watch::channel(false);
        let (events, _) = tokio::sync::broadcast::channel(512);
        Arc::new(Self {
            run_id: run_id.to_string(),
            cancel_flag: Arc::new(AtomicBool::new(false)),
            cancelled_tx,
            events,
            inbox: tokio::sync::Mutex::new(VecDeque::new()),
            permission: tokio::sync::Mutex::new(PermissionSlot::default()),
            question: tokio::sync::Mutex::new(QuestionSlot::default()),
            allow_run: AtomicBool::new(false),
        })
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }

    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
        self.cancelled_tx.send_replace(true);
    }

    pub fn reset_cancel(&self) {
        self.cancel_flag.store(false, Ordering::SeqCst);
        self.cancelled_tx.send_replace(false);
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

/// Registry of live run handles.
pub struct Orchestrator {
    runs: Mutex<HashMap<String, Arc<RunHandle>>>,
}

impl Default for Orchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl Orchestrator {
    pub fn new() -> Self {
        Self { runs: Mutex::new(HashMap::new()) }
    }

    /// The live handle for a run, if the orchestrator knows about it.
    pub fn handle(&self, run_id: &str) -> Option<Arc<RunHandle>> {
        self.runs.lock().expect("orchestrator lock poisoned").get(run_id).cloned()
    }

    /// Gets or creates the handle for a run.
    pub fn ensure_handle(&self, run_id: &str) -> Arc<RunHandle> {
        let mut runs = self.runs.lock().expect("orchestrator lock poisoned");
        runs.entry(run_id.to_string()).or_insert_with(|| RunHandle::new(run_id)).clone()
    }

    /// Forgets a finished run's handle (its terminal state lives in the Db).
    pub fn drop_handle(&self, run_id: &str) {
        self.runs.lock().expect("orchestrator lock poisoned").remove(run_id);
    }

    /// Emits a `status_changed` event on a live handle, if one exists.
    pub fn emit_status(&self, run_id: &str, status: &str) {
        if let Some(handle) = self.handle(run_id) {
            handle.emit(json!({ "type": "status_changed", "status": status }));
        }
    }

    /// Spawns the drive task for a run that already has its initial user
    /// message persisted.
    pub fn spawn(self: &Arc<Self>, state: Arc<AppState>, run_id: &str) {
        let handle = self.ensure_handle(run_id);
        let orchestrator = self.clone();
        tokio::spawn(async move {
            drive_run(state, handle, orchestrator).await;
        });
    }
}

// ----------------------------------------------------------------------
// Errors
// ----------------------------------------------------------------------

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
    Ok(llm::Endpoint {
        base_url,
        api,
        api_key,
        session_id: session_id.to_string(),
        model: model.to_string(),
        reasoning_effort: reasoning_effort.filter(|e| !e.is_empty()).map(str::to_string),
        max_tokens: info.as_ref().and_then(|info| info.max_tokens),
        reasoning_supported: reasoning_supported(model, info.as_ref()),
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
    if run.system_prompt.as_deref().map(str::trim).map(str::is_empty) == Some(false)
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
        return prompt;
    }
    base_system_prompt(state, run)
}

fn base_system_prompt(state: &AppState, run: &crate::db::Run) -> String {
    let project = if run.project_path.is_empty() {
        "No project folder selected."
    } else {
        "Workspace root (all file tool paths are relative to it):"
    };
    format!(
        "You are the Locagens coding agent, working inside the user's project.\n\n\
{project} {path}\n{project_name}\n\n{mode}\n{memories}\n\n\
## Working style\n\
- Inspect before changing: read the relevant files first.\n\
- Prefer edit_file for small changes; write_file for new files.\n\
- Keep answers concise and concrete; summarize what you did.\n\
- Use update_plan for multi-step work so the user can follow progress.\n\
- Fan independent research or implementation sub-tasks out via spawn_agents (explorer / worker).\n\
- Use ask_user_question when a decision genuinely needs the user.\n",
        project = project,
        path = run.project_path,
        project_name = {
            let name = &run.project_name;
            if name.is_empty() { String::new() } else { format!("(project: {name})") }
        },
        mode = mode_rules(&run.mode),
        memories = memories_block(state, &run.project_path),
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
Work independently and completely; do not ask the user questions. When done, reply with a concise final summary of what you did and any important outcomes — it is returned to the coordinating agent as your task result.\n"
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
    agent_role: Option<&'static str>,
    agent_name: Option<String>,
    history: Vec<llm::LlmMessage>,
    tools: Vec<Value>,
    system: String,
    /// Remaining delegation depth (1 = may spawn sub-agents).
    depth_left: u32,
}

impl AgentLoop {
    fn check_cancel(&self) -> Result<(), AgentError> {
        if self.handle.is_cancelled() {
            Err(AgentError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn new_message(&self, role: &str, content: &str) -> RunMessage {
        RunMessage {
            id: format!("msg-{}", uuid::Uuid::new_v4()),
            run_id: self.run_id.clone(),
            role: role.to_string(),
            agent_role: self.agent_role.map(str::to_string),
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
        for round in 1..=max_rounds {
            self.check_cancel()?;
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

            let stream = llm::stream_chat(
                &endpoint_http(&self.state),
                &endpoint,
                &self.system,
                &self.history,
                &self.tools,
                &mut |content_delta: &str, reasoning_delta: &str| {
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
                            .update_message_full(&self.run_id, &message_id, &content, Some(&reasoning), None);
                        last_persist = Instant::now();
                    }
                },
                &|| self.handle.is_cancelled(),
            )
            .await;

            let outcome = match stream {
                Ok(outcome) => outcome,
                Err(err) if err.to_string() == "cancelled" || self.handle.is_cancelled() => {
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
                    agent_role: Some(self.agent_role.unwrap_or("planner").to_string()),
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
                // Final answer.
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

                let result = match self.dispatch_tool(&call_id, &name, &args).await {
                    Ok(result) => result,
                    Err(AgentError::Cancelled) => return Err(AgentError::Cancelled),
                    Err(AgentError::Failed(err)) => format!("Tool error: {err}"),
                };

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
            for queued_content in queued {
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

    /// Executes one tool call, routing interactive/delegation tools to the
    /// orchestrator and workspace tools through permission gating.
    async fn dispatch_tool(&self, call_id: &str, name: &str, args: &Value) -> Result<String, AgentError> {
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
            "spawn_agents" if self.depth_left > 0 => self.spawn_agents(args).await,
            name if WORKSPACE_TOOLS.contains(&name) => self.gated_execute(call_id, name, args).await,
            other => Err(AgentError::Failed(format!("Unknown tool \"{other}\"."))),
        }
    }

    async fn update_plan(&self, args: &Value) -> Result<String, AgentError> {
        let title = args.get("title").and_then(Value::as_str).unwrap_or_default().trim().to_string();
        if title.is_empty() {
            return Err(AgentError::Failed("update_plan: title is empty.".into()));
        }
        let tasks = args.get("tasks").cloned().unwrap_or(json!([]));
        let tasks = match tasks {
            Value::Array(items) => Value::Array(
                items
                    .into_iter()
                    .filter_map(|item| match item {
                        Value::String(text) => Some(json!({ "text": text, "status": "pending" })),
                        Value::Object(_) => {
                            let text = item.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
                            if text.is_empty() {
                                None
                            } else {
                                let status = item.get("status").and_then(Value::as_str).unwrap_or("pending").to_string();
                                let status = ["pending", "in_progress", "completed"]
                                    .iter()
                                    .any(|s| *s == status)
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

        let existing = self.state.db.get_plan(&self.run_id);
        let now = crate::util::now_iso();
        let plan = match existing {
            Some(mut plan) => {
                plan.title = title;
                plan.body = args.get("body").and_then(Value::as_str).map(str::to_string);
                plan.tasks = tasks;
                plan.version += 1;
                plan.updated_at = now;
                plan
            }
            None => Plan {
                id: format!("plan-{}", uuid::Uuid::new_v4()),
                run_id: self.run_id.clone(),
                title,
                body: args.get("body").and_then(Value::as_str).map(str::to_string),
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

    // -- Delegation ------------------------------------------------------

    async fn run_sub_agent(
        &self,
        role: &'static str,
        title: &str,
        provider_id: &str,
        model: &str,
        reasoning: Option<&str>,
        task: &str,
        readonly: bool,
    ) -> Result<String, AgentError> {
        // Empty provider/model falls back to the run's main model.
        let provider_id = if provider_id.trim().is_empty() { self.provider_id.clone() } else { provider_id.trim().to_string() };
        let model = if model.trim().is_empty() { self.model.clone() } else { model.trim().to_string() };
        let reasoning_effort = reasoning.map(str::to_string).filter(|r| !r.is_empty());
        let providers = self.state.pi.list_providers().await;
        let mut sub = AgentLoop {
            state: self.state.clone(),
            handle: self.handle.clone(),
            run_id: self.run_id.clone(),
            project_path: self.project_path.clone(),
            mode: self.mode.clone(),
            provider_display_name: provider_display_name(&providers, &provider_id),
            provider_id,
            model,
            reasoning_effort,
            agent_role: Some(role),
            agent_name: Some(title.to_string()),
            history: vec![llm::LlmMessage {
                role: "user".into(),
                content: task.to_string(),
                ..Default::default()
            }],
            tools: if readonly { tools::readonly_tool_defs(false) } else { worker_tool_defs() },
            system: sub_agent_system_prompt(role, title, task, &self.project_path.display().to_string(), readonly),
            depth_left: 0,
        };
        // Boxed to break the recursion cycle (sub-agent → run → dispatch →
        // spawn → sub-agent).
        Box::pin(sub.run(SUB_AGENT_MAX_ROUNDS)).await
    }

    async fn spawn_agents(&self, args: &Value) -> Result<String, AgentError> {
        let agents = args.get("agents").and_then(Value::as_array).cloned().unwrap_or_default();
        if agents.is_empty() {
            return Err(AgentError::Failed("spawn_agents: agents must be a non-empty array.".into()));
        }
        let mut results = Vec::new();
        for agent in agents.iter().take(MAX_SUB_AGENTS) {
            let role = agent.get("role").and_then(Value::as_str).unwrap_or("explorer");
            let task = agent.get("task").and_then(Value::as_str).unwrap_or_default().trim().to_string();
            if task.is_empty() {
                continue;
            }
            let (agent_role, readonly) = match role {
                "worker" => ("worker", false),
                _ => ("explorer", true),
            };
            let outcome = self.run_sub_agent(agent_role, &task, &self.provider_id, &self.model, self.reasoning_effort.as_deref(), &task, readonly).await;
            match outcome {
                Ok(answer) => results.push(format!("## {agent_role}: {task}\n{answer}")),
                Err(AgentError::Cancelled) => return Err(AgentError::Cancelled),
                Err(AgentError::Failed(err)) => results.push(format!("## {agent_role}: {task}\n(agent failed: {err})")),
            }
        }
        Ok(format!("Sub-agent results:\n\n{}", results.join("\n\n")))
    }

    // -- Permission gating -------------------------------------------------

    fn standing_grant(&self, name: &str, args: &Value) -> bool {
        let project_path = self.project_path.display().to_string();
        let command = args.get("command").and_then(Value::as_str).unwrap_or_default();
        let domains = sorted_domains(args);
        self.state
            .db
            .list_permissions()
            .iter()
            .any(|rule| standing_grant_matches(rule, name, command, &domains, &project_path))
    }

    async fn gated_execute(&self, call_id: &str, name: &str, args: &Value) -> Result<String, AgentError> {
        if !tools::needs_permission(&self.mode, name) || self.handle.allow_run.load(Ordering::SeqCst) || self.standing_grant(name, args) {
            return tools::execute(&self.project_path, name, args)
                .await
                .map_err(AgentError::Failed);
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
        let preview = tools::build_preview(&self.project_path, name, args);
        self.state.db.patch_run(&self.run_id, |run| run.status = "awaiting_permission".into());
        self.handle.emit(json!({ "type": "status_changed", "status": "awaiting_permission" }));
        self.handle.emit(json!({
            "type": "permission_requested",
            "runId": self.run_id,
            "toolCall": tool_call,
            "preview": preview,
        }));

        let (sender, receiver) = oneshot::channel::<String>();
        {
            let mut slot = self.handle.permission.lock().await;
            slot.pending = Some(crate::orchestrator::PendingPermission { tool_call, preview });
            slot.sender = Some(sender);
        }
        let decision = tokio::select! {
            decision = receiver => decision.ok(),
            _ = self.handle.cancelled() => None,
        };
        {
            let mut slot = self.handle.permission.lock().await;
            slot.pending = None;
            slot.sender = None;
        }
        if decision.is_none() {
            return Err(AgentError::Cancelled);
        }

        self.state.db.patch_run(&self.run_id, |run| run.status = "generating".into());
        self.handle.emit(json!({ "type": "status_changed", "status": "generating" }));

        let decision = decision.unwrap();
        match decision.as_str() {
            "deny" => Ok("The user denied permission for this tool call.".to_string()),
            "allow_run" => {
                self.handle.allow_run.store(true, Ordering::SeqCst);
                tools::execute(&self.project_path, name, args)
                    .await
                    .map_err(AgentError::Failed)
            }
            "allow_project" | "allow_always" => {
                let command = args.get("command").and_then(Value::as_str).unwrap_or_default().to_string();
                let domains = args
                    .get("network_domains")
                    .or_else(|| args.get("networkDomains"))
                    .and_then(Value::as_array)
                    .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
                    .unwrap_or_default();
                self.state.db.upsert_permission(
                    if decision == "allow_project" { "project" } else { "global" },
                    &self.project_path.display().to_string(),
                    name,
                    &command,
                    domains,
                    &decision,
                );
                tools::execute(&self.project_path, name, args)
                    .await
                    .map_err(AgentError::Failed)
            }
            // allow_once and unknown decisions just proceed once.
            _ => tools::execute(&self.project_path, name, args)
                .await
                .map_err(AgentError::Failed),
        }
    }
}

/// Shared HTTP client for streaming calls (long timeouts: streams can run
/// for minutes).
fn endpoint_http(_state: &AppState) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
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
    if tool == "run_command" {
        if !rule.command.is_empty() && rule.command != command {
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
// Drive task
// ----------------------------------------------------------------------

/// The spawned task that owns one run: seeds history, drives the loop and
/// lands the terminal status + events.
async fn drive_run(state: Arc<AppState>, handle: Arc<RunHandle>, orchestrator: Arc<Orchestrator>) {
    let run_id = handle.run_id.clone();
    handle.reset_cancel();

    let Some(run) = state.db.get_run_full(&run_id) else {
        orchestrator.drop_handle(&run_id);
        return;
    };

    state.db.patch_run(&run_id, |run| {
        run.status = "generating".into();
        run.error_message = None;
    });
    handle.emit(json!({ "type": "run_started", "runId": run_id }));
    handle.emit(json!({ "type": "status_changed", "status": "generating" }));

    let history = seed_history(&state.db.list_messages(&run_id));
    let providers = state.pi.list_providers().await;

    let run_full = state.db.get_run_full(&run_id).unwrap_or_else(|| run.clone());
    let mut tool_names: Vec<&str> = WORKSPACE_TOOLS.to_vec();
    tool_names.extend_from_slice(&["set_chat_title", "update_plan", "remember", "load_skill", "ask_user_question"]);
    tool_names.push("spawn_agents");

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
        tools: tools::filter_tool_defs(&tool_names),
        system: system_prompt(&state, &run_full),
        depth_left: 1,
        state: state.clone(),
    };

    let result = agent.run(MAX_ROUNDS).await;

    // Land the terminal state unless the route already recorded it (cancel).
    let current = state.db.get_run_full(&run_id);
    if current.map(|run| TERMINAL_STATUSES.contains(&run.status.as_str())).unwrap_or(false) {
        orchestrator.drop_handle(&run_id);
        return;
    }
    match result {
        Ok(final_output) => {
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
    orchestrator.drop_handle(&run_id);
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
