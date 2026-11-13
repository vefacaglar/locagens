//! Multi-agent v2: persistent, named, concurrent sub-agents.
//!
//! Ported from Codex's `multi_agents_v2` tool family:
//!
//! | tool | semantics |
//! |------|-----------|
//! | `spawn_agent` | start a named agent (`task_name`, `message`, `agent_type`) that runs in the background |
//! | `send_message` | queue a message on an agent; does **not** start a turn |
//! | `followup_task` | send a task; starts a new turn when the agent is idle, otherwise folded in at the next round boundary |
//! | `wait_agent` | wait for a mailbox update from any agent (final status or message) |
//! | `interrupt_agent` | stop the agent's current turn; the agent stays alive |
//! | `close_agent` | shut the agent down and free its concurrency slot |
//! | `list_agents` | live agents and their statuses |
//!
//! Each agent is a long-lived task that owns its own conversation: it runs a
//! turn, reports the outcome to the parent's mailbox, then idles until the
//! next `followup_task`. Completed agents keep occupying one of
//! [`MAX_CONCURRENT_AGENTS`] slots until closed (as in Codex). Children share
//! the parent's `RunHandle`, so parent cancellation reaches them, their
//! permission requests surface through the parent run's permission flow, and
//! their messages stream as ordinary run messages tagged `agentRole` /
//! `agentName`. Depth stays capped at 1: children get no agent tools.
//!
//! Deviation from Codex: `wait_agent` also returns each update's content.
//! Codex injects the content into the parent as a mailbox message; Locagens
//! history seeding folds sub-agent output into the parent's tool responses so
//! that a reopened run sees it.

use super::agent_roles::{self, AgentRole};
use super::{AgentError, AgentLoop, RunHandle, SubAgentSpec, SUB_AGENT_MAX_ROUNDS};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::{mpsc, Notify};

/// Codex v2 default for concurrent agents per session.
pub const MAX_CONCURRENT_AGENTS: usize = 4;
pub const WAIT_DEFAULT_MS: u64 = 30_000;
pub const WAIT_MIN_MS: u64 = 10_000;
pub const WAIT_MAX_MS: u64 = 3_600_000;

pub const AGENT_TOOL_NAMES: [&str; 7] = [
    "spawn_agent",
    "send_message",
    "followup_task",
    "wait_agent",
    "interrupt_agent",
    "close_agent",
    "list_agents",
];

pub fn is_agent_tool(name: &str) -> bool {
    AGENT_TOOL_NAMES.contains(&name)
}

/// What a parent can push into an agent's inbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentCommand {
    /// Queued context; never starts a turn on its own.
    Message(String),
    /// A task; starts a turn when the agent is idle.
    Task(String),
}

impl AgentCommand {
    pub fn into_text(self) -> String {
        match self {
            Self::Message(text) | Self::Task(text) => text,
        }
    }
}

/// One mailbox entry: an agent reached a final status for a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentUpdate {
    pub agent_id: String,
    pub task_name: String,
    pub status: String,
    pub output: String,
}

impl AgentUpdate {
    fn to_json(&self) -> Value {
        json!({
            "agent_id": self.agent_id,
            "task_name": self.task_name,
            "status": self.status,
            "output": self.output,
        })
    }
}

struct SubAgent {
    id: String,
    task_name: String,
    /// Display name (`agentName` on messages).
    nickname: String,
    role: String,
    /// `running | completed | interrupted | failed | closed`.
    status: String,
    commands: Option<mpsc::UnboundedSender<AgentCommand>>,
    interrupt: Arc<AtomicBool>,
    abort: Option<tokio::task::AbortHandle>,
}

/// Snapshot handed to tool code (no locks held while it is used).
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub id: String,
    pub task_name: String,
    pub nickname: String,
    pub role: String,
    pub status: String,
}

impl SubAgent {
    fn info(&self) -> AgentInfo {
        AgentInfo {
            id: self.id.clone(),
            task_name: self.task_name.clone(),
            nickname: self.nickname.clone(),
            role: self.role.clone(),
            status: self.status.clone(),
        }
    }
}

