//! `/api/runs/*` HTTP + SSE routes: run lifecycle, messages, usage, plan,
//! permission/question coordination and the live event stream.
//!
//! Wire-compatible with `apps/web/src/api/client.ts` and `useChatSession.ts`.

use crate::db::{Run, RunMessage, UsageLogEntry};
use crate::util::{json_error, now_iso};
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::stream::Stream;
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;

/// `POST /api/runs` body.
#[derive(Deserialize, Debug, Default)]
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
    pub coder_provider_id: Option<String>,
    pub coder_model: Option<String>,
    pub coder_reasoning_effort: Option<String>,
    pub utility_provider_id: Option<String>,
    pub utility_model: Option<String>,
    pub utility_reasoning_effort: Option<String>,
    pub agent_preset: Option<String>,
}

/// `POST /api/runs/:id/continue` body — same fields, all provider fields
/// optional so the frontend can resend the full composer state.
#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ContinueRunPayload {
    pub task: String,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub mode: Option<String>,
    #[serde(default)]
    pub bypass_permissions: bool,
    pub coder_provider_id: Option<String>,
    pub coder_model: Option<String>,
    pub coder_reasoning_effort: Option<String>,
    pub utility_provider_id: Option<String>,
    pub utility_model: Option<String>,
    pub utility_reasoning_effort: Option<String>,
    pub agent_preset: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct PermissionPayload {
    pub decision: String,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct AnswerPayload {
    #[serde(default)]
    pub selections: Vec<Vec<String>>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct MidRunMessagePayload {
    pub content: String,
}

#[derive(Deserialize, Debug, Default)]
pub struct UsageLogQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub search: Option<String>,
    #[serde(rename = "providerId")]
    pub provider_id: Option<String>,
    #[serde(rename = "agentRole")]
    pub agent_role: Option<String>,
}

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

// ----------------------------------------------------------------------
// POST /api/runs
// ----------------------------------------------------------------------

pub async fn create_run(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateRunPayload>,
) -> Response {
    if payload.task.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "Task is required.");
    }
    if payload.provider_id.trim().is_empty() || payload.model.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "Provider and model are required.");
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
        provider_display_name: provider_display_name(&state, &payload.provider_id),
        model: payload.model.clone(),
        reasoning_effort: payload.reasoning_effort.clone().filter(|e| !e.is_empty()),
        mode: mode.clone(),
        coder_provider_id: payload.coder_provider_id.clone().filter(|p| !p.is_empty()),
        coder_model: payload.coder_model.clone().filter(|m| !m.is_empty()),
        coder_reasoning_effort: payload.coder_reasoning_effort.clone().filter(|e| !e.is_empty()),
        agent_preset: payload.agent_preset.clone().filter(|p| !p.is_empty()),
        utility_provider_id: payload.utility_provider_id.clone().filter(|p| !p.is_empty()),
        utility_model: payload.utility_model.clone().filter(|m| !m.is_empty()),
        utility_reasoning_effort: payload.utility_reasoning_effort.clone().filter(|e| !e.is_empty()),
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

    state.orchestrator.spawn(state.clone(), &run_id);

    Json(serde_json::to_value(&run).unwrap_or(Value::Null)).into_response()
}

/// The run's effective mode: explicit bypass wins over a milder mode.
fn effective_mode(mode: Option<&str>, bypass: bool) -> String {
    let mode = mode.filter(|mode| !mode.trim().is_empty()).unwrap_or("accept_edits");
    if bypass && !matches!(mode, "chat" | "plan") {
        "auto".to_string()
    } else {
        mode.to_string()
    }
}

// ----------------------------------------------------------------------
// POST /api/runs/:id/continue
// ----------------------------------------------------------------------

