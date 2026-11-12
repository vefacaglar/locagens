//! Projects, workspace file listing, native folder picker, and git routes.
//! Thin adapters over the `crate::api::projects` facade.

use crate::api::projects as projects_api;
use crate::routes::respond;
use crate::util::{canonical_project_path, json_error};
use crate::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Deserialize, Default)]
pub struct ProjectPathQuery {
    pub path: Option<String>,
    pub query: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GenerateMessageInput {
    pub run_id: Option<String>,
}

/// Backwards-compatible shim: registered-project validation used by other
/// route modules; maps the facade error onto its message.
pub fn require_registered_project_pub(state: &AppState, raw: &str) -> Result<PathBuf, String> {
    projects_api::require_registered_project(state, raw).map_err(|err| err.message)
}

pub async fn list_projects(State(state): State<Arc<AppState>>) -> Response {
    respond(projects_api::list_projects(&state).await)
}

pub async fn create_project(
    State(state): State<Arc<AppState>>,
    body: Option<Json<projects_api::CreateProjectInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    respond(projects_api::create_project(&state, input).await)
}

pub async fn delete_project(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProjectPathQuery>,
) -> Response {
    let Some(raw_path) = query.path.filter(|p| !p.is_empty()) else {
        return json_error(StatusCode::BAD_REQUEST, "Missing required query parameter: path");
    };
    respond(projects_api::delete_project(&state, &raw_path).await.map(|_| json!({ "success": true })))
}

pub async fn select_dir() -> Response {
    #[cfg(target_os = "macos")]
    {
        let output = tokio::process::Command::new("osascript")
            .args(["-e", "POSIX path of (choose folder with prompt \"Select a project folder:\")"])
            .output()
            .await;
        match output {
            Ok(out) if out.status.success() => {
                let selected = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if selected.is_empty() {
                    return json_error(StatusCode::BAD_REQUEST, "No folder selected");
                }
                match canonical_project_path(&selected) {
                    Ok(path) => {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "Workspace".to_string());
                        Json(json!({ "path": path.to_string_lossy(), "name": name })).into_response()
                    }
                    Err(err) => json_error(StatusCode::BAD_REQUEST, &err),
                }
            }
            Ok(_) => json_error(StatusCode::BAD_REQUEST, "Selection cancelled by user."),
            Err(err) => json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to open folder picker: {err}"),
            ),
        }
    }
    #[cfg(windows)]
    {
        let script = "Add-Type -AssemblyName System.Windows.Forms; \
                      $f = New-Object System.Windows.Forms.FolderBrowserDialog; \
                      $f.Description = 'Select a project folder:'; \
                      $r = $f.ShowDialog((New-Object System.Windows.Forms.Form -Property @{TopMost = $true})); \
                      if ($r -eq [System.Windows.Forms.DialogResult]::OK) { $f.SelectedPath }";
        let output = tokio::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .output()
            .await;
        match output {
            Ok(out) if out.status.success() => {
                let selected = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if selected.is_empty() {
                    return json_error(StatusCode::BAD_REQUEST, "No folder selected");
                }
                match canonical_project_path(&selected) {
                    Ok(path) => {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "Workspace".to_string());
                        Json(json!({ "path": path.to_string_lossy(), "name": name })).into_response()
                    }
                    Err(err) => json_error(StatusCode::BAD_REQUEST, &err),
                }
            }
            Ok(_) => json_error(StatusCode::BAD_REQUEST, "Selection cancelled by user."),
            Err(err) => json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to open folder picker: {err}"),
            ),
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        json_error(
            StatusCode::BAD_REQUEST,
            "Automatic folder picking is only supported on macOS and Windows. Please input the path manually.",
        )
    }
}

pub async fn list_files(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProjectPathQuery>,
) -> Response {
    let Some(raw_path) = query.path.filter(|p| !p.is_empty()) else {
        return json_error(StatusCode::BAD_REQUEST, "Missing required query parameter: path");
    };

    respond(
        projects_api::list_files(&state, &raw_path, query.query.as_deref())
            .await
            .map(|files| -> Value { json!({ "files": files }) }),
    )
}

pub async fn git_status(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProjectPathQuery>,
) -> Response {
    let Some(raw_path) = query.path.filter(|p| !p.is_empty()) else {
        return json_error(StatusCode::BAD_REQUEST, "Missing required query parameter: path");
    };
    respond(projects_api::git_status(&state, &raw_path).await)
}

pub async fn git_diff_details(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProjectPathQuery>,
) -> Response {
    let Some(raw_path) = query.path.filter(|p| !p.is_empty()) else {
        return json_error(StatusCode::BAD_REQUEST, "Missing required query parameter: path");
    };
    respond(
        projects_api::git_diff_details(&state, &raw_path)
            .await
            .map(|files| -> Value { json!({ "files": files }) }),
    )
}

pub async fn git_generate_message(
    State(state): State<Arc<AppState>>,
    body: Option<Json<GenerateMessageInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    let Some(run_id) = input.run_id.filter(|id| !id.is_empty()) else {
        return json_error(StatusCode::BAD_REQUEST, "Missing required body parameter: runId");
    };
    respond(
        projects_api::git_generate_message(&state, &run_id)
            .await
            .map(|message| -> Value { json!({ "message": message }) }),
    )
}

pub async fn git_commit(
    State(state): State<Arc<AppState>>,
    body: Option<Json<projects_api::GitCommitInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    respond(
        projects_api::git_commit(&state, input)
            .await
            .map(|_| -> Value { json!({ "success": true }) }),
    )
}