/// Everything a new agent needs registered before its task starts.
pub struct NewAgent {
    pub task_name: String,
    pub role: String,
    pub nicknames: Vec<String>,
    pub commands: mpsc::UnboundedSender<AgentCommand>,
    pub interrupt: Arc<AtomicBool>,
}

/// Outcome of [`AgentHub::wait_for_update`].
#[derive(Debug, PartialEq, Eq)]
pub enum WaitOutcome {
    Updates(Vec<AgentUpdate>),
    /// Nothing is running and nothing is pending: waiting would hang.
    NothingToWaitFor,
    TimedOut,
    Cancelled,
}

/// Per-run registry of sub-agents plus the parent's mailbox.
#[derive(Default)]
pub struct AgentHub {
    agents: Mutex<Vec<SubAgent>>,
    mailbox: Mutex<VecDeque<AgentUpdate>>,
    notify: Notify,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl AgentHub {
    /// Registers an agent under a unique `task_name` (suffixed `_2`, `_3`, …
    /// on collision). Fails when all concurrency slots are taken.
    pub fn register(&self, new: NewAgent) -> Result<AgentInfo, String> {
        let mut agents = lock(&self.agents);
        let live = agents.iter().filter(|agent| agent.status != "closed").count();
        if live >= MAX_CONCURRENT_AGENTS {
            return Err(format!(
                "Agent limit reached ({MAX_CONCURRENT_AGENTS} live agents). Completed agents keep their slot until closed: use close_agent on agents you no longer need."
            ));
        }
        let taken = |name: &str| agents.iter().any(|agent| agent.status != "closed" && agent.task_name == name);
        let task_name = unique_name(&new.task_name, &taken);
        let nickname = new
            .nicknames
            .iter()
            .find(|candidate| !agents.iter().any(|agent| agent.status != "closed" && &agent.nickname == *candidate))
            .cloned()
            .unwrap_or_else(|| task_name.clone());
        let agent = SubAgent {
            id: format!("agent-{}", uuid::Uuid::new_v4().simple()),
            task_name,
            nickname,
            role: new.role,
            status: "running".to_string(),
            commands: Some(new.commands),
            interrupt: new.interrupt,
            abort: None,
        };
        let info = agent.info();
        agents.push(agent);
        Ok(info)
    }

    pub fn set_abort(&self, id: &str, abort: tokio::task::AbortHandle) {
        if let Some(agent) = lock(&self.agents).iter_mut().find(|agent| agent.id == id) {
            agent.abort = Some(abort);
        }
    }

    /// Finds a live agent by id, task name (optionally `/root/`-prefixed) or nickname.
    pub fn find(&self, target: &str) -> Option<AgentInfo> {
        let target = target.trim();
        let short = target.rsplit('/').next().unwrap_or(target);
        lock(&self.agents)
            .iter()
            .filter(|agent| agent.status != "closed")
            .find(|agent| {
                agent.id == target
                    || agent.task_name == short
                    || agent.nickname.eq_ignore_ascii_case(short)
            })
            .map(SubAgent::info)
    }

    pub fn list(&self, path_prefix: Option<&str>) -> Vec<AgentInfo> {
        let prefix = path_prefix.map(|p| p.trim().trim_start_matches("/root/").trim_end_matches('/'));
        lock(&self.agents)
            .iter()
            .filter(|agent| agent.status != "closed")
            .filter(|agent| prefix.is_none_or(|p| p.is_empty() || agent.task_name.starts_with(p)))
            .map(SubAgent::info)
            .collect()
    }

    pub fn running_count(&self) -> usize {
        lock(&self.agents).iter().filter(|agent| agent.status == "running").count()
    }

    pub fn has_unread(&self, agent_id: &str) -> bool {
        lock(&self.mailbox).iter().any(|update| update.agent_id == agent_id)
    }

    pub fn set_status(&self, id: &str, status: &str) {
        if let Some(agent) = lock(&self.agents).iter_mut().find(|agent| agent.id == id) {
            if agent.status != "closed" {
                agent.status = status.to_string();
            }
        }
    }

    /// Records the end of a turn: sets the status and notifies the parent.
    pub fn finish_turn(&self, id: &str, status: &str, output: String) {
        let task_name = {
            let mut agents = lock(&self.agents);
            let Some(agent) = agents.iter_mut().find(|agent| agent.id == id) else { return };
            if agent.status == "closed" {
                return;
            }
            agent.status = status.to_string();
            agent.task_name.clone()
        };
        lock(&self.mailbox).push_back(AgentUpdate {
            agent_id: id.to_string(),
            task_name,
            status: status.to_string(),
            output,
        });
        self.notify.notify_one();
    }

    pub fn send(&self, id: &str, command: AgentCommand) -> Result<(), String> {
        let agents = lock(&self.agents);
        let agent = agents.iter().find(|agent| agent.id == id).ok_or("unknown agent")?;
        agent
            .commands
            .as_ref()
            .ok_or("agent is closed")?
            .send(command)
            .map_err(|_| "agent is no longer accepting messages".to_string())
    }

    /// Raises the interrupt flag; returns the status seen before.
    pub fn interrupt(&self, id: &str) -> Option<String> {
        let agents = lock(&self.agents);
        let agent = agents.iter().find(|agent| agent.id == id)?;
        if agent.status == "running" {
            agent.interrupt.store(true, Ordering::SeqCst);
        }
        Some(agent.status.clone())
    }

    /// Closes one agent (abort, free its slot, drop its unread updates);
    /// returns the status seen before.
    pub fn close(&self, id: &str) -> Option<String> {
        let previous = {
            let mut agents = lock(&self.agents);
            let agent = agents.iter_mut().find(|agent| agent.id == id)?;
            let previous = agent.status.clone();
            agent.status = "closed".to_string();
            agent.commands = None;
            if let Some(abort) = agent.abort.take() {
                abort.abort();
            }
            previous
        };
        lock(&self.mailbox).retain(|update| update.agent_id != id);
        Some(previous)
    }

    /// Closes everything (end of the parent run).
    pub fn shutdown(&self) {
        let ids: Vec<String> = lock(&self.agents).iter().map(|agent| agent.id.clone()).collect();
        for id in ids {
            self.close(&id);
        }
    }

    fn take_updates(&self) -> Vec<AgentUpdate> {
        lock(&self.mailbox).drain(..).collect()
    }

    /// Free concurrency slots (live agents count, completed ones included).
    pub fn free_slots(&self) -> usize {
        let live = lock(&self.agents).iter().filter(|agent| agent.status != "closed").count();
        MAX_CONCURRENT_AGENTS.saturating_sub(live)
    }

    /// Takes the mailbox updates of `ids` only (others stay for `wait_agent`).
    fn take_updates_of(&self, ids: &[String]) -> Vec<AgentUpdate> {
        let mut mailbox = lock(&self.mailbox);
        let (mine, rest): (VecDeque<AgentUpdate>, VecDeque<AgentUpdate>) =
            mailbox.drain(..).partition(|update| ids.contains(&update.agent_id));
        *mailbox = rest;
        mine.into_iter().collect()
    }

    /// Waits until every agent in `ids` reported one update (the batch
    /// `spawn_agents` call); `None` when `cancelled` resolves first.
    pub async fn wait_for_agents(&self, ids: &[String], cancelled: impl Future<Output = ()>) -> Option<Vec<AgentUpdate>> {
        tokio::pin!(cancelled);
        let mut collected: Vec<AgentUpdate> = Vec::new();
        loop {
            for update in self.take_updates_of(ids) {
                if !collected.iter().any(|seen| seen.agent_id == update.agent_id) {
                    collected.push(update);
                }
            }
            if ids.iter().all(|id| collected.iter().any(|update| &update.agent_id == id)) {
                return Some(collected);
            }
            tokio::select! {
                _ = self.notify.notified() => {}
                _ = &mut cancelled => return None,
            }
        }
    }

    /// Waits until any agent has an unread update, `timeout` passes, or
    /// `cancelled` resolves.
    pub async fn wait_for_update(&self, timeout: Duration, cancelled: impl Future<Output = ()>) -> WaitOutcome {
        let deadline = tokio::time::Instant::now() + timeout;
        tokio::pin!(cancelled);
        loop {
            let updates = self.take_updates();
            if !updates.is_empty() {
                return WaitOutcome::Updates(updates);
            }
            if self.running_count() == 0 {
                return WaitOutcome::NothingToWaitFor;
            }
            tokio::select! {
                _ = self.notify.notified() => {}
                _ = tokio::time::sleep_until(deadline) => return WaitOutcome::TimedOut,
                _ = &mut cancelled => return WaitOutcome::Cancelled,
            }
        }
    }
}

// ----------------------------------------------------------------------
// Names
// ----------------------------------------------------------------------

/// Lower-case letters, digits and underscores only (Codex's task-name rule).
pub fn sanitize_task_name(raw: &str) -> String {
    let mut name = String::new();
    for ch in raw.trim().chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_alphanumeric() {
            name.push(ch);
        } else if !name.ends_with('_') {
            name.push('_');
        }
        if name.len() >= 40 {
            break;
        }
    }
    let name = name.trim_matches('_').to_string();
    if name.is_empty() { "agent".to_string() } else { name }
}

