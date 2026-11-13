//! Runs facade: run lifecycle, messages, usage, plan, permission/question
//! coordination and live event subscription. Used by the desktop UI and the
//! `lcgn` CLI.

use crate::api::{ApiError, ApiResult, EventRx, RunEvent};
use crate::db::{Plan, Run, RunMessage, UsageLogEntry};
use crate::orchestrator::RunHandle;
use crate::util::now_iso;
use crate::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

// ----------------------------------------------------------------------
// Payloads
// ----------------------------------------------------------------------

/// `POST /api/runs` body.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct CreateRunPayload {
    pub task: String,
    pub project_path: Option<String>,
    pub project_name: Option<String>,
    pub provider_id: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub mode: Option<String>,
    #[serde(default)]
    pub bypass_permissions: bool,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub append_system_prompt: Option<String>,
}

/// `POST /api/runs/:id/continue` body — same fields, all provider fields
/// optional so the frontend can resend the full composer state.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ContinueRunPayload {
    pub task: String,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub mode: Option<String>,
    #[serde(default)]
    pub bypass_permissions: bool,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub append_system_prompt: Option<String>,
}

/// `GET /api/usage-logs` query parameters.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageLogQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub search: Option<String>,
    pub provider_id: Option<String>,
    pub agent_role: Option<String>,
}

// ----------------------------------------------------------------------
// Typed outputs
// ----------------------------------------------------------------------

/// Per-agent-role usage slice inside a `RunUsageSummary`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct RoleUsage {
    pub agent_role: String,
    pub cost: f64,
    pub calls: usize,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// Aggregate usage for one run (`GET /api/runs/:id/usage`).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunUsageSummary {
    pub run_id: String,
    pub total_cost: f64,
    pub total_calls: usize,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_read_tokens: i64,
    pub avg_cache_hit_rate: f64,
    pub by_role: Vec<RoleUsage>,
}

/// Pending permission/question requests (`GET /api/runs/:id/pending`); both
/// keys are always present, `null` when nothing is pending.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct PendingRequests {
    pub permission_request: Option<Value>,
    pub question_request: Option<Value>,
}

/// Totals inside a `UsageLogsResponse`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageMetrics {
    pub total_cost: f64,
    pub total_calls: usize,
    pub total_tokens: i64,
    pub avg_cache_hit_rate: f64,
}

/// Paged usage logs (`GET /api/usage-logs`).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageLogsResponse {
    pub logs: Vec<UsageLogEntry>,
    pub total: usize,
    pub unique_providers: Vec<String>,
    pub unique_roles: Vec<String>,
    pub metrics: UsageMetrics,
}

/// A freshly started run. `events` is a receiver subscribed BEFORE the drive
/// task starts, so no event is lost — UIs should use it for the first turn
/// instead of calling `subscribe_run_events` afterwards.
pub struct StartedRun {
    pub run: Run,
    pub handle: Arc<RunHandle>,
    pub events: EventRx,
}

/// A continued run; same `events` contract as `StartedRun`.
pub struct ContinuedRun {
    pub handle: Arc<RunHandle>,
    pub events: EventRx,
}

// ----------------------------------------------------------------------
// Private helpers
// ----------------------------------------------------------------------

fn is_terminal(status: &str) -> bool {
    matches!(status, "done" | "failed" | "cancelled")
}

fn active_run_statuses() -> [&'static str; 4] {
    ["created", "generating", "awaiting_permission", "awaiting_input"]
}

fn provider_display_name(state: &AppState, provider_id: &str) -> String {
    state
        .pi
        .get_configs()
        .get(provider_id)
        .and_then(|config| config.display_name.clone())
        .unwrap_or_else(|| provider_id.to_string())
}

fn truncate_title(task: &str) -> String {
    let first_line = task.lines().next().unwrap_or("").trim();
    let mut title: String = first_line.chars().take(60).collect();
    if first_line.chars().count() > 60 {
        title.push('…');
    }
    if title.is_empty() {
        title = "New chat".to_string();
    }
    title
}

