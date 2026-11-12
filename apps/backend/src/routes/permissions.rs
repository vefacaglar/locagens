//! Standing permission list / revoke routes (Settings → Permissions).

use crate::util::json_error;
use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::sync::Arc;

pub async fn list_permissions(State(state): State<Arc<AppState>>) -> Response {
    Json(state.db.list_permissions()).into_response()
}

pub async fn delete_permission(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let Ok(numeric_id) = id.parse::<i64>() else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid permission id");
    };
    if !state.db.delete_permission(numeric_id) {
        return json_error(
            StatusCode::NOT_FOUND,
            &format!("Permission with id \"{id}\" not found"),
        );
    }
    Json(json!({ "success": true })).into_response()
}

pub async fn clear_permissions(State(state): State<Arc<AppState>>) -> Response {
    state.db.clear_permissions();
    Json(json!({ "success": true })).into_response()
}