fn unique_name(base: &str, taken: &dyn Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}_{n}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| base.to_string())
}

fn derive_task_name(message: &str) -> String {
    sanitize_task_name(&message.split_whitespace().take(5).collect::<Vec<_>>().join(" "))
}

// ----------------------------------------------------------------------
// Tool definitions
// ----------------------------------------------------------------------

fn def(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({ "type": "function", "function": {
        "name": name,
        "description": description,
        "parameters": { "type": "object", "properties": properties, "required": required }
    }})
}

fn target_prop(what: &str) -> Value {
    json!({ "type": "string", "description": format!("Agent id or task name {what} (from spawn_agent).") })
}

/// The seven v2 tool specs; `roles` feeds the `agent_type` description.
pub fn tool_defs(roles: &[AgentRole]) -> Vec<Value> {
    vec![
        def("spawn_agent",
            "Spawn a named sub-agent for a well-scoped task. It runs in the background on the current model; collect its result with wait_agent and keep working with followup_task. Completed agents keep a slot (max 4 live agents) until you close_agent them.",
            json!({
                "task_name": { "type": "string", "description": "Task name for the new agent. Use lowercase letters, digits, and underscores." },
                "message": { "type": "string", "description": "Initial plain-text task for the new agent. Make it self-contained." },
                "agent_type": { "type": "string", "description": format!("Agent role. Defaults to explorer.\n{}", agent_roles::describe_roles(roles)) }
            }), &["task_name", "message"]),
        def("send_message",
            "Send a message to an existing agent. It is delivered promptly and does not trigger a new turn.",
            json!({ "target": target_prop("to message"), "message": { "type": "string", "description": "Message text to queue on the target agent." } }),
            &["target", "message"]),
        def("followup_task",
            "Send a follow-up task to an existing agent and trigger a turn if it is idle. If the target is already running, the task is delivered at its next message boundary.",
            json!({ "target": target_prop("to send a follow-up task to"), "message": { "type": "string", "description": "Task text for the target agent." } }),
            &["target", "message"]),
        def("wait_agent",
            "Wait for a mailbox update from any live agent (a turn finished or failed). Returns the pending updates with each agent's output, or a timeout summary if nothing arrives before the deadline.",
            json!({ "timeout_ms": { "type": "integer", "description": format!("How long to wait. Default {WAIT_DEFAULT_MS}, min {WAIT_MIN_MS}, max {WAIT_MAX_MS}.") } }),
            &[]),
        def("interrupt_agent",
            "Interrupt an agent's current turn, if any, and return its previous status. The agent remains available for messages and follow-up tasks.",
            json!({ "target": target_prop("to interrupt") }), &["target"]),
        def("close_agent",
            "Close an agent when it is no longer needed and return its previous status. Frees its concurrency slot.",
            json!({ "target": target_prop("to close") }), &["target"]),
        def("list_agents",
            "List live agents in this run, optionally filtered by task-name prefix.",
            json!({ "path_prefix": { "type": "string", "description": "Task-name prefix filter. Omit to list all live agents." } }),
            &[]),
    ]
}