/// The run's effective mode: explicit bypass wins over a milder mode.
fn effective_mode(mode: Option<&str>, bypass: bool) -> String {
    // Unset mode falls back to config.toml (`approval_policy` / `sandbox_mode`),
    // then to Build (`accept_edits`).
    let configured = crate::config_runtime::current().default_run_mode();
    let mode = mode
        .filter(|mode| !mode.trim().is_empty())
        .or(configured.as_deref())
        .unwrap_or("accept_edits");
    if bypass && !matches!(mode, "chat" | "plan") {
        "auto".to_string()
    } else {
        mode.to_string()
    }
}

/// Clones the shared state for the spawned drive task.
fn state_for_spawn(state: &AppState) -> Arc<AppState> {
    Arc::new(AppState {
        db: state.db.clone(),
        pi: state.pi.clone(),
        orchestrator: state.orchestrator.clone(),
        settings_path: state.settings_path.clone(),
        default_project_path: state.default_project_path.clone(),
        processes: state.processes.clone(),
    })
}

/// The replay event sent to a late subscriber of an already-terminal run.
fn terminal_event_value(status: &str, error_message: Option<&str>) -> Value {
    match status {
        "done" => json!({ "type": "run_completed", "finalOutput": "" }),
        "failed" => json!({
            "type": "run_failed",
            "errorMessage": error_message.unwrap_or("Run failed.")
        }),
        _ => json!({ "type": "status_changed", "status": status }),
    }
}

/// Maps a live run event the way the SSE endpoint always did: terminal
/// `status_changed` events are reduced to `{ type, status }`, everything
/// else passes through unchanged.
fn map_event(event: RunEvent) -> RunEvent {
    match event.get("type").and_then(Value::as_str) {
        Some("run_completed") | Some("run_failed") => event,
        Some("status_changed") => {
            let status = event.get("status").and_then(Value::as_str).unwrap_or_default();
            if is_terminal(status) {
                json!({ "type": "status_changed", "status": status })
            } else {
                event
            }
        }
        _ => event,
    }
}

/// Replays pending permission/question requests into a fresh subscriber.
async fn replay_pending(tx: &tokio::sync::broadcast::Sender<RunEvent>, handle: &Arc<RunHandle>) {
    if let Some(permission) = handle.pending_permission().await {
        let _ = tx.send(permission);
    }
    if let Some(question) = handle.pending_question().await {
        let _ = tx.send(question);
    }
}

/// Forwards mapped events from a run handle into a subscriber channel until
/// the source closes or the last receiver goes away.
async fn forward_from_handle(
    tx: tokio::sync::broadcast::Sender<RunEvent>,
    mut source: tokio::sync::broadcast::Receiver<RunEvent>,
    handle: Arc<RunHandle>,
) {
    // The handle keeps the task alive (its event sender lives as long as the
    // handle); drop the explicit binding once the loop is done with it.
    let _keep_alive = handle;
    loop {
        match source.recv().await {
            Ok(event) => {
                if tx.send(map_event(event)).is_err() {
                    break;
                }
            }
            Err(RecvError::Lagged(_)) => {
                if tx.send(json!({ "type": "lagged" })).is_err() {
                    break;
                }
            }
            Err(RecvError::Closed) => break,
        }
    }
}

// ----------------------------------------------------------------------
// POST /api/runs
// ----------------------------------------------------------------------

