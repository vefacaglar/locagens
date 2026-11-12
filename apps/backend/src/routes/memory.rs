//! Memory CRUD routes (Settings → Memory).

use crate::util::json_error;
use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

const CATEGORIES: [&str; 4] = ["user", "feedback", "project", "reference"];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateMemoryInput {
    pub scope: Option<String>,
    pub category: Option<String>,
    pub content: Option<String>,
    pub project_path: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMemoryInput {
    pub content: Option<String>,
}

pub async fn list_memories(State(state): State<Arc<AppState>>) -> Response {
    Json(state.db.list_memories()).into_response()
}

pub async fn create_memory(
    State(state): State<Arc<AppState>>,
    body: Option<Json<CreateMemoryInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    let content = input.content.as_deref().unwrap_or("").trim().to_string();
    if content.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "content is required");
    }
    let scope = if input.scope.as_deref() == Some("global") { "global" } else { "project" };
    let category = match input.category.as_deref() {
        Some(cat) if CATEGORIES.contains(&cat) => cat.to_string(),
        _ => "project".to_string(),
    };
    if scope == "project" && input.project_path.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "projectPath is required for a project-scoped memory",
        );
    }

    let project_path = if scope == "project" {
        input.project_path.as_deref().unwrap_or("").trim().to_string()
    } else {
        String::new()
    };

    match state.db.create_memory(scope, &project_path, &category, &content) {
        Ok(memory) => Json(memory).into_response(),
        Err(err) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to save memory: {err}"),
        ),
    }
}

pub async fn update_memory(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<UpdateMemoryInput>>,
) -> Response {
    let Ok(numeric_id) = id.parse::<i64>() else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid memory id");
    };
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    let content = input.content.unwrap_or_default().trim().to_string();
    if content.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "content is required");
    }
    match state.db.update_memory(numeric_id, &content) {
        Some(memory) => Json(memory).into_response(),
        None => json_error(
            StatusCode::NOT_FOUND,
            &format!("Memory with id \"{id}\" not found"),
        ),
    }
}

pub async fn delete_memory(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let Ok(numeric_id) = id.parse::<i64>() else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid memory id");
    };
    if !state.db.delete_memory(numeric_id) {
        return json_error(
            StatusCode::NOT_FOUND,
            &format!("Memory with id \"{id}\" not found"),
        );
    }
    Json(json!({ "success": true })).into_response()
}

pub async fn clear_memories(State(state): State<Arc<AppState>>) -> Response {
    state.db.clear_memories();
    Json(json!({ "success": true })).into_response()
}
