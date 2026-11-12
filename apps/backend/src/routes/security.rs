//! Security status & Windows sandbox setup routes. Thin adapters over
//! `crate::api::security`.

use crate::api::security;
use crate::routes::respond;
use axum::response::Response;
use axum::Json;
use serde_json::Value;

pub async fn status() -> Response {
    respond(security::security_status().await)
}

pub async fn sandbox_setup(body: Option<Json<Value>>) -> Response {
    let confirmed = body
        .and_then(|Json(value)| value.get("confirmed").and_then(Value::as_bool))
        .unwrap_or(false);
    respond(security::setup_sandbox(confirmed).await)
}
