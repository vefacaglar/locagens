//! MCP facade: list, create/update, delete, restart and toggle logic for the
//! Settings → MCP feature.

use crate::api::error::{ApiError, ApiResult};
use crate::services::mcp as mcp_service;
use crate::AppState;

/// Resolves `raw` to a registered project path; `None` when absent, empty or
/// unregistered.
fn resolve_project(state: &AppState, raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim().to_string();
    if raw.is_empty() {
        return None;
    }
    match crate::api::projects::require_registered_project(state, &raw) {
        Ok(path) => Some(path.to_string_lossy().to_string()),
        Err(_) => None,
    }
}

/// Lists MCP server configs visible to the project (or user scope).
/// Mirrors `GET /api/mcp/servers`.
pub async fn list_mcp_servers(state: &AppState, project_path: Option<&str>) -> ApiResult<mcp_service::McpServersResponse> {
    let project = resolve_project(state, project_path);
    let servers: Vec<mcp_service::McpServerInfo> = mcp_service::list_configs(project.as_deref())
        .iter()
        .map(mcp_service::server_info)
        .collect();
    Ok(mcp_service::McpServersResponse { servers })
}

/// Validates and saves an MCP server create/update payload (all errors are
/// BadRequest, as the route returned). Mirrors `POST /api/mcp/servers`.
pub async fn upsert_mcp_server(state: &AppState, input: mcp_service::McpServerInput) -> ApiResult<mcp_service::McpServerInfo> {
    let scope = if input.scope.as_deref() == Some("project") { "project" } else { "user" };
    let registered = if scope == "project" {
        let raw = input.project_path.clone().unwrap_or_default();
        if raw.trim().is_empty() {
            return Err(ApiError::bad_request(
                "projectPath is required for project MCP servers.",
            ));
        }
        Some(
            crate::api::projects::require_registered_project(state, raw.trim())?
                .to_string_lossy()
                .to_string(),
        )
    } else {
        None
    };

    let config = mcp_service::validate_input(&input, registered.as_deref()).map_err(ApiError::bad_request)?;

    mcp_service::save_config(&config).map_err(ApiError::bad_request)?;
    Ok(mcp_service::server_info(&config))
}

/// Deletes an MCP server config by name (trimmed + lowercased); returns
/// whether anything was deleted. Mirrors `DELETE /api/mcp/servers/:name`.
pub async fn delete_mcp_server(state: &AppState, name: &str, project_path: Option<&str>) -> ApiResult<bool> {
    let name = name.trim().to_lowercase();
    if name.is_empty() {
        return Err(ApiError::bad_request("Server name is required."));
    }
    let project = resolve_project(state, project_path);

    mcp_service::delete_config(&name, project.as_deref()).map_err(ApiError::bad_request)
}

/// Restarts an MCP server process and returns its info.
/// Mirrors `POST /api/mcp/servers/:name/restart`.
pub async fn restart_mcp_server(state: &AppState, name: &str, project_path: Option<&str>) -> ApiResult<mcp_service::McpServerInfo> {
    let name = name.trim().to_lowercase();
    if name.is_empty() {
        return Err(ApiError::bad_request("Server name is required."));
    }
    let project = project_path
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .and_then(|p| resolve_project(state, Some(p.as_str())));

    mcp_service::restart(&name, project.as_deref()).map_err(ApiError::bad_request)
}

/// Enables/disables an MCP server and returns its info.
/// Mirrors `PUT /api/mcp/servers/:name/enabled`.
pub async fn toggle_mcp_server(state: &AppState, name: &str, enabled: bool, project_path: Option<&str>) -> ApiResult<mcp_service::McpServerInfo> {
    let name = name.trim().to_lowercase();
    if name.is_empty() {
        return Err(ApiError::bad_request("Server name is required."));
    }
    let project = project_path
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .and_then(|p| resolve_project(state, Some(p.as_str())));

    mcp_service::toggle(&name, enabled, project.as_deref()).map_err(ApiError::bad_request)
}
