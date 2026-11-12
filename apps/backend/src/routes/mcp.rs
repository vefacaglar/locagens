//! Settings → MCP routes: list, create/update, delete, restart, toggle.
//! Thin adapters over the `crate::api::mcp` facade.

use crate::api::mcp as mcp_api;
use crate::routes::respond;
use crate::services::mcp as mcp_service;
use crate::util::json_error;
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectScopeQuery {
    pub project_path: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectBody {
    pub project_path: Option<String>,
}

pub async fn list_servers(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProjectScopeQuery>,
) -> Response {
    respond(mcp_api::list_mcp_servers(&state, query.project_path.as_deref()).await)
}

pub async fn upsert_server(
    State(state): State<Arc<AppState>>,
    body: Option<Json<mcp_service::McpServerInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    respond(
        mcp_api::upsert_mcp_server(&state, input)
            .await
            .map(|server| -> Value { json!({ "success": true, "server": server }) }),
    )
}

pub async fn delete_server(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(query): Query<ProjectScopeQuery>,
    body: Option<Json<ProjectBody>>,
) -> Response {
    // Body projectPath wins over query, exactly as before.
    let raw_project = body
        .and_then(|Json(b)| b.project_path)
        .or_else(|| query.project_path.clone())
        .unwrap_or_default();
    respond(
        mcp_api::delete_mcp_server(&state, &name, Some(raw_project.trim()))
            .await
            .map(|deleted| -> Value { json!({ "success": true, "deleted": deleted }) }),
    )
}

pub async fn restart_server(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    body: Option<Json<ProjectBody>>,
) -> Response {
    let project = body.and_then(|Json(b)| b.project_path);
    respond(
        mcp_api::restart_mcp_server(&state, &name, project.as_deref())
            .await
            .map(|server| -> Value { json!({ "success": true, "server": server }) }),
    )
}

pub async fn toggle_server(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    body: Option<Json<Value>>,
) -> Response {
    let (enabled, project_raw) = match body {
        Some(Json(value)) => (
            value.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            value
                .get("projectPath")
                .and_then(Value::as_str)
                .map(str::to_string),
        ),
        None => (true, None),
    };
    respond(
        mcp_api::toggle_mcp_server(&state, &name, enabled, project_raw.as_deref())
            .await
            .map(|server| -> Value { json!({ "success": true, "server": server }) }),
    )
}
