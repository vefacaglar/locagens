//! Plugins facade: discover, templates, install, toggle and delete logic
//! for the Settings → Plugins feature.

use crate::api::error::{ApiError, ApiResult};
use crate::services::plugins as plugin_service;
use crate::AppState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Input for `toggle_plugin`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToggleInput {
    pub enabled: Option<bool>,
    pub scope: Option<String>,
    pub project_path: Option<String>,
}

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

/// Lists installed plugins plus plugin templates and the user/project plugin
/// roots.
pub async fn list_plugins(state: &AppState, project_path: Option<&str>) -> ApiResult<plugin_service::PluginsResponse> {
    let project = resolve_project(state, project_path);
    Ok(plugin_service::PluginsResponse {
        plugins: plugin_service::discover(project.as_deref()),
        templates: plugin_service::templates(),
        user_plugins_dir: plugin_service::user_root().to_string_lossy().to_string(),
        project_plugins_dir: plugin_service::project_root(project.as_deref())
            .map(|p| p.to_string_lossy().to_string()),
    })
}

/// Available plugin templates.
pub fn plugin_templates() -> ApiResult<Vec<Value>> {
    Ok(plugin_service::templates())
}

/// Installs a plugin from a template or local folder (service failures map
/// to Internal).
pub async fn install_plugin(state: &AppState, payload: plugin_service::InstallPayload) -> ApiResult<Value> {
    if payload.uri.is_empty() || payload.source.is_empty() {
        return Err(ApiError::bad_request(
            "source and uri are required to install a plugin.",
        ));
    }

    let scope = if payload.scope.as_deref() == Some("project") { "project" } else { "user" };
    let mut project_path = payload.project_path.clone();
    if scope == "project" {
        let raw = project_path.clone().unwrap_or_default();
        if raw.trim().is_empty() {
            return Err(ApiError::bad_request(
                "projectPath is required for project-scoped plugins.",
            ));
        }
        project_path = Some(
            crate::api::projects::require_registered_project(state, raw.trim())?
                .to_string_lossy()
                .to_string(),
        );
    }

    plugin_service::install(&payload.source, &payload.uri, scope, project_path.as_deref())
        .map_err(ApiError::internal)
}

/// Toggles a plugin's enabled flag; service failures are NotFound, matching
/// NotFound.
pub async fn toggle_plugin(state: &AppState, id: &str, input: ToggleInput) -> ApiResult<Value> {
    let enabled = !matches!(input.enabled, Some(false));
    let scope = if input.scope.as_deref() == Some("project") { "project" } else { "user" };
    let project = if scope == "project" {
        resolve_project(state, input.project_path.as_deref())
    } else {
        None
    };

    plugin_service::toggle(id.trim(), enabled, scope, project.as_deref()).map_err(ApiError::not_found)
}

/// Deletes a plugin directory by id. Unknown id -> NotFound
/// "Plugin not found or could not be deleted."; service errors -> Internal.
pub async fn delete_plugin(
    state: &AppState,
    id: &str,
    scope: Option<&str>,
    project_path: Option<&str>,
) -> ApiResult<()> {
    let scope = if scope == Some("project") { "project" } else { "user" };
    let project = if scope == "project" {
        resolve_project(state, project_path)
    } else {
        None
    };

    match plugin_service::delete(id.trim(), scope, project.as_deref()) {
        Ok(true) => Ok(()),
        Ok(false) => Err(ApiError::not_found("Plugin not found or could not be deleted.")),
        Err(err) => Err(ApiError::internal(err)),
    }
}
