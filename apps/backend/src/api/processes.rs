//! Background process facade
//! (HTTP equivalent: `routes/processes.rs`).

use crate::api::error::{ApiError, ApiResult};
use crate::services::processes::{ProcessInfo, ProcessLogEntry};
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Input for spawning a background process
/// (HTTP equivalent: `POST /api/processes/spawn` body).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SpawnProcessInput {
    pub command: Option<String>,
    pub project_path: Option<String>,
    pub env: Option<HashMap<String, String>>,
}

/// Logs of a background process (`GET /api/processes/:id/logs`).
/// (`Default` is impossible: `ProcessInfo` does not implement it.)
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProcessLogs {
    pub process: ProcessInfo,
    pub logs: Vec<ProcessLogEntry>,
}

/// Lists tracked background processes, optionally filtered to a registered
/// project directory (`GET /api/processes`). A path that does not resolve to a
/// registered project is ignored (no filter), matching the HTTP route.
pub async fn list_processes(
    state: &AppState,
    project_path: Option<&str>,
) -> ApiResult<Vec<ProcessInfo>> {
    let project = match project_path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(raw) => match crate::api::projects::require_registered_project(state, raw) {
            Ok(path) => Some(path.to_string_lossy().to_string()),
            Err(_) => None,
        },
        None => None,
    };
    Ok(state.processes.list(project.as_deref()))
}

/// Spawns a background process in a registered project directory
/// (`POST /api/processes/spawn`). Falls back to the default project path when
/// no `projectPath` is given.
pub async fn spawn_process(
    state: &AppState,
    input: &SpawnProcessInput,
) -> ApiResult<ProcessInfo> {
    let Some(command) = input
        .command
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    else {
        return Err(ApiError::bad_request("Missing required parameter: command"));
    };

    let raw_project = input
        .project_path
        .clone()
        .unwrap_or_else(|| state.default_project_path.to_string_lossy().to_string());
    if raw_project.trim().is_empty() {
        return Err(ApiError::bad_request("Project path is required."));
    }
    let canonical = crate::api::projects::require_registered_project(state, raw_project.trim())
        .map_err(|err| ApiError::bad_request(err.message))?;

    Ok(state
        .processes
        .spawn(command, canonical, input.env.clone()))
}

/// Kills a background process; returns whether a process with that id existed
/// (`POST /api/processes/:id/kill`).
pub async fn kill_process(state: &AppState, id: &str) -> ApiResult<bool> {
    Ok(state.processes.kill(id).await)
}

/// Restarts a background process; 404 when the id is unknown
/// (`POST /api/processes/:id/restart`).
pub async fn restart_process(state: &AppState, id: &str) -> ApiResult<ProcessInfo> {
    state
        .processes
        .restart(id)
        .await
        .ok_or_else(|| ApiError::not_found("Process not found"))
}

/// Returns a background process plus its captured log lines; 404 when the id
/// is unknown (`GET /api/processes/:id/logs`).
pub async fn process_logs(state: &AppState, id: &str) -> ApiResult<ProcessLogs> {
    let Some(process) = state.processes.get(id) else {
        return Err(ApiError::not_found("Process not found"));
    };
    Ok(ProcessLogs {
        process,
        logs: state.processes.get_logs(id),
    })
}