/// Rewrites the `spawn_agent` spec in `tools` so its role list includes the
/// project's and user's custom roles.
pub fn apply_project_roles(tools: &mut [Value], project: &std::path::Path) {
    let Some(slot) = tools
        .iter_mut()
        .find(|tool| tool.pointer("/function/name").and_then(Value::as_str) == Some("spawn_agent"))
    else {
        return;
    };
    let roles = agent_roles::load_roles(project);
    if let Some(spec) = tool_defs(&roles).into_iter().next() {
        *slot = spec;
    }
}

// ----------------------------------------------------------------------
// Tool handlers
// ----------------------------------------------------------------------

fn failed(message: impl Into<String>) -> AgentError {
    AgentError::Failed(message.into())
}

fn text_arg<'a>(args: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| args.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn to_text(value: Value) -> String {
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into())
}

/// Clamps a requested wait to Codex's bounds.
pub fn clamp_wait_ms(requested: Option<i64>) -> Result<u64, String> {
    match requested {
        None => Ok(WAIT_DEFAULT_MS),
        Some(ms) if ms > WAIT_MAX_MS as i64 => Err(format!("timeout_ms must be at most {WAIT_MAX_MS}")),
        Some(ms) => Ok((ms.max(0) as u64).max(WAIT_MIN_MS)),
    }
}