/// Creates a run and spawns its drive task (`POST /api/runs`). The event
/// receiver is subscribed before the drive task starts so no event is lost.
pub async fn create_run(
    state: &AppState,
    payload: &CreateRunPayload,
) -> ApiResult<StartedRun> {
    if payload.task.trim().is_empty() {
        return Err(ApiError::bad_request("Task is required."));
    }
    if payload.provider_id.trim().is_empty() || payload.model.trim().is_empty() {
        return Err(ApiError::bad_request("Provider and model are required."));
    }

    let project_path = payload
        .project_path
        .clone()
        .filter(|path| !path.trim().is_empty())
        .unwrap_or_else(|| state.default_project_path.display().to_string());
    let project_name = payload
        .project_name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| {
            state
                .db
                .get_project(&project_path)
                .map(|project| project.name)
                .unwrap_or_default()
        });
    let mode = effective_mode(payload.mode.as_deref(), payload.bypass_permissions);
    let run_id = format!("run-{}", uuid::Uuid::new_v4());

    let run = Run {
        id: run_id.clone(),
        title: truncate_title(&payload.task),
        task: payload.task.clone(),
        project_path: project_path.clone(),
        project_name,
        status: "created".to_string(),
        provider_id: payload.provider_id.clone(),
        provider_display_name: provider_display_name(state, &payload.provider_id),
        model: payload.model.clone(),
        reasoning_effort: payload
            .reasoning_effort
            .clone()
            .filter(|e| !e.is_empty())
            .or_else(|| crate::config_runtime::current().reasoning_effort().filter(|e| e != "off")),
        mode: mode.clone(),
        system_prompt: payload.system_prompt.clone().filter(|p| !p.is_empty()),
        append_system_prompt: payload.append_system_prompt.clone().filter(|p| !p.is_empty()),
        created_at: now_iso(),
        updated_at: now_iso(),
        last_active_at: Some(now_iso()),
        ..Default::default()
    };
    state.db.create_run(run.clone());

    // Initial user message drives the first round.
    let message = RunMessage {
        id: format!("msg-{}", uuid::Uuid::new_v4()),
        run_id: run_id.clone(),
        role: "user".to_string(),
        content: payload.task.clone(),
        created_at: now_iso(),
        ..Default::default()
    };
    state.db.append_message(message.clone());

    let handle = state.orchestrator.ensure_handle(&run_id);
    // Subscribe before spawning and replay the initial user message so the
    // caller's receiver sees it immediately (the drive task emits nothing
    // for it).
    let events = handle.events.subscribe();
    handle.emit(json!({
        "type": "message_created",
        "message": serde_json::to_value(&message).unwrap_or(Value::Null),
    }));
    state.orchestrator.spawn(state_for_spawn(state), &run_id);

    Ok(StartedRun { run, handle, events })
}

// ----------------------------------------------------------------------
// POST /api/runs/:id/continue
// ----------------------------------------------------------------------

/// Appends a user message to a finished run and drives the next round
/// (`POST /api/runs/:id/continue`).
pub async fn continue_run(
    state: &AppState,
    run_id: &str,
    payload: &ContinueRunPayload,
) -> ApiResult<ContinuedRun> {
    let Some(existing) = state.db.get_run_full(run_id) else {
        return Err(ApiError::not_found("Run not found."));
    };
    if active_run_statuses().contains(&existing.status.as_str()) && existing.status != "generating" {
        return Err(ApiError::conflict(
            "Run is awaiting your input or approval; resolve it before sending a new message.",
        ));
    }
    if payload.task.trim().is_empty() {
        return Err(ApiError::bad_request("Task is required."));
    }

    let mode = effective_mode(payload.mode.as_deref(), payload.bypass_permissions);
    let message = RunMessage {
        id: format!("msg-{}", uuid::Uuid::new_v4()),
        run_id: run_id.to_string(),
        role: "user".to_string(),
        content: payload.task.clone(),
        created_at: now_iso(),
        ..Default::default()
    };

    state.db.patch_run(run_id, |run| {
        run.status = "generating".to_string();
        run.error_message = None;
        run.mode = mode.clone();
        if let Some(provider_id) = payload.provider_id.as_deref().filter(|p| !p.is_empty()) {
            run.provider_id = provider_id.to_string();
            run.provider_display_name = provider_display_name(state, provider_id);
        }
        if let Some(model) = payload.model.as_deref().filter(|m| !m.is_empty()) {
            run.model = model.to_string();
        }
        run.reasoning_effort = payload.reasoning_effort.clone().filter(|e| !e.is_empty());
        if payload.system_prompt.is_some() {
            run.system_prompt = payload.system_prompt.clone().filter(|p| !p.trim().is_empty());
        }
        if payload.append_system_prompt.is_some() {
            run.append_system_prompt = payload
                .append_system_prompt
                .clone()
                .filter(|p| !p.trim().is_empty());
        }
    });
    state.db.append_message(message.clone());

    let handle = state
        .orchestrator
        .handle(run_id)
        .unwrap_or_else(|| state.orchestrator.ensure_handle(run_id));
    // Subscribe before emitting so the caller's receiver gets the
    // message_created and status events (ordering would otherwise race).
    let events = handle.events.subscribe();
    handle.emit(json!({
        "type": "message_created",
        "message": serde_json::to_value(&message).unwrap_or(Value::Null),
    }));
    state.orchestrator.emit_status(run_id, "generating");
    state.orchestrator.spawn(state_for_spawn(state), run_id);

    Ok(ContinuedRun { handle, events })
}

