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

/// Lists MCP servers visible to the project (or user scope) with live probe
/// status: enabled servers get a concurrent `tools/list` probe (or their
/// cached result), disabled servers report `disabled`.
/// Mirrors `GET /api/mcp/servers`.
pub async fn list_mcp_servers(state: &AppState, project_path: Option<&str>) -> ApiResult<mcp_service::McpServersResponse> {
    let project = resolve_project(state, project_path);
    let servers = mcp_service::probe_all(project.as_deref()).await;
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
    // The stored config changed: drop stale probes for this server name.
    mcp_service::invalidate_server(&config.name);
    Ok(mcp_service::probe_server(&config).await)
}

/// Deletes an MCP server config by name (trimmed + lowercased); returns
/// whether anything was deleted. Mirrors `DELETE /api/mcp/servers/:name`.
pub async fn delete_mcp_server(state: &AppState, name: &str, project_path: Option<&str>) -> ApiResult<bool> {
    let name = name.trim().to_lowercase();
    if name.is_empty() {
        return Err(ApiError::bad_request("Server name is required."));
    }
    let project = resolve_project(state, project_path);

    let deleted = mcp_service::delete_config(&name, project.as_deref()).map_err(ApiError::bad_request)?;
    mcp_service::invalidate_server(&name);
    Ok(deleted)
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

    let config = mcp_service::find_config(&name, project.as_deref())
        .ok_or_else(|| ApiError::bad_request(format!("MCP server \"{name}\" not found.")))?;
    mcp_service::invalidate_server(&name);
    Ok(mcp_service::probe_server(&config).await)
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

    let info = mcp_service::toggle(&name, enabled, project.as_deref()).map_err(ApiError::bad_request)?;
    mcp_service::invalidate_server(&name);
    Ok(info)
}

fn find_remote_server(name: &str, project: Option<&str>) -> ApiResult<mcp_service::McpServerConfig> {
    let name = name.trim().to_lowercase();
    let config = mcp_service::effective_configs(project)
        .into_iter()
        .find(|cfg| cfg.name.eq_ignore_ascii_case(&name))
        .ok_or_else(|| ApiError::bad_request(format!("MCP server \"{name}\" not found.")))?;
    if config.url.is_none() || config.transport == "stdio" {
        return Err(ApiError::bad_request(format!(
            "MCP server \"{name}\" is not a remote (http) server."
        )));
    }
    Ok(config)
}

/// Signs a remote MCP server in via OAuth (discovery, dynamic registration,
/// PKCE, loopback callback). `on_url` receives the authorization URL (open it
/// in a browser or print it); tokens go to `~/.locagens/mcp-oauth.json`.
/// Returns the re-probed server info.
pub async fn login_mcp_server(
    state: &AppState,
    name: &str,
    project_path: Option<&str>,
    on_url: &(dyn Fn(&str) + Sync),
) -> ApiResult<mcp_service::McpServerInfo> {
    let project = resolve_project(state, project_path);
    let config = find_remote_server(name, project.as_deref())?;
    let store = mcp_service::oauth::TokenStore::default_location();
    mcp_service::oauth::login(&store, &config, on_url)
        .await
        .map_err(|err| ApiError::bad_request(format!("MCP login failed: {err}")))?;
    mcp_service::invalidate_server(&config.name);
    Ok(mcp_service::probe_server(&config).await)
}

/// Removes the stored OAuth token of a remote MCP server; returns whether one
/// existed.
pub async fn logout_mcp_server(state: &AppState, name: &str, project_path: Option<&str>) -> ApiResult<bool> {
    let project = resolve_project(state, project_path);
    let config = find_remote_server(name, project.as_deref())?;
    let store = mcp_service::oauth::TokenStore::default_location();
    let removed = mcp_service::oauth::logout(&store, &config).map_err(ApiError::bad_request)?;
    mcp_service::invalidate_server(&config.name);
    Ok(removed)
}
