//! Background process routes.

use crate::util::json_error;
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProcessListQuery {
    pub project_path: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SpawnInput {
    pub command: Option<String>,
    pub project_path: Option<String>,
    pub env: Option<HashMap<String, String>>,
}

pub async fn list_processes(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProcessListQuery>,
) -> Response {
    let project = match query.project_path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(raw) => match crate::routes::projects::require_registered_project_pub(&state, raw) {
            Ok(path) => Some(path.to_string_lossy().to_string()),
            Err(_) => None,
        },
        None => None,
    };
    Json(json!({ "processes": state.processes.list(project.as_deref()) })).into_response()
}

pub async fn spawn_process(
    State(state): State<Arc<AppState>>,
    body: Option<Json<SpawnInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    let Some(command) = input.command.as_deref().map(str::trim).filter(|c| !c.is_empty()) else {
        return json_error(StatusCode::BAD_REQUEST, "Missing required parameter: command");
    };

    let raw_project = input
        .project_path
        .clone()
        .unwrap_or_else(|| state.default_project_path.to_string_lossy().to_string());
    if raw_project.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "Project path is required.");
    }
    let canonical = match crate::routes::projects::require_registered_project_pub(&state, raw_project.trim()) {
        Ok(path) => path,
        Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
    };

    let info = state
        .processes
        .spawn(command, PathBuf::from(&canonical), input.env.clone());
    Json(json!({ "process": info })).into_response()
}

pub async fn kill_process(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let success = state.processes.kill(&id).await;
    Json(json!({ "success": success })).into_response()
}

pub async fn restart_process(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    match state.processes.restart(&id).await {
        Some(info) => Json(json!({ "process": info })).into_response(),
        None => json_error(StatusCode::NOT_FOUND, "Process not found"),
    }
}

pub async fn get_logs(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.processes.get(&id) {
        Some(info) => Json(json!({ "process": info, "logs": state.processes.get_logs(&id) }))
            .into_response(),
        None => json_error(StatusCode::NOT_FOUND, "Process not found"),
    }
}