// ----------------------------------------------------------------------
// POST /api/runs/:id/fork
// ----------------------------------------------------------------------

/// Forks a run (optionally cut at `up_to_message_id`) into a new independent
/// run with cloned messages + plan. Used by the sidebar fork button and the
/// `lcgn` `/fork` command.
pub async fn fork_run(
    state: &AppState,
    run_id: &str,
    up_to_message_id: Option<&str>,
) -> ApiResult<Run> {
    if state.db.get_run_full(run_id).is_none() {
        return Err(ApiError::not_found("Run not found."));
    }
    let forked = state
        .db
        .fork_run(run_id, up_to_message_id)
        .ok_or_else(|| {
            ApiError::bad_request("Fork point message not found in the source run.")
        })?;
    Ok(forked)
}

// ----------------------------------------------------------------------
// POST /api/runs/:id/cancel
// ----------------------------------------------------------------------

/// Cancels a run and resolves any pending permission/question so the loop
/// can unwind (`POST /api/runs/:id/cancel`). Never errors.
pub async fn cancel_run(state: &AppState, run_id: &str) -> ApiResult<()> {
    if let Some(handle) = state.orchestrator.handle(run_id) {
        handle.cancel();
        // Resolve any pending permission/question so the loop can unwind.
        {
            let mut slot = handle.permission.lock().await;
            if let Some(sender) = slot.sender.take() {
                let _ = sender.send("deny".to_string());
            }
            slot.pending = None;
        }
        {
            let mut slot = handle.question.lock().await;
            if let Some(sender) = slot.sender.take() {
                let _ = sender.send(json!({ "selections": [], "notes": ["Run cancelled by the user."] }));
            }
            slot.pending = None;
        }
    }

    if let Some(run) = state.db.get_run_full(run_id) {
        if active_run_statuses().contains(&run.status.as_str()) {
            state.db.patch_run(run_id, |run| run.status = "cancelled".to_string());
            state.orchestrator.emit_status(run_id, "cancelled");
        }
    }
    Ok(())
}

// ----------------------------------------------------------------------
// POST /api/runs/:id/messages
// ----------------------------------------------------------------------

/// Queues a mid-run message on an active run's inbox
/// (`POST /api/runs/:id/messages`).
pub async fn queue_message(state: &AppState, run_id: &str, content: &str) -> ApiResult<()> {
    let Some(handle) = state.orchestrator.handle(run_id) else {
        return Err(ApiError::conflict("Run is not active."));
    };
    let content = content.trim().to_string();
    if content.is_empty() {
        return Err(ApiError::bad_request("Message content is required."));
    }
    handle.inbox.lock().await.push_back(content);
    Ok(())
}

// ----------------------------------------------------------------------
// POST /api/runs/:id/permission
// ----------------------------------------------------------------------

