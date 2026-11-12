//! `settings.json` routes — currently only the backend port.

use crate::AppState;
use crate::util::json_error;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use std::sync::Arc;

/// Stored port, or the default when settings.json is absent/invalid.
pub fn read_stored_port(settings_path: &std::path::Path) -> Option<u16> {
    let raw = std::fs::read_to_string(settings_path).ok()?;
    let parsed: Value = serde_json::from_str(&raw).ok()?;
    normalize_port(parsed.get("port")?)
}

/// Validates a candidate port, returning a clamped integer or None.
pub fn normalize_port(value: &Value) -> Option<u16> {
    let port = value.as_u64().or_else(|| value.as_i64().map(|v| v as u64))?;
    if (1..=65535).contains(&port) {
        Some(port as u16)
    } else {
        None
    }
}

pub async fn get_settings(State(state): State<Arc<AppState>>) -> Response {
    let port = read_stored_port(&state.settings_path).unwrap_or(crate::DEFAULT_PORT);
    Json(json!({ "port": port })).into_response()
}

pub async fn put_settings(
    State(state): State<Arc<AppState>>,
    body: Option<Json<Value>>,
) -> Response {
    let Some(Json(body)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    let port = body
        .get("port")
        .and_then(normalize_port);
    let Some(port) = port else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "Invalid port. Use an integer between 1 and 65535.",
        );
    };

    if let Some(parent) = state.settings_path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to create settings directory: {err}"),
            );
        }
    }
    let content = json!({ "port": port });
    if let Err(err) = std::fs::write(
        &state.settings_path,
        serde_json::to_vec_pretty(&content).unwrap_or_default(),
    ) {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to save settings: {err}"),
        );
    }

    Json(json!({ "port": port, "restartRequired": true })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_ports() {
        assert_eq!(normalize_port(&json!(4321)), Some(4321));
        assert_eq!(normalize_port(&json!(0)), None);
        assert_eq!(normalize_port(&json!(65536)), None);
        assert_eq!(normalize_port(&json!("4321")), None);
    }
}
