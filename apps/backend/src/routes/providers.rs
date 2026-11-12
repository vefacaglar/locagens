//! Provider, agent-preset and native-`pi` auth routes. Thin adapters over
//! `crate::api::providers`.

use crate::api::providers;
use crate::pi_providers::{
    FetchModelsInput, SaveProviderConfigInput, SetPiAuthInput, TestProviderInput,
};
use crate::routes::{api_error_response, respond};
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
    respond(providers::list_providers(&state).await)
}

/// `GET /api/providers/config` — editable configs with masked API keys.
pub async fn get_config(State(state): State<Arc<AppState>>) -> Response {
    respond(providers::get_provider_configs(&state).await)
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
    match providers::save_provider_configs(&state, &configs).await {
        Ok(()) => Json(json!({ "success": true })).into_response(),
        Err(err) => api_error_response(err),
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
    respond(providers::fetch_models(&state, &input).await)
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
    respond(providers::test_provider(&state, &input).await)
}

/// `GET /api/agent-presets` — architect / coder / utility presets.
pub async fn get_presets(State(state): State<Arc<AppState>>) -> Response {
    respond(providers::get_agent_presets(&state).await)
}

/// `PUT /api/agent-presets` — merge presets into `locagens-presets.json`.
pub async fn put_presets(
    State(state): State<Arc<AppState>>,
    body: Option<Json<Value>>,
) -> Response {
    let Some(Json(body)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    match providers::save_agent_presets(&state, &body).await {
        Ok(presets) => Json(presets).into_response(),
        Err(err) => api_error_response(err),
    }
}

/// `GET /api/pi/auth` — masked credential status from `~/.pi/agent/auth.json`.
pub async fn get_pi_auth(State(state): State<Arc<AppState>>) -> Response {
    respond(providers::get_pi_auth(&state).await)
}

/// `POST /api/pi/auth` — set or delete a provider credential.
pub async fn set_pi_auth(
    State(state): State<Arc<AppState>>,
    body: Option<Json<SetPiAuthInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    match providers::set_pi_auth(&state, &input).await {
        Ok(()) => Json(json!({ "success": true })).into_response(),
        Err(err) => api_error_response(err),
    }
}
