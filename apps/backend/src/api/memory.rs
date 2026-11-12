//! Memory CRUD facade (Settings → Memory).

use crate::api::error::{ApiError, ApiResult};
use crate::AppState;
use serde::{Deserialize, Serialize};

/// Allowed memory categories; anything else falls back to `"project"`.
const CATEGORIES: [&str; 4] = ["user", "feedback", "project", "reference"];

/// Input for creating a memory (HTTP equivalent: `POST /api/memories` body).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct CreateMemoryInput {
    pub scope: Option<String>,
    pub category: Option<String>,
    pub content: Option<String>,
    pub project_path: Option<String>,
}

/// Lists all memories, newest update first (`GET /api/memories`).
pub async fn list_memories(state: &AppState) -> ApiResult<Vec<crate::db::Memory>> {
    Ok(state.db.list_memories())
}

/// Creates a memory after validating content, scope, category and project path
/// (`POST /api/memories`).
pub async fn create_memory(state: &AppState, input: CreateMemoryInput) -> ApiResult<crate::db::Memory> {
    let content = input.content.as_deref().unwrap_or("").trim().to_string();
    if content.is_empty() {
        return Err(ApiError::bad_request("content is required"));
    }
    let scope = if input.scope.as_deref() == Some("global") { "global" } else { "project" };
    let category = match input.category.as_deref() {
        Some(cat) if CATEGORIES.contains(&cat) => cat.to_string(),
        _ => "project".to_string(),
    };
    if scope == "project"
        && input
            .project_path
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
    {
        return Err(ApiError::bad_request(
            "projectPath is required for a project-scoped memory",
        ));
    }

    let project_path = if scope == "project" {
        input
            .project_path
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_string()
    } else {
        String::new()
    };

    state
        .db
        .create_memory(scope, &project_path, &category, &content)
        .map_err(|err| ApiError::internal(format!("Failed to save memory: {err}")))
}

/// Updates the content of a memory; 404 when the id does not exist
/// (`PUT /api/memories/:id`).
pub async fn update_memory(state: &AppState, id: i64, content: &str) -> ApiResult<crate::db::Memory> {
    let content = content.trim().to_string();
    if content.is_empty() {
        return Err(ApiError::bad_request("content is required"));
    }
    state.db.update_memory(id, &content).ok_or_else(|| {
        ApiError::not_found(format!("Memory with id \"{id}\" not found"))
    })
}

/// Deletes a memory; 404 when the id does not exist (`DELETE /api/memories/:id`).
pub async fn delete_memory(state: &AppState, id: i64) -> ApiResult<()> {
    if !state.db.delete_memory(id) {
        return Err(ApiError::not_found(format!(
            "Memory with id \"{id}\" not found"
        )));
    }
    Ok(())
}

/// Deletes every memory (`DELETE /api/memories`).
pub async fn clear_memories(state: &AppState) -> ApiResult<()> {
    state.db.clear_memories();
    Ok(())
}
