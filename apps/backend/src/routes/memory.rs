//! Memory CRUD routes (Settings → Memory). Thin adapters over
//! `crate::api::memory`.

use crate::api::memory::{self, CreateMemoryInput};
use crate::routes::{api_error_response, respond};
use crate::util::json_error;
use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMemoryInput {
    pub content: Option<String>,
}

pub async fn list_memories(State(state): State<Arc<AppState>>) -> Response {
    respond(memory::list_memories(&state).await)
}

pub async fn create_memory(
    State(state): State<Arc<AppState>>,
    body: Option<Json<CreateMemoryInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    respond(memory::create_memory(&state, input).await)
}

pub async fn update_memory(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<UpdateMemoryInput>>,
) -> Response {
    let Ok(numeric_id) = id.parse::<i64>() else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid memory id");
    };
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    respond(
        memory::update_memory(&state, numeric_id, &input.content.unwrap_or_default()).await,
    )
}

pub async fn delete_memory(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let Ok(numeric_id) = id.parse::<i64>() else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid memory id");
    };
    match memory::delete_memory(&state, numeric_id).await {
        Ok(()) => Json(json!({ "success": true })).into_response(),
        Err(err) => api_error_response(err),
    }
}

pub async fn clear_memories(State(state): State<Arc<AppState>>) -> Response {
    match memory::clear_memories(&state).await {
        Ok(()) => Json(json!({ "success": true })).into_response(),
        Err(err) => api_error_response(err),
    }
}