impl AgentLoop {
    /// Routes one v2 agent tool call.
    pub(super) async fn dispatch_agent_tool(&self, name: &str, args: &Value) -> Result<String, AgentError> {
        match name {
            "spawn_agent" => self.spawn_agent_tool(args),
            "send_message" => self.message_agent_tool(args, false),
            "followup_task" => self.message_agent_tool(args, true),
            "wait_agent" => {
                let timeout = clamp_wait_ms(args.get("timeout_ms").and_then(Value::as_i64)).map_err(failed)?;
                self.wait_agent_tool(Duration::from_millis(timeout)).await
            }
            "interrupt_agent" => self.interrupt_agent_tool(args),
            "close_agent" => self.close_agent_tool(args),
            "list_agents" => Ok(self.list_agents_tool(args)),
            other => Err(failed(format!("Unknown agent tool \"{other}\"."))),
        }
    }

    fn resolve_target(&self, tool: &str, args: &Value) -> Result<AgentInfo, AgentError> {
        let target = text_arg(args, &["target", "agent_id"]).ok_or_else(|| failed(format!("{tool}: target is required.")))?;
        self.handle
            .agents
            .find(target)
            .ok_or_else(|| failed(format!("{tool}: no live agent \"{target}\". Use list_agents.")))
    }

    fn spawn_agent_tool(&self, args: &Value) -> Result<String, AgentError> {
        let message = text_arg(args, &["message", "task"]).ok_or_else(|| failed("spawn_agent: message is empty."))?;
        let roles = agent_roles::load_roles(&self.project_path);
        let role = agent_roles::resolve_role(&roles, text_arg(args, &["agent_type", "role"]), &self.mode).map_err(failed)?;
        let requested_name = text_arg(args, &["task_name", "name"]).map(sanitize_task_name);
        let info = self
            .start_agent(&role, requested_name.unwrap_or_else(|| derive_task_name(message)), message)
            .map_err(failed)?;
        Ok(to_text(json!({
            "agent_id": info.id,
            "task_name": info.task_name,
            "nickname": info.nickname,
            "role": info.role,
            "note": "Running in the background. Use wait_agent to collect its result.",
        })))
    }

