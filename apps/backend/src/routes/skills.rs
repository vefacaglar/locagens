//! Settings → Skills routes: list, install, open-folder, delete.
//! Thin adapters over the `crate::api::skills` facade.

use crate::api::skills as skills_api;
use crate::routes::respond;
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
pub struct SkillListQuery {
    pub project_path: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillTargetQuery {
    pub target: Option<String>,
    pub project_path: Option<String>,
}

pub async fn list_skills(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SkillListQuery>,
) -> Response {
    respond(skills_api::list_skills(&state, query.project_path.as_deref()).await)
}

pub async fn get_skill(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SkillTargetQuery>,
    Path(name): Path<String>,
) -> Response {
    respond(skills_api::get_skill(&state, &name, query.project_path.as_deref()).await)
}

pub async fn install_skill(
    State(state): State<Arc<AppState>>,
    body: Option<Json<skills_api::InstallSkillInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    respond(
        skills_api::install_skill(&state, input)
            .await
            .map(|skill| -> Value { json!({ "success": true, "skill": skill }) }),
    )
}

pub async fn open_folder(
    State(state): State<Arc<AppState>>,
    body: Option<Json<skills_api::InstallSkillInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    respond(skills_api::skills_folder(&state, input).await)
}

pub async fn delete_skill(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(query): Query<SkillTargetQuery>,
    body: Option<Json<skills_api::InstallSkillInput>>,
) -> Response {
    // Body values win over query values, exactly as before.
    let body_target = body
        .as_ref()
        .and_then(|Json(b)| b.target.clone())
        .or_else(|| query.target.clone());
    let body_project = body
        .as_ref()
        .and_then(|Json(b)| b.project_path.clone())
        .or_else(|| query.project_path.clone());

    respond(
        skills_api::delete_skill(&state, &name, body_target.as_deref(), body_project.as_deref())
            .await
            .map(|deleted| -> Value { json!({ "success": true, "deleted": deleted }) }),
    )
}
