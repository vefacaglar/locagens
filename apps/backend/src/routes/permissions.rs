//! Standing permission list / revoke routes (Settings → Permissions). Thin
//! adapters over `crate::api::permissions`.

use crate::api::permissions;
use crate::routes::{api_error_response, respond};
use crate::util::json_error;
use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::sync::Arc;

pub async fn list_permissions(State(state): State<Arc<AppState>>) -> Response {
    respond(permissions::list_permissions(&state).await)
}

pub async fn delete_permission(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let Ok(numeric_id) = id.parse::<i64>() else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid permission id");
    };
    match permissions::delete_permission(&state, numeric_id).await {
        Ok(()) => Json(json!({ "success": true })).into_response(),
        Err(err) => api_error_response(err),
    }
}

pub async fn clear_permissions(State(state): State<Arc<AppState>>) -> Response {
    match permissions::clear_permissions(&state).await {
        Ok(()) => Json(json!({ "success": true })).into_response(),
        Err(err) => api_error_response(err),
    }
}