pub async fn continue_run(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    Json(payload): Json<ContinueRunPayload>,
) -> Response {
    let Some(existing) = state.db.get_run_full(&run_id) else {
        return json_error(StatusCode::NOT_FOUND, "Run not found.");
    };
    if active_run_statuses().contains(&existing.status.as_str()) && existing.status != "generating" {
        return json_error(
            StatusCode::CONFLICT,
            "Run is awaiting your input or approval; resolve it before sending a new message.",
        );
    }
    if payload.task.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "Task is required.");
    }

    let mode = effective_mode(payload.mode.as_deref(), payload.bypass_permissions);
    let message = RunMessage {
        id: format!("msg-{}", uuid::Uuid::new_v4()),
        run_id: run_id.clone(),
        role: "user".to_string(),
        content: payload.task.clone(),
        created_at: now_iso(),
        ..Default::default()
    };

    state.db.patch_run(&run_id, |run| {
        run.status = "generating".to_string();
        run.error_message = None;
        run.mode = mode.clone();
        if let Some(provider_id) = payload.provider_id.as_deref().filter(|p| !p.is_empty()) {
            run.provider_id = provider_id.to_string();
            run.provider_display_name = provider_display_name(&state, provider_id);
        }
        if let Some(model) = payload.model.as_deref().filter(|m| !m.is_empty()) {
            run.model = model.to_string();
        }
        run.reasoning_effort = payload.reasoning_effort.clone().filter(|e| !e.is_empty());
        run.agent_preset = payload.agent_preset.clone().filter(|p| !p.is_empty());
        run.coder_provider_id = payload.coder_provider_id.clone().filter(|p| !p.is_empty());
        run.coder_model = payload.coder_model.clone().filter(|m| !m.is_empty());
        run.coder_reasoning_effort = payload.coder_reasoning_effort.clone().filter(|e| !e.is_empty());
        run.utility_provider_id = payload.utility_provider_id.clone().filter(|p| !p.is_empty());
        run.utility_model = payload.utility_model.clone().filter(|m| !m.is_empty());
        run.utility_reasoning_effort = payload.utility_reasoning_effort.clone().filter(|e| !e.is_empty());
    });
    state.db.append_message(message.clone());

    if let Some(handle) = state.orchestrator.handle(&run_id) {
        handle.emit(json!({
            "type": "message_created",
            "message": serde_json::to_value(&message).unwrap_or(Value::Null),
        }));
    }
    state.orchestrator.emit_status(&run_id, "generating");
    state.orchestrator.spawn(state.clone(), &run_id);

    Json(json!({ "success": true })).into_response()
}

// ----------------------------------------------------------------------
// POST /api/runs/:id/cancel
// ----------------------------------------------------------------------

pub async fn cancel_run(State(state): State<Arc<AppState>>, Path(run_id): Path<String>) -> Response {
    if let Some(handle) = state.orchestrator.handle(&run_id) {
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

    if let Some(run) = state.db.get_run_full(&run_id) {
        if active_run_statuses().contains(&run.status.as_str()) {
            state.db.patch_run(&run_id, |run| run.status = "cancelled".to_string());
            state.orchestrator.emit_status(&run_id, "cancelled");
        }
    }
    Json(json!({ "success": true })).into_response()
}

// ----------------------------------------------------------------------
// POST /api/runs/:id/messages
// ----------------------------------------------------------------------

pub async fn queue_message(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    Json(payload): Json<MidRunMessagePayload>,
) -> Response {
    let Some(handle) = state.orchestrator.handle(&run_id) else {
        return json_error(StatusCode::CONFLICT, "Run is not active.");
    };
    let content = payload.content.trim().to_string();
    if content.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "Message content is required.");
    }
    handle.inbox.lock().await.push_back(content);
    Json(json!({ "success": true })).into_response()
}

// ----------------------------------------------------------------------
// POST /api/runs/:id/permission
// ----------------------------------------------------------------------

pub async fn permission_decision(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    Json(payload): Json<PermissionPayload>,
) -> Response {
    let decision = payload.decision.trim().to_string();
    if !["allow_once", "allow_project", "allow_always", "allow_run", "deny"]
        .contains(&decision.as_str())
    {
        return json_error(StatusCode::BAD_REQUEST, "Invalid permission decision.");
    }
    let Some(handle) = state.orchestrator.handle(&run_id) else {
        return json_error(StatusCode::CONFLICT, "No pending permission request for this run.");
    };

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
            if matches!(decision.as_str(), "allow_project" | "allow_always") {
                if let Some(run) = state.db.get_run_full(&run_id) {
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
                        &decision,
                    );
                }
            }
        }
    }
    match sender {
        Some(sender) => {
            let _ = sender.send(decision);
            Json(json!({ "success": true })).into_response()
        }
        None => json_error(StatusCode::CONFLICT, "No pending permission request for this run."),
    }
}

// ----------------------------------------------------------------------
// POST /api/runs/:id/answer
// ----------------------------------------------------------------------

