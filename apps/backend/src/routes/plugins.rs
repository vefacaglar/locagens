//! Settings → Plugins routes.

use crate::services::plugins as plugin_service;
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
pub struct ProjectScopeQuery {
    pub project_path: Option<String>,
    pub scope: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToggleInput {
    pub enabled: Option<bool>,
    pub scope: Option<String>,
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

pub async fn list_plugins(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProjectScopeQuery>,
) -> Response {
    let project = resolve_project(&state, query.project_path.as_deref());
    let response = plugin_service::PluginsResponse {
        plugins: plugin_service::discover(project.as_deref()),
        templates: plugin_service::templates(),
        user_plugins_dir: plugin_service::user_root().to_string_lossy().to_string(),
        project_plugins_dir: plugin_service::project_root(project.as_deref())
            .map(|p| p.to_string_lossy().to_string()),
    };
    Json(response).into_response()
}

pub async fn list_templates() -> Response {
    Json(json!({ "templates": plugin_service::templates() })).into_response()
}

pub async fn install_plugin(
    State(state): State<Arc<AppState>>,
    body: Option<Json<plugin_service::InstallPayload>>,
) -> Response {
    let Some(Json(payload)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    if payload.uri.is_empty() || payload.source.is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "source and uri are required to install a plugin.",
        );
    }

    let scope = if payload.scope.as_deref() == Some("project") { "project" } else { "user" };
    let mut project_path = payload.project_path.clone();
    if scope == "project" {
        let raw = project_path.clone().unwrap_or_default();
        if raw.trim().is_empty() {
            return json_error(
                StatusCode::BAD_REQUEST,
                "projectPath is required for project-scoped plugins.",
            );
        }
        project_path = match crate::routes::projects::require_registered_project_pub(&state, raw.trim()) {
            Ok(path) => Some(path.to_string_lossy().to_string()),
            Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
        };
    }

    match plugin_service::install(&payload.source, &payload.uri, scope, project_path.as_deref()) {
        Ok(plugin) => Json(json!({ "success": true, "plugin": plugin })).into_response(),
        Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &err),
    }
}

pub async fn toggle_plugin(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<ToggleInput>>,
) -> Response {
    let Json(input) = body.unwrap_or_default();
    let enabled = !matches!(input.enabled, Some(false));
    let scope = if input.scope.as_deref() == Some("project") { "project" } else { "user" };
    let project = if scope == "project" {
        resolve_project(&state, input.project_path.as_deref())
    } else {
        None
    };

    match plugin_service::toggle(id.trim(), enabled, scope, project.as_deref()) {
        Ok(plugin) => Json(json!({ "success": true, "plugin": plugin })).into_response(),
        Err(err) => json_error(StatusCode::NOT_FOUND, &err),
    }
}

pub async fn delete_plugin(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<ProjectScopeQuery>,
) -> Response {
    let scope = if query.scope.as_deref() == Some("project") { "project" } else { "user" };
    let project = if scope == "project" {
        resolve_project(&state, query.project_path.as_deref())
    } else {
        None
    };

    match plugin_service::delete(id.trim(), scope, project.as_deref()) {
        Ok(true) => Json(json!({ "success": true, "deleted": true })).into_response(),
        Ok(false) => {
            json_error(StatusCode::NOT_FOUND, "Plugin not found or could not be deleted.")
        }
        Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &err),
    }
}
