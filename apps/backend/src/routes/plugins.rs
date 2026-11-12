//! Settings → Plugins routes. Thin adapters over the `crate::api::plugins`
//! facade.

use crate::api::plugins as plugins_api;
use crate::routes::respond;
use crate::services::plugins as plugin_service;
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
    pub scope: Option<String>,
}

pub async fn list_plugins(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProjectScopeQuery>,
) -> Response {
    respond(plugins_api::list_plugins(&state, query.project_path.as_deref()).await)
}

pub async fn list_templates() -> Response {
    respond(
        plugins_api::plugin_templates()
            .map(|templates| -> Value { json!({ "templates": templates }) }),
    )
}

pub async fn install_plugin(
    State(state): State<Arc<AppState>>,
    body: Option<Json<plugin_service::InstallPayload>>,
) -> Response {
    let Some(Json(payload)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    respond(
        plugins_api::install_plugin(&state, payload)
            .await
            .map(|plugin| -> Value { json!({ "success": true, "plugin": plugin }) }),
    )
}

pub async fn toggle_plugin(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<plugins_api::ToggleInput>>,
) -> Response {
    let Json(input) = body.unwrap_or_default();
    respond(
        plugins_api::toggle_plugin(&state, &id, input)
            .await
            .map(|plugin| -> Value { json!({ "success": true, "plugin": plugin }) }),
    )
}

pub async fn delete_plugin(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<ProjectScopeQuery>,
) -> Response {
    respond(
        plugins_api::delete_plugin(&state, &id, query.scope.as_deref(), query.project_path.as_deref())
            .await
            .map(|_| -> Value { json!({ "success": true, "deleted": true }) }),
    )
}
