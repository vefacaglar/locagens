//! Provider, agent-preset and native-`pi` auth routes.
//!
//! Backed entirely by `PiProviderManager` (pure Rust handling of
//! `~/.pi/agent/auth.json`, `models.json`, `models-store.json` and Locagens
//! metadata files).

use crate::pi_providers::{
    FetchModelsInput, SaveProviderConfigInput, SetPiAuthInput, TestProviderInput,
};
use crate::util::json_error;
use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// `GET /api/providers` — all configured / authenticated providers.
pub async fn list_providers(State(state): State<Arc<AppState>>) -> Response {
    Json(state.pi.list_providers().await).into_response()
}

/// `GET /api/providers/config` — editable configs with masked API keys.
pub async fn get_config(State(state): State<Arc<AppState>>) -> Response {
    Json(state.pi.get_configs()).into_response()
}

/// `POST /api/providers/config` — saves the whole provider config map the
/// web client sends (`Record<providerId, ProviderConfig>`); providers
/// missing from the map are removed.
pub async fn save_config(
    State(state): State<Arc<AppState>>,
    body: Option<Json<HashMap<String, SaveProviderConfigInput>>>,
) -> Response {
    let Some(Json(configs)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    match state.pi.save_configs(&configs) {
        Ok(()) => Json(json!({ "success": true })).into_response(),
        Err(err) => json_error(
            StatusCode::BAD_REQUEST,
            &format!("Failed to save provider config: {err}"),
        ),
    }
}

/// `POST /api/providers/fetch-models` — remote `{baseUrl}/models` + cached
/// `models-store.json` models, merged.
pub async fn fetch_models(
    State(state): State<Arc<AppState>>,
    body: Option<Json<FetchModelsInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    match state.pi.fetch_models(&input).await {
        Ok(models) => Json(json!({ "success": true, "models": models })).into_response(),
        Err(err) => Json(json!({ "success": false, "error": err.to_string() })).into_response(),
    }
}

/// `POST /api/providers/test` — quick completion request through the
/// provider's endpoint using the resolved `auth.json` credential.
pub async fn test_provider(
    State(state): State<Arc<AppState>>,
    body: Option<Json<TestProviderInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    match state.pi.test_provider(&input).await {
        Ok((content, usage)) => {
            Json(json!({ "success": true, "content": content, "usage": usage })).into_response()
        }
        Err(err) => Json(json!({ "success": false, "error": err.to_string() })).into_response(),
    }
}

/// `GET /api/agent-presets` — architect / coder / utility presets.
pub async fn get_presets(State(state): State<Arc<AppState>>) -> Response {
    Json(state.pi.get_presets()).into_response()
}

/// `PUT /api/agent-presets` — merge presets into `locagens-presets.json`.
pub async fn put_presets(
    State(state): State<Arc<AppState>>,
    body: Option<Json<Value>>,
) -> Response {
    let Some(Json(body)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    match state.pi.save_presets(&body) {
        Ok(presets) => Json(presets).into_response(),
        Err(err) => json_error(
            StatusCode::BAD_REQUEST,
            &format!("Failed to save agent presets: {err}"),
        ),
    }
}

/// `GET /api/pi/auth` — masked credential status from `~/.pi/agent/auth.json`.
pub async fn get_pi_auth(State(state): State<Arc<AppState>>) -> Response {
    Json(state.pi.pi_auth_status()).into_response()
}

/// `POST /api/pi/auth` — set or delete a provider credential.
pub async fn set_pi_auth(
    State(state): State<Arc<AppState>>,
    body: Option<Json<SetPiAuthInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    match state.pi.set_pi_auth(&input) {
        Ok(()) => Json(json!({ "success": true })).into_response(),
        Err(err) => json_error(
            StatusCode::BAD_REQUEST,
            &format!("Failed to update pi auth: {err}"),
        ),
    }
}
