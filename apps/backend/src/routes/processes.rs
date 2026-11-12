//! Background process routes. Thin adapters over `crate::api::processes`.

use crate::api::processes::{self, SpawnProcessInput};
use crate::routes::api_error_response;
use crate::util::json_error;
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProcessListQuery {
    pub project_path: Option<String>,
}

pub async fn list_processes(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProcessListQuery>,
) -> Response {
    match processes::list_processes(&state, query.project_path.as_deref()).await {
        Ok(list) => Json(json!({ "processes": list })).into_response(),
        Err(err) => api_error_response(err),
    }
}

pub async fn spawn_process(
    State(state): State<Arc<AppState>>,
    body: Option<Json<SpawnProcessInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    match processes::spawn_process(&state, &input).await {
        Ok(info) => Json(json!({ "process": info })).into_response(),
        Err(err) => api_error_response(err),
    }
}

pub async fn kill_process(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    match processes::kill_process(&state, &id).await {
        Ok(success) => Json(json!({ "success": success })).into_response(),
        Err(err) => api_error_response(err),
    }
}

pub async fn restart_process(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    match processes::restart_process(&state, &id).await {
        Ok(info) => Json(json!({ "process": info })).into_response(),
        Err(err) => api_error_response(err),
    }
}

pub async fn get_logs(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match processes::process_logs(&state, &id).await {
        Ok(logs) => Json(logs).into_response(),
        Err(err) => api_error_response(err),
    }
}