    /// Registers an agent in the hub (counting against
    /// [`MAX_CONCURRENT_AGENTS`]) and starts its background task. Shared by
    /// `spawn_agent` and the `spawn_agents` batch.
    fn start_agent(&self, role: &AgentRole, task_name: String, message: &str) -> Result<AgentInfo, String> {
        let (commands_tx, commands_rx) = mpsc::unbounded_channel();
        let interrupt = Arc::new(AtomicBool::new(false));
        let info = self.handle.agents.register(NewAgent {
            task_name,
            role: role.name.clone(),
            nicknames: role.nicknames.clone(),
            commands: commands_tx,
            interrupt: interrupt.clone(),
        })?;
        let spec = self.agent_spec(role, &info.nickname, message);
        let task = supervised_agent_task(spec, info.id.clone(), commands_rx, interrupt, self.turn_diff.clone());
        let abort = tokio::spawn(task).abort_handle();
        self.handle.agents.set_abort(&info.id, abort);
        self.handle.emit(json!({
            "type": "agent_spawned",
            "agentId": info.id,
            "name": info.nickname,
            "taskName": info.task_name,
            "role": info.role,
        }));
        Ok(info)
    }

    /// `spawn_agents`: the one-shot parallel batch, implemented on the same
    /// hub as `spawn_agent` (QA-033): every agent takes a slot, the call
    /// waits for each agent's first turn, then closes them all.
    pub(super) async fn spawn_agents_tool(&self, args: &Value) -> Result<String, AgentError> {
        let requested = args.get("agents").and_then(Value::as_array).cloned().unwrap_or_default();
        let jobs: Vec<(Option<String>, String)> = requested
            .iter()
            .filter_map(|agent| {
                let task = text_arg(agent, &["task", "message"])?.to_string();
                Some((text_arg(agent, &["role", "agent_type"]).map(str::to_string), task))
            })
            .collect();
        if jobs.is_empty() {
            return Err(failed("spawn_agents: agents must be a non-empty array of {role, task}."));
        }
        let free = self.handle.agents.free_slots();
        if jobs.len() > free {
            return Err(failed(format!(
                "spawn_agents: {} agents requested but only {free} of {MAX_CONCURRENT_AGENTS} agent slots are free. Close agents you no longer need (close_agent) or request fewer.",
                jobs.len()
            )));
        }
        let roles = agent_roles::load_roles(&self.project_path);
        let mut started: Vec<(AgentInfo, String)> = Vec::new();
        let mut results: Vec<String> = Vec::new();
        for (role, task) in jobs {
            let outcome = agent_roles::resolve_role(&roles, role.as_deref(), &self.mode)
                .and_then(|role| self.start_agent(&role, derive_task_name(&task), &task));
            match outcome {
                Ok(info) => started.push((info, task)),
                Err(err) => results.push(format!("## {}: {task}\n(agent failed: {err})", role.unwrap_or_default())),
            }
        }
        let ids: Vec<String> = started.iter().map(|(info, _)| info.id.clone()).collect();
        let updates = self.handle.agents.wait_for_agents(&ids, self.handle.cancelled()).await;
        for id in &ids {
            self.handle.agents.close(id);
        }
        let Some(updates) = updates else {
            return Err(AgentError::Cancelled);
        };
        for (info, task) in started {
            let Some(update) = updates.iter().find(|update| update.agent_id == info.id) else { continue };
            let body = match update.status.as_str() {
                "completed" => update.output.clone(),
                _ => format!("(agent {}: {})", update.status, update.output),
            };
            results.push(format!("## {}: {task}\n{body}", info.role));
        }
        Ok(format!("Sub-agent results:\n\n{}", results.join("\n\n")))
    }

    fn agent_spec(&self, role: &AgentRole, title: &str, task: &str) -> SubAgentSpec {
        SubAgentSpec {
            state: self.state.clone(),
            handle: self.handle.clone(),
            run_id: self.run_id.clone(),
            project_path: self.project_path.clone(),
            mode: self.mode.clone(),
            provider_id: self.provider_id.clone(),
            model: self.model.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            role: role.name.clone(),
            instructions: role.instructions.clone(),
            title: title.to_string(),
            task: task.to_string(),
            readonly: role.readonly,
            depth_left: 0,
        }
    }

