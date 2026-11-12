//! Security status & Windows sandbox setup routes.

use crate::util::json_error;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

/// Mirrors `CommandSandbox.status()` in compatibility mode: commands run on the
/// host with secrets removed from their environment, so the sandbox reports
/// ready. Native sandboxing arrives with the pi-based orchestrator phase.
async fn security_status() -> Value {
    json!({
        "api": {
            "bindAddress": "127.0.0.1",
            "authenticated": true
        },
        "sandbox": {
            "platform": std::env::consts::OS,
            "status": "ready",
            "errors": [],
            "warnings": [
                "Compatibility mode is active: commands run on the host with secrets removed from their environment."
            ],
            "canInstall": false
        },
        "policy": {
            "commandNetwork": "deny_by_default",
            "workspaceWritesOnly": true
        }
    })
}

pub async fn status() -> Response {
    Json(security_status().await).into_response()
}

pub async fn sandbox_setup(body: Option<Json<Value>>) -> Response {
    let confirmed = body
        .and_then(|Json(value)| value.get("confirmed").and_then(Value::as_bool))
        .unwrap_or(false);
    if !confirmed {
        return json_error(
            StatusCode::BAD_REQUEST,
            "Explicit user confirmation is required.",
        );
    }
    if !cfg!(windows) {
        return json_error(
            StatusCode::BAD_REQUEST,
            "Sandbox setup is only required on Windows.",
        );
    }
    // The Rust backend runs commands in compatibility mode; nothing to
    // install until the native sandbox runtime is wired up.
    Json(security_status().await).into_response()
}
