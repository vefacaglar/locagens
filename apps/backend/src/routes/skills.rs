//! Settings → Skills routes: list, install, open-folder, delete.

use crate::services::skills as skill_service;
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
pub struct SkillListQuery {
    pub project_path: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillTargetQuery {
    pub target: Option<String>,
    pub project_path: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct InstallSkillInput {
    pub target: Option<String>,
    pub content: Option<String>,
    pub project_path: Option<String>,
}

fn resolve_project(state: &AppState, raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim().to_string();
    if raw.is_empty() {
        return None;
    }
    // Try a registered-project check, fall back to any existing directory.
    match crate::routes::projects::require_registered_project_pub(state, &raw) {
        Ok(path) => Some(path.to_string_lossy().to_string()),
        Err(_) => skill_service::validate_project_path(&raw),
    }
}

pub async fn list_skills(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SkillListQuery>,
) -> Response {
    let project = resolve_project(&state, query.project_path.as_deref());
    let response = crate::services::skills::SkillsListResponse {
        skills: skill_service::list(project.as_deref()),
        roots: crate::services::skills::SkillRoots {
            user: skill_service::user_root().to_string_lossy().to_string(),
            project: skill_service::project_root(project.as_deref())
                .map(|p| p.to_string_lossy().to_string()),
        },
    };
    Json(response).into_response()
}

pub async fn get_skill(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SkillTargetQuery>,
    Path(name): Path<String>,
) -> Response {
    let name = name.trim().to_string();
    if name.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "Skill name is required");
    }
    let project = resolve_project(&state, query.project_path.as_deref());
    let found = skill_service::list(project.as_deref())
        .into_iter()
        .find(|skill| skill.name == name);
    match found {
        Some(skill) => Json(skill).into_response(),
        None => json_error(StatusCode::NOT_FOUND, &format!("Skill \"{name}\" not found")),
    }
}

pub async fn install_skill(
    State(state): State<Arc<AppState>>,
    body: Option<Json<InstallSkillInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    let target = match input.target.as_deref() {
        Some("project") => "project",
        Some("user") => "user",
        _ => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "target must be \"user\" or \"project\"",
            )
        }
    };
    let content = input.content.clone().unwrap_or_default();
    if content.trim().is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "content is required (full SKILL.md text)",
        );
    }

    let project = if target == "project" {
        let raw = input.project_path.clone().unwrap_or_default();
        if raw.trim().is_empty() {
            return json_error(
                StatusCode::BAD_REQUEST,
                "projectPath is required for project skills",
            );
        }
        match crate::routes::projects::require_registered_project_pub(&state, raw.trim()) {
            Ok(path) => Some(path.to_string_lossy().to_string()),
            Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
        }
    } else {
        None
    };

    match skill_service::install(target, &content, project.as_deref()) {
        Ok(skill) => Json(json!({ "success": true, "skill": skill })).into_response(),
        Err(err) => json_error(StatusCode::BAD_REQUEST, &err),
    }
}

pub async fn open_folder(
    State(state): State<Arc<AppState>>,
    body: Option<Json<InstallSkillInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    let target = match input.target.as_deref() {
        Some("project") => "project",
        Some("user") => "user",
        _ => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "target must be \"user\" or \"project\"",
            )
        }
    };

    if target == "user" {
        let dir = skill_service::ensure_user_root();
        return Json(json!({ "path": dir.to_string_lossy(), "target": "user" })).into_response();
    }

    let raw = input.project_path.clone().unwrap_or_default();
    if raw.trim().is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "projectPath is required for project skills folder",
        );
    }
    let project = match crate::routes::projects::require_registered_project_pub(&state, raw.trim()) {
        Ok(path) => path,
        Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
    };

    let dir = match skill_service::ensure_project_root(&project.to_string_lossy()) {
        Some(dir) => dir,
        None => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "Could not create project skills folder",
            )
        }
    };
    // The skills folder must stay inside the project.
    if !dir.starts_with(&project) {
        return json_error(
            StatusCode::BAD_REQUEST,
            "Skills folder resolved outside the project",
        );
    }
    Json(json!({ "path": dir.to_string_lossy(), "target": "project" })).into_response()
}

pub async fn delete_skill(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(query): Query<SkillTargetQuery>,
    body: Option<Json<InstallSkillInput>>,
) -> Response {
    let name = name.trim().to_string();
    if name.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "Skill name is required");
    }
    let body_target = body.as_ref().and_then(|Json(b)| b.target.clone()).or_else(|| query.target.clone());
    let body_project = body
        .and_then(|Json(b)| b.project_path)
        .or_else(|| query.project_path.clone());

    let target = match body_target.as_deref() {
        Some("project") => "project",
        Some("user") => "user",
        _ => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "target must be \"user\" or \"project\"",
            )
        }
    };

    let project = if target == "project" {
        let raw = body_project.unwrap_or_default();
        if raw.trim().is_empty() {
            return json_error(
                StatusCode::BAD_REQUEST,
                "projectPath is required for project skills",
            );
        }
        match crate::routes::projects::require_registered_project_pub(&state, raw.trim()) {
            Ok(path) => Some(path.to_string_lossy().to_string()),
            Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
        }
    } else {
        None
    };

    match skill_service::delete(target, &name, project.as_deref()) {
        Ok(deleted) => Json(json!({ "success": true, "deleted": deleted })).into_response(),
        Err(err) => json_error(StatusCode::BAD_REQUEST, &err),
    }
}