    fn message_agent_tool(&self, args: &Value, starts_turn: bool) -> Result<String, AgentError> {
        let tool = if starts_turn { "followup_task" } else { "send_message" };
        let message = text_arg(args, &["message", "input"]).ok_or_else(|| failed(format!("{tool}: message is empty.")))?;
        let info = self.resolve_target(tool, args)?;
        let command = if starts_turn {
            AgentCommand::Task(message.to_string())
        } else {
            AgentCommand::Message(message.to_string())
        };
        self.handle.agents.send(&info.id, command).map_err(|err| failed(format!("{tool}: {err}")))?;
        let note = if starts_turn && info.status != "running" {
            self.handle.agents.set_status(&info.id, "running");
            "The agent was idle; a new turn has started."
        } else if starts_turn {
            "The agent is running; the task is delivered at its next round boundary."
        } else {
            "Queued. The agent sees it at its next round boundary; no new turn was started."
        };
        Ok(to_text(json!({ "agent_id": info.id, "task_name": info.task_name, "note": note })))
    }

    async fn wait_agent_tool(&self, timeout: Duration) -> Result<String, AgentError> {
        let outcome = self.handle.agents.wait_for_update(timeout, self.handle.cancelled()).await;
        match outcome {
            WaitOutcome::Cancelled => Err(AgentError::Cancelled),
            WaitOutcome::Updates(updates) => Ok(to_text(json!({
                "message": "Wait completed.",
                "timed_out": false,
                "updates": updates.iter().map(AgentUpdate::to_json).collect::<Vec<_>>(),
            }))),
            WaitOutcome::NothingToWaitFor => Ok(to_text(json!({
                "message": "No agent is running and no updates are pending.",
                "timed_out": false,
                "updates": [],
            }))),
            WaitOutcome::TimedOut => Ok(to_text(json!({
                "message": "Wait timed out.",
                "timed_out": true,
                "updates": [],
            }))),
        }
    }

    fn interrupt_agent_tool(&self, args: &Value) -> Result<String, AgentError> {
        let info = self.resolve_target("interrupt_agent", args)?;
        let previous = self.handle.agents.interrupt(&info.id).unwrap_or(info.status);
        Ok(to_text(json!({ "previous_status": previous })))
    }

    fn close_agent_tool(&self, args: &Value) -> Result<String, AgentError> {
        let info = self.resolve_target("close_agent", args)?;
        let previous = self.handle.agents.close(&info.id).unwrap_or(info.status);
        Ok(to_text(json!({ "previous_status": previous })))
    }

    fn list_agents_tool(&self, args: &Value) -> String {
        let hub = &self.handle.agents;
        let agents: Vec<Value> = hub
            .list(text_arg(args, &["path_prefix"]))
            .into_iter()
            .map(|agent| {
                json!({
                    "agent_id": agent.id,
                    "task_name": agent.task_name,
                    "nickname": agent.nickname,
                    "role": agent.role,
                    "status": agent.status,
                    "unread_update": hub.has_unread(&agent.id),
                })
            })
            .collect();
        to_text(json!({ "agents": agents, "max_live_agents": MAX_CONCURRENT_AGENTS }))
    }

    /// While idle, waits for the next `followup_task`. `send_message`
    /// content is appended to the history without starting a turn. Returns
    /// `None` when the agent should shut down (closed or run cancelled).
    async fn wait_for_task(&mut self) -> Option<String> {
        loop {
            let inbox = self.agent_inbox.as_mut()?;
            let command = tokio::select! {
                command = inbox.recv() => command?,
                _ = self.handle.cancelled() => return None,
            };
            match command {
                AgentCommand::Task(text) => return Some(text),
                AgentCommand::Message(text) => self.add_user_turn_input(text),
            }
        }
    }

    /// Persists + emits a user-role message in this agent's thread and
    /// appends it to the model history.
    fn add_user_turn_input(&mut self, text: String) {
        let message = self.new_message("user", &text);
        self.persist_and_emit(&message);
        self.history.push(super::llm::LlmMessage { role: "user".into(), content: text, ..Default::default() });
    }
}

// ----------------------------------------------------------------------
// The agent task
// ----------------------------------------------------------------------