pub async fn answer_question(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    Json(payload): Json<AnswerPayload>,
) -> Response {
    let Some(handle) = state.orchestrator.handle(&run_id) else {
        return json_error(StatusCode::CONFLICT, "No pending question for this run.");
    };
    let mut slot = handle.question.lock().await;
    let sender = slot.sender.take();
    slot.pending = None;
    drop(slot);
    match sender {
        Some(sender) => {
            let _ = sender.send(json!({
                "selections": payload.selections,
                "notes": payload.notes,
            }));
            Json(json!({ "success": true })).into_response()
        }
        None => json_error(StatusCode::CONFLICT, "No pending question for this run."),
    }
}

// ----------------------------------------------------------------------
// GET /api/runs, /api/runs/:id, messages, usage, plan, pending
// ----------------------------------------------------------------------

pub async fn list_runs(State(state): State<Arc<AppState>>) -> Response {
    let mut runs = state.db.list_runs();
    runs.sort_by(|a, b| b.last_active_at.clone().unwrap_or_default().cmp(&a.last_active_at.clone().unwrap_or_default()));
    let runs: Vec<Value> = runs
        .iter()
        .map(|run| serde_json::to_value(run).unwrap_or(Value::Null))
        .collect();
    Json(runs).into_response()
}

pub async fn get_run(State(state): State<Arc<AppState>>, Path(run_id): Path<String>) -> Response {
    match state.db.get_run_full(&run_id) {
        Some(run) => Json(serde_json::to_value(&run).unwrap_or(Value::Null)).into_response(),
        None => json_error(StatusCode::NOT_FOUND, "Run not found."),
    }
}

pub async fn list_messages(State(state): State<Arc<AppState>>, Path(run_id): Path<String>) -> Response {
    if state.db.get_run_full(&run_id).is_none() {
        return json_error(StatusCode::NOT_FOUND, "Run not found.");
    }
    let messages = state.db.list_messages(&run_id);
    let messages: Vec<Value> = messages
        .iter()
        .map(|message| serde_json::to_value(message).unwrap_or(Value::Null))
        .collect();
    Json(messages).into_response()
}

/// `RunUsageSummary` aggregation over the run's usage logs.
pub fn usage_summary(run_id: &str, entries: &[UsageLogEntry]) -> Value {
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

    let mut by_role: Vec<(String, f64, usize, i64, i64)> = Vec::new();
    for entry in entries {
        let role = entry.agent_role.clone().unwrap_or_else(|| "planner".to_string());
        if let Some(slot) = by_role.iter_mut().find(|(existing, ..)| *existing == role) {
            slot.1 += entry.cost;
            slot.2 += 1;
            slot.3 += entry.input_tokens;
            slot.4 += entry.output_tokens;
        } else {
            by_role.push((role, entry.cost, 1, entry.input_tokens, entry.output_tokens));
        }
    }
    let by_role: Vec<Value> = by_role
        .into_iter()
        .map(|(agent_role, cost, calls, input_tokens, output_tokens)| {
            json!({
                "agentRole": agent_role,
                "cost": cost,
                "calls": calls,
                "inputTokens": input_tokens,
                "outputTokens": output_tokens,
            })
        })
        .collect();

    json!({
        "runId": run_id,
        "totalCost": total_cost,
        "totalCalls": total_calls,
        "totalInputTokens": total_input,
        "totalOutputTokens": total_output,
        "totalCacheReadTokens": total_cache_read,
        "avgCacheHitRate": avg_cache_hit_rate,
        "byRole": by_role,
    })
}

pub async fn run_usage(State(state): State<Arc<AppState>>, Path(run_id): Path<String>) -> Response {
    if state.db.get_run_full(&run_id).is_none() {
        return json_error(StatusCode::NOT_FOUND, "Run not found.");
    }
    Json(usage_summary(&run_id, &state.db.list_usage(Some(&run_id)))).into_response()
}

pub async fn run_plan(State(state): State<Arc<AppState>>, Path(run_id): Path<String>) -> Response {
    match state.db.get_plan(&run_id) {
        Some(plan) => Json(serde_json::to_value(&plan).unwrap_or(Value::Null)).into_response(),
        None => Json(Value::Null).into_response(),
    }
}

pub async fn run_pending(State(state): State<Arc<AppState>>, Path(run_id): Path<String>) -> Response {
    let Some(handle) = state.orchestrator.handle(&run_id) else {
        return Json(json!({ "permissionRequest": null, "questionRequest": null })).into_response();
    };
    Json(json!({
        "permissionRequest": handle.pending_permission().await,
        "questionRequest": handle.pending_question().await,
    }))
    .into_response()
}

