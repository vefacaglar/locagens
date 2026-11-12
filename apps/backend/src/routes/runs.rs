//! `/api/runs/*` HTTP + SSE routes: thin adapters over the typed facade in
//! `crate::api::runs`. They add HTTP-only validation, the `{ "success": true }`
//! wrappers and the SSE `connected` prefix, and stay wire-compatible with
//! `apps/web/src/api/client.ts` and `useChatSession.ts`.

use crate::api::runs as api;
use crate::api::runs::{CreateRunPayload, ContinueRunPayload, UsageLogQuery};
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::stream::Stream;
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;

use crate::routes::{api_error_response, respond};

/// `POST /api/runs/:id/permission` body.
#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct PermissionPayload {
    pub decision: String,
}

/// `POST /api/runs/:id/answer` body.
#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct AnswerPayload {
    #[serde(default)]
    pub selections: Vec<Vec<String>>,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// `POST /api/runs/:id/messages` body.
#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct MidRunMessagePayload {
    pub content: String,
}

// ----------------------------------------------------------------------
// POST /api/runs
// ----------------------------------------------------------------------

pub async fn create_run(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateRunPayload>,
) -> Response {
    match api::create_run(&state, &payload).await {
        Ok(started) => Json(serde_json::to_value(&started.run).unwrap_or(Value::Null)).into_response(),
        Err(err) => api_error_response(err),
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
    match api::continue_run(&state, &run_id, &payload).await {
        Ok(_) => Json(json!({ "success": true })).into_response(),
        Err(err) => api_error_response(err),
    }
}

// ----------------------------------------------------------------------
// POST /api/runs/:id/cancel
// ----------------------------------------------------------------------

pub async fn cancel_run(State(state): State<Arc<AppState>>, Path(run_id): Path<String>) -> Response {
    respond(api::cancel_run(&state, &run_id).await.map(|_| json!({ "success": true })))
}

// ----------------------------------------------------------------------
// POST /api/runs/:id/messages
// ----------------------------------------------------------------------

pub async fn queue_message(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    Json(payload): Json<MidRunMessagePayload>,
) -> Response {
    respond(
        api::queue_message(&state, &run_id, &payload.content)
            .await
            .map(|_| json!({ "success": true })),
    )
}

// ----------------------------------------------------------------------
// POST /api/runs/:id/permission
// ----------------------------------------------------------------------

pub async fn permission_decision(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    Json(payload): Json<PermissionPayload>,
) -> Response {
    respond(
        api::permission_decision(&state, &run_id, &payload.decision)
            .await
            .map(|_| json!({ "success": true })),
    )
}

// ----------------------------------------------------------------------
// POST /api/runs/:id/answer
// ----------------------------------------------------------------------

pub async fn answer_question(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    Json(payload): Json<AnswerPayload>,
) -> Response {
    respond(
        api::answer_question(&state, &run_id, payload.selections.clone(), payload.notes.clone())
            .await
            .map(|_| json!({ "success": true })),
    )
}

// ----------------------------------------------------------------------
// GET /api/runs, /api/runs/:id, messages, usage, plan, pending
// ----------------------------------------------------------------------

pub async fn list_runs(State(state): State<Arc<AppState>>) -> Response {
    respond(api::list_runs(&state).await)
}

pub async fn get_run(State(state): State<Arc<AppState>>, Path(run_id): Path<String>) -> Response {
    respond(api::get_run(&state, &run_id).await)
}

pub async fn list_messages(State(state): State<Arc<AppState>>, Path(run_id): Path<String>) -> Response {
    respond(api::list_messages(&state, &run_id).await)
}

pub async fn run_usage(State(state): State<Arc<AppState>>, Path(run_id): Path<String>) -> Response {
    respond(api::run_usage(&state, &run_id).await)
}

pub async fn run_plan(State(state): State<Arc<AppState>>, Path(run_id): Path<String>) -> Response {
    respond(api::run_plan(&state, &run_id).await)
}

pub async fn run_pending(State(state): State<Arc<AppState>>, Path(run_id): Path<String>) -> Response {
    respond(api::run_pending(&state, &run_id).await)
}

// ----------------------------------------------------------------------
// GET /api/usage-logs
// ----------------------------------------------------------------------

pub async fn usage_logs(
    State(state): State<Arc<AppState>>,
    Query(query): Query<UsageLogQuery>,
) -> Response {
    respond(api::usage_logs(&state, &query).await)
}

// ----------------------------------------------------------------------
// GET /api/runs/:id/events (SSE)
// ----------------------------------------------------------------------

/// Boxed stream so all return branches share one concrete type.
type EventStream = std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

fn event_from_value(value: Value) -> Result<Event, Infallible> {
    Ok(Event::default().data(value.to_string()))
}

pub async fn run_events(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> Sse<EventStream> {
    let connected = Ok(Event::default().data(json!({ "type": "connected" }).to_string()));

    // Live events from the facade (terminal replay, pending replay and the
    // forwarded broadcast, `lagged` included); the stream simply ends when
    // the facade's channel closes.
    let receiver = api::subscribe_run_events(&state, &run_id).await;
    let live = BroadcastStream::new(receiver).filter_map(|item| {
        let payload: Option<Value> = match item {
            Ok(event) => Some(event),
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(_)) => {
                Some(json!({ "type": "lagged" }))
            }
        };
        payload.map(event_from_value)
    });

    let stream: EventStream = Box::pin(futures::stream::iter([connected]).chain(live));
    Sse::new(stream).keep_alive(KeepAlive::default())
}