/// [`run_agent_task`] with a panic guard: a crashing agent reports a
/// `failed` update instead of leaving waiters hanging on a `running` agent.
async fn supervised_agent_task(
    spec: SubAgentSpec,
    id: String,
    commands: mpsc::UnboundedReceiver<AgentCommand>,
    interrupt: Arc<AtomicBool>,
    turn_diff: Arc<super::TurnDiffTracker>,
) {
    use futures::FutureExt;
    let handle = spec.handle.clone();
    let task = std::panic::AssertUnwindSafe(run_agent_task(spec, id.clone(), commands, interrupt, turn_diff));
    if let Err(payload) = task.catch_unwind().await {
        let reason = super::panic_message(payload);
        tracing::error!("Sub-agent {id} panicked: {reason}");
        handle.agents.finish_turn(&id, "failed", format!("The agent crashed: {reason}"));
    }
}

/// Body of the tokio task behind one agent: turn → report → idle → repeat.
async fn run_agent_task(
    spec: SubAgentSpec,
    id: String,
    commands: mpsc::UnboundedReceiver<AgentCommand>,
    interrupt: Arc<AtomicBool>,
    turn_diff: Arc<super::TurnDiffTracker>,
) {
    let handle: Arc<RunHandle> = spec.handle.clone();
    let mut agent = super::build_sub_loop(spec, Some(commands), turn_diff, Some(interrupt.clone())).await;
    loop {
        interrupt.store(false, Ordering::SeqCst);
        handle.agents.set_status(&id, "running");
        // Race the turn against run cancellation and `interrupt_agent` so
        // either takes effect even while a slow model call is in flight.
        agent.subagent_start_hook().await;
        let turn = tokio::select! {
            result = Box::pin(agent.run(SUB_AGENT_MAX_ROUNDS)) => Some(result),
            _ = handle.cancelled() => None,
            _ = interrupt_raised(&interrupt) => Some(Err(AgentError::Cancelled)),
        };
        if let Some(result) = &turn {
            agent.subagent_stop_hook(result).await;
        }
        match turn {
            None | Some(Err(AgentError::Cancelled)) if handle.is_cancelled() => {
                handle.agents.set_status(&id, "cancelled");
                return;
            }
            None => return,
            Some(Ok(text)) => {
                agent.record_final_answer(&text);
                handle.agents.finish_turn(&id, "completed", text);
            }
            Some(Err(AgentError::Cancelled)) => {
                repair_dangling_tool_calls(&mut agent.history);
                handle.agents.finish_turn(&id, "interrupted", "The agent's turn was interrupted.".to_string());
            }
            Some(Err(AgentError::Failed(error))) => handle.agents.finish_turn(&id, "failed", error),
        }
        match agent.wait_for_task().await {
            Some(task) => agent.add_user_turn_input(task),
            None => return,
        }
    }
}

/// Resolves once `interrupt_agent` raised the flag.
pub(super) async fn interrupt_raised(flag: &AtomicBool) {
    while !flag.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

impl AgentLoop {
    /// `run` returns the final answer without adding it to the history;
    /// a follow-up turn needs it there.
    fn record_final_answer(&mut self, text: &str) {
        let last_is_assistant = self.history.last().is_some_and(|message| message.role == "assistant");
        if !text.is_empty() && !last_is_assistant {
            self.history.push(super::llm::LlmMessage { role: "assistant".into(), content: text.to_string(), ..Default::default() });
        }
    }
}

/// After an interrupt the history may end in an assistant message whose tool
/// calls have no results; providers reject that, so answer each one.
pub fn repair_dangling_tool_calls(history: &mut Vec<super::llm::LlmMessage>) {
    let Some(assistant_at) = history.iter().rposition(|message| message.role == "assistant") else { return };
    let calls = history[assistant_at].tool_calls.clone().unwrap_or_default();
    for call in calls {
        let call_id = call.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let answered = history[assistant_at + 1..]
            .iter()
            .any(|message| message.role == "tool" && message.tool_call_id.as_deref() == Some(call_id.as_str()));
        if !answered {
            history.push(super::llm::LlmMessage {
                role: "tool".into(),
                content: "Tool call interrupted before it ran.".into(),
                tool_call_id: Some(call_id),
                ..Default::default()
            });
        }
    }
}

#[cfg(test)]
pub(super) mod tests;