// ----------------------------------------------------------------------
// GET /api/usage-logs
// ----------------------------------------------------------------------

pub async fn usage_logs(
    State(state): State<Arc<AppState>>,
    Query(query): Query<UsageLogQuery>,
) -> Response {
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
    let page: Vec<Value> = filtered
        .iter()
        .skip(offset)
        .take(limit)
        .map(|entry| serde_json::to_value(entry).unwrap_or(Value::Null))
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

    Json(json!({
        "logs": page,
        "total": total,
        "uniqueProviders": unique_providers,
        "uniqueRoles": unique_roles,
        "metrics": {
            "totalCost": total_cost,
            "totalCalls": total,
            "totalTokens": total_tokens,
            "avgCacheHitRate": avg_cache_hit_rate
        }
    }))
    .into_response()
}

// ----------------------------------------------------------------------
// GET /api/runs/:id/events (SSE)
// ----------------------------------------------------------------------

/// Boxed stream so all three return branches share one concrete type.
type EventStream = std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

fn event_from_value(value: Value) -> Result<Event, Infallible> {
    Ok(Event::default().data(value.to_string()))
}

pub async fn run_events(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> Sse<EventStream> {
    let mut prefix: Vec<Result<Event, Infallible>> =
        vec![Ok(Event::default().data(json!({ "type": "connected" }).to_string()))];

    let run = state.db.get_run_full(&run_id);
    let handle = state.orchestrator.handle(&run_id);

    if let Some(run) = run.as_ref() {
        if is_terminal(&run.status) {
            prefix.push(terminal_event(&run.status, run.error_message.as_deref()));
            let stream: EventStream = Box::pin(futures::stream::iter(prefix));
            return Sse::new(stream).keep_alive(KeepAlive::default());
        }
    }

    if let Some(handle) = handle.as_ref() {
        if let Some(permission) = handle.pending_permission().await {
            prefix.push(event_from_value(permission));
        }
        if let Some(question) = handle.pending_question().await {
            prefix.push(event_from_value(question));
        }
        let receiver = handle.events.subscribe();
        let live = BroadcastStream::new(receiver).filter_map(|item| {
            let payload: Option<Value> = match item {
                Ok(event) => match event.get("type").and_then(Value::as_str) {
                    Some("run_completed") | Some("run_failed") => Some(event),
                    Some("status_changed") => {
                        let status = event.get("status").and_then(Value::as_str).unwrap_or_default();
                        if is_terminal(status) {
                            Some(terminal_sse_payload(status))
                        } else {
                            Some(event)
                        }
                    }
                    _ => Some(event),
                },
                Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(_)) => {
                    Some(json!({ "type": "lagged" }))
                }
            };
            payload.map(event_from_value)
        });
        let stream: EventStream = Box::pin(
            futures::stream::iter(prefix)
                .chain(live),
        );
        return Sse::new(stream).keep_alive(KeepAlive::default());
    }

    // No live handle and not terminal: keep the stream open with a heartbeat;
    // the client reconnects if the run is picked up again.
    let idle = futures::stream::repeat(()).then(|()| async move {
        tokio::time::sleep(Duration::from_secs(15)).await;
        Ok(Event::default().comment("keep-alive"))
    });
    let stream: EventStream = Box::pin(futures::stream::iter(prefix).chain(idle));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn terminal_sse_payload(status: &str) -> Value {
    json!({ "type": "status_changed", "status": status })
}

fn terminal_event(status: &str, error_message: Option<&str>) -> Result<Event, Infallible> {
    let payload = match status {
        "done" => json!({ "type": "run_completed", "finalOutput": "" }),
        "failed" => json!({
            "type": "run_failed",
            "errorMessage": error_message.unwrap_or("Run failed.")
        }),
        _ => terminal_sse_payload(status),
    };
    Ok(Event::default().data(payload.to_string()))
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
        assert_eq!(summary["totalCalls"], 3);
        assert!((summary["totalCost"].as_f64().unwrap() - 0.17).abs() < 1e-9);
        assert_eq!(summary["totalInputTokens"], 2000);
        assert_eq!(summary["totalOutputTokens"], 400);
        let roles = summary["byRole"].as_array().unwrap();
        assert_eq!(roles.len(), 2);
        assert_eq!(roles[0]["agentRole"], "planner");
        assert_eq!(roles[1]["agentRole"], "coder");
        assert_eq!(roles[1]["calls"], 2);
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