/// Applies a permission decision to a run's pending request
/// (`POST /api/runs/:id/permission`). Returns `false` when nothing was
/// pending. Also used directly by the `lcgn` interactive prompt.
pub async fn apply_permission_decision(
    state: &AppState,
    handle: &Arc<RunHandle>,
    decision: &str,
) -> bool {
    let mut slot = handle.permission.lock().await;
    let (pending, sender) = (slot.pending.take(), slot.sender.take());
    drop(slot);

    if decision == "allow_run" {
        handle.allow_run.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    if let Some(pending) = pending {
        let tool_call = pending.tool_call;
        if tool_call.is_object() {
            let name = tool_call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let args: Value = serde_json::from_str(
                tool_call
                    .pointer("/function/arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}"),
            )
            .unwrap_or_else(|_| json!({}));
            if matches!(decision, "allow_project" | "allow_always") {
                if let Some(run) = state.db.get_run_full(&handle.run_id) {
                    let command = args.get("command").and_then(Value::as_str).unwrap_or_default().to_string();
                    let domains = args
                        .get("network_domains")
                        .or_else(|| args.get("networkDomains"))
                        .and_then(Value::as_array)
                        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
                        .unwrap_or_default();
                    state.db.upsert_permission(
                        if decision == "allow_project" { "project" } else { "global" },
                        &run.project_path,
                        &name,
                        &command,
                        domains,
                        decision,
                    );
                }
            }
        }
    }
    match sender {
        Some(sender) => {
            let _ = sender.send(decision.to_string());
            true
        }
        None => false,
    }
}

/// Records a permission decision for a run by id
/// (`POST /api/runs/:id/permission`).
pub async fn permission_decision(state: &AppState, run_id: &str, decision: &str) -> ApiResult<()> {
    let decision = decision.trim();
    if !["allow_once", "allow_project", "allow_always", "allow_run", "deny"]
        .contains(&decision)
    {
        return Err(ApiError::bad_request("Invalid permission decision."));
    }
    let Some(handle) = state.orchestrator.handle(run_id) else {
        return Err(ApiError::conflict("No pending permission request for this run."));
    };
    if apply_permission_decision(state, &handle, decision).await {
        Ok(())
    } else {
        Err(ApiError::conflict("No pending permission request for this run."))
    }
}

// ----------------------------------------------------------------------
// POST /api/runs/:id/answer
// ----------------------------------------------------------------------

/// Sends an answer to a run's pending question (`POST /api/runs/:id/answer`).
/// Returns `false` when nothing was pending. Also used by the `lcgn` prompt.
pub async fn answer_question_core(
    handle: &Arc<RunHandle>,
    selections: Vec<Vec<String>>,
    notes: Vec<String>,
) -> bool {
    let mut slot = handle.question.lock().await;
    let sender = slot.sender.take();
    slot.pending = None;
    drop(slot);
    match sender {
        Some(sender) => {
            let _ = sender.send(json!({
                "selections": selections,
                "notes": notes,
            }));
            true
        }
        None => false,
    }
}

/// Records an answer for a run's pending question by id
/// (`POST /api/runs/:id/answer`).
pub async fn answer_question(
    state: &AppState,
    run_id: &str,
    selections: Vec<Vec<String>>,
    notes: Vec<String>,
) -> ApiResult<()> {
    let Some(handle) = state.orchestrator.handle(run_id) else {
        return Err(ApiError::conflict("No pending question for this run."));
    };
    if answer_question_core(&handle, selections, notes).await {
        Ok(())
    } else {
        Err(ApiError::conflict("No pending question for this run."))
    }
}

// ----------------------------------------------------------------------
// GET /api/runs, /api/runs/:id, messages, usage, plan, pending
// ----------------------------------------------------------------------

/// Lists all runs, most recently active first (`GET /api/runs`).
pub async fn list_runs(state: &AppState) -> ApiResult<Vec<Run>> {
    let mut runs = state.db.list_runs();
    runs.sort_by(|a, b| b.last_active_at.clone().unwrap_or_default().cmp(&a.last_active_at.clone().unwrap_or_default()));
    Ok(runs)
}

/// Fetches one run (`GET /api/runs/:id`).
pub async fn get_run(state: &AppState, run_id: &str) -> ApiResult<Run> {
    state
        .db
        .get_run_full(run_id)
        .ok_or_else(|| ApiError::not_found("Run not found."))
}

/// Lists a run's messages (`GET /api/runs/:id/messages`).
pub async fn list_messages(state: &AppState, run_id: &str) -> ApiResult<Vec<RunMessage>> {
    if state.db.get_run_full(run_id).is_none() {
        return Err(ApiError::not_found("Run not found."));
    }
    Ok(state.db.list_messages(run_id))
}

/// Aggregated usage for one run (`GET /api/runs/:id/usage`).
pub async fn run_usage(state: &AppState, run_id: &str) -> ApiResult<RunUsageSummary> {
    if state.db.get_run_full(run_id).is_none() {
        return Err(ApiError::not_found("Run not found."));
    }
    Ok(usage_summary(run_id, &state.db.list_usage(Some(run_id))))
}

/// `RunUsageSummary` aggregation over a run's usage logs.
pub fn usage_summary(run_id: &str, entries: &[UsageLogEntry]) -> RunUsageSummary {
    let total_calls = entries.len();
    let total_cost: f64 = entries.iter().map(|entry| entry.cost).sum();
    let total_input: i64 = entries.iter().map(|entry| entry.input_tokens).sum();
    let total_output: i64 = entries.iter().map(|entry| entry.output_tokens).sum();
    let total_cache_read: i64 = entries.iter().map(|entry| entry.cache_read_tokens).sum();
    let total_input_weighted: i64 = entries.iter().map(|entry| entry.input_tokens).sum();
    let avg_cache_hit_rate = if total_input_weighted > 0 {
        total_cache_read as f64 * 100.0 / total_input_weighted as f64
    } else {
        0.0
    };

    let mut by_role: Vec<RoleUsage> = Vec::new();
    for entry in entries {
        let role = entry.agent_role.clone().unwrap_or_else(|| "planner".to_string());
        if let Some(slot) = by_role.iter_mut().find(|existing| existing.agent_role == role) {
            slot.cost += entry.cost;
            slot.calls += 1;
            slot.input_tokens += entry.input_tokens;
            slot.output_tokens += entry.output_tokens;
        } else {
            by_role.push(RoleUsage {
                agent_role: role,
                cost: entry.cost,
                calls: 1,
                input_tokens: entry.input_tokens,
                output_tokens: entry.output_tokens,
            });
        }
    }

    RunUsageSummary {
        run_id: run_id.to_string(),
        total_cost,
        total_calls,
        total_input_tokens: total_input,
        total_output_tokens: total_output,
        total_cache_read_tokens: total_cache_read,
        avg_cache_hit_rate,
        by_role,
    }
}

/// A run's stored plan, `None` when there is none (`GET /api/runs/:id/plan`;
/// the HTTP adapter serializes `None` as JSON `null`).
pub async fn run_plan(state: &AppState, run_id: &str) -> ApiResult<Option<Plan>> {
    Ok(state.db.get_plan(run_id))
}

/// A run's pending permission/question requests
/// (`GET /api/runs/:id/pending`).
pub async fn run_pending(state: &AppState, run_id: &str) -> ApiResult<PendingRequests> {
    let Some(handle) = state.orchestrator.handle(run_id) else {
        return Ok(PendingRequests {
            permission_request: None,
            question_request: None,
        });
    };
    Ok(PendingRequests {
        permission_request: handle.pending_permission().await,
        question_request: handle.pending_question().await,
    })
}

// ----------------------------------------------------------------------
// GET /api/usage-logs
// ----------------------------------------------------------------------

/// Filtered + paged usage logs with totals (`GET /api/usage-logs`).
pub async fn usage_logs(state: &AppState, query: &UsageLogQuery) -> ApiResult<UsageLogsResponse> {
    let all = state.db.list_usage(None);
    let search = query.search.clone().unwrap_or_default().to_lowercase();
    let filtered: Vec<&UsageLogEntry> = all
        .iter()
        .filter(|entry| {
            if let Some(provider) = query.provider_id.as_deref().filter(|p| !p.is_empty()) {
                if entry.provider_id != provider {
                    return false;
                }
            }
            if let Some(role) = query.agent_role.as_deref().filter(|r| !r.is_empty()) {
                if entry.agent_role.as_deref().unwrap_or("planner") != role {
                    return false;
                }
            }
            if !search.is_empty() {
                let haystack = format!(
                    "{} {} {} {}",
                    entry.provider_id,
                    entry.model,
                    entry.run_id,
                    entry.agent_role.clone().unwrap_or_default()
                )
                .to_lowercase();
                if !haystack.contains(&search) {
                    return false;
                }
            }
            true
        })
        .collect();

    let total = filtered.len();
    let unique_providers: Vec<String> = {
        let mut providers: Vec<String> = filtered.iter().map(|entry| entry.provider_id.clone()).collect();
        providers.sort();
        providers.dedup();
        providers
    };
    let unique_roles: Vec<String> = {
        let mut roles: Vec<String> = filtered
            .iter()
            .map(|entry| entry.agent_role.clone().unwrap_or_else(|| "planner".to_string()))
            .collect();
        roles.sort();
        roles.dedup();
        roles
    };
    let offset = query.offset.unwrap_or(0).max(0) as usize;
    let limit = query.limit.unwrap_or(50).clamp(1, 500) as usize;
    let logs: Vec<UsageLogEntry> = filtered
        .iter()
        .skip(offset)
        .take(limit)
        .map(|entry| (*entry).clone())
        .collect();
    let total_cost: f64 = filtered.iter().map(|entry| entry.cost).sum();
    let total_tokens: i64 = filtered.iter().map(|entry| entry.input_tokens + entry.output_tokens).sum();
    let summed_input: i64 = filtered.iter().map(|entry| entry.input_tokens).sum();
    let summed_read: i64 = filtered.iter().map(|entry| entry.cache_read_tokens).sum();
    let avg_cache_hit_rate = if summed_input > 0 {
        summed_read as f64 * 100.0 / summed_input as f64
    } else {
        0.0
    };

    Ok(UsageLogsResponse {
        logs,
        total,
        unique_providers,
        unique_roles,
        metrics: UsageMetrics {
            total_cost,
            total_calls: total,
            total_tokens,
            avg_cache_hit_rate,
        },
    })
}

// ----------------------------------------------------------------------
// GET /api/runs/:id/events
// ----------------------------------------------------------------------

/// Subscribes to a run's live event stream (`GET /api/runs/:id/events`,
/// without the HTTP-only leading `{"type":"connected"}` event).
///
/// Semantics:
/// * terminal run in the DB — replays the terminal event, then closes;
/// * live handle — replays pending permission/question, then forwards every
///   event until the handle's sender closes or the last receiver goes away;
/// * otherwise — polls until a handle appears, the run turns terminal, or
///   the last receiver drops.
pub async fn subscribe_run_events(state: &AppState, run_id: &str) -> EventRx {
    let (tx, rx) = tokio::sync::broadcast::channel::<RunEvent>(1024);

    // 1. Run already terminal in the DB: replay the terminal event and close.
    if let Some(run) = state.db.get_run_full(run_id) {
        if is_terminal(&run.status) {
            let _ = tx.send(terminal_event_value(&run.status, run.error_message.as_deref()));
            drop(tx);
            return rx;
        }
    }

    // 2. Live handle: replay pending, then forward.
    if let Some(handle) = state.orchestrator.handle(run_id) {
        let source = handle.events.subscribe();
        replay_pending(&tx, &handle).await;
        tokio::spawn(forward_from_handle(tx, source, handle));
        return rx;
    }

    // 3. No live handle and not terminal (or run unknown): poll until a
    //    handle appears, the run turns terminal, or the last receiver drops.
    let orchestrator = state.orchestrator.clone();
    let db = state.db.clone();
    let run_id = run_id.to_string();
    tokio::spawn(async move {
        loop {
            if tx.receiver_count() == 0 {
                return;
            }
            if let Some(handle) = orchestrator.handle(&run_id) {
                let source = handle.events.subscribe();
                replay_pending(&tx, &handle).await;
                forward_from_handle(tx, source, handle).await;
                return;
            }
            if let Some(run) = db.get_run_full(&run_id) {
                if is_terminal(&run.status) {
                    let _ = tx.send(terminal_event_value(&run.status, run.error_message.as_deref()));
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_summary_aggregates_by_role() {
        let entry = |role: Option<&str>, cost: f64, input: i64, output: i64| UsageLogEntry {
            run_id: "r".into(),
            agent_role: role.map(str::to_string),
            provider_id: "p".into(),
            model: "m".into(),
            input_tokens: input,
            output_tokens: output,
            cost,
            ..Default::default()
        };
        let entries = vec![
            entry(None, 0.10, 1000, 200),
            entry(Some("coder"), 0.05, 500, 100),
            entry(Some("coder"), 0.02, 500, 100),
        ];
        let summary = usage_summary("r", &entries);
        assert_eq!(summary.total_calls, 3);
        assert!((summary.total_cost - 0.17).abs() < 1e-9);
        assert_eq!(summary.total_input_tokens, 2000);
        assert_eq!(summary.total_output_tokens, 400);
        assert_eq!(summary.by_role.len(), 2);
        assert_eq!(summary.by_role[0].agent_role, "planner");
        assert_eq!(summary.by_role[1].agent_role, "coder");
        assert_eq!(summary.by_role[1].calls, 2);
    }

    #[test]
    fn title_truncation() {
        assert_eq!(truncate_title("short task"), "short task");
        let long = "x".repeat(100);
        let title = truncate_title(&long);
        assert_eq!(title.chars().count(), 61);
        assert!(title.ends_with('…'));
    }
}
