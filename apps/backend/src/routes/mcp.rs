//! Settings → MCP routes: list, create/update, delete, restart, toggle.

use crate::services::mcp as mcp_service;
use crate::util::json_error;
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
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

fn resolve_project(state: &AppState, raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim().to_string();
    if raw.is_empty() {
        return None;
    }
    match crate::routes::projects::require_registered_project_pub(state, &raw) {
        Ok(path) => Some(path.to_string_lossy().to_string()),
        Err(_) => None,
    }
}

pub async fn list_servers(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProjectScopeQuery>,
) -> Response {
    let project = resolve_project(&state, query.project_path.as_deref());
    let servers: Vec<mcp_service::McpServerInfo> = mcp_service::list_configs(project.as_deref())
        .iter()
        .map(mcp_service::server_info)
        .collect();
    let response = mcp_service::McpServersResponse { servers };
    Json(response).into_response()
}

pub async fn upsert_server(
    State(state): State<Arc<AppState>>,
    body: Option<Json<mcp_service::McpServerInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    let scope = if input.scope.as_deref() == Some("project") { "project" } else { "user" };
    let registered = if scope == "project" {
        let raw = input.project_path.clone().unwrap_or_default();
        if raw.trim().is_empty() {
            return json_error(
                StatusCode::BAD_REQUEST,
                "projectPath is required for project MCP servers.",
            );
        }
        match crate::routes::projects::require_registered_project_pub(&state, raw.trim()) {
            Ok(path) => Some(path.to_string_lossy().to_string()),
            Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
        }
    } else {
        None
    };

    let config = match mcp_service::validate_input(&input, registered.as_deref()) {
        Ok(config) => config,
        Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
    };

    match mcp_service::save_config(&config) {
        Ok(()) => {
            let info = mcp_service::server_info(&config);
            Json(json!({ "success": true, "server": info })).into_response()
        }
        Err(err) => json_error(StatusCode::BAD_REQUEST, &err),
    }
}

pub async fn delete_server(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(query): Query<ProjectScopeQuery>,
    body: Option<Json<ProjectBody>>,
) -> Response {
    let name = name.trim().to_lowercase();
    if name.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "Server name is required.");
    }
    let raw_project = body
        .and_then(|Json(b)| b.project_path)
        .or_else(|| query.project_path.clone())
        .unwrap_or_default();
    let project = resolve_project(&state, Some(raw_project.trim()));

    match mcp_service::delete_config(&name, project.as_deref()) {
        Ok(deleted) => Json(json!({ "success": true, "deleted": deleted })).into_response(),
        Err(err) => json_error(StatusCode::BAD_REQUEST, &err),
    }
}

pub async fn restart_server(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    body: Option<Json<ProjectBody>>,
) -> Response {
    let name = name.trim().to_lowercase();
    if name.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "Server name is required.");
    }
    let project = body
        .and_then(|Json(b)| b.project_path)
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .and_then(|p| resolve_project(&state, Some(&p)));

    match mcp_service::restart(&name, project.as_deref()) {
        Ok(info) => Json(json!({ "success": true, "server": info })).into_response(),
        Err(err) => json_error(StatusCode::BAD_REQUEST, &err),
    }
}

pub async fn toggle_server(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    body: Option<Json<Value>>,
) -> Response {
    let name = name.trim().to_lowercase();
    if name.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "Server name is required.");
    }
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
    let project = project_raw
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .and_then(|p| resolve_project(&state, Some(&p)));

    match mcp_service::toggle(&name, enabled, project.as_deref()) {
        Ok(info) => Json(json!({ "success": true, "server": info })).into_response(),
        Err(err) => json_error(StatusCode::BAD_REQUEST, &err),
    }
}
