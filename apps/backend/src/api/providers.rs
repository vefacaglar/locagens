//! Provider and native-`pi` auth facade.
//!
//! Thin wrappers over `PiProviderManager` (pure Rust handling of
//! `~/.pi/agent/auth.json`, `models.json`, `models-store.json` and Locagens
//! metadata files).

use crate::api::error::{ApiError, ApiResult};
use crate::pi_providers::{
    EditableProviderConfig, FetchModelsInput, ProviderMetadata, SaveProviderConfigInput,
    SetPiAuthInput, TestProviderInput,
};
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Result of `POST /api/providers/fetch-models`: remote + cached models, or a
/// soft error (never a hard failure).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct FetchModelsResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Result of `POST /api/providers/test`: a quick completion through the
/// provider endpoint, or a soft error (never a hard failure).
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct TestProviderResult {
    pub success: bool,
    pub content: Option<String>,
    pub usage: Option<serde_json::Value>,
    pub error: Option<String>,
}

// Hand-written Serialize so the JSON matches the HTTP route exactly: the
// success branch always emits `content` and `usage` (usage serializes as
// `null` when the provider response has no usage block), the failure branch
// emits only `success` + `error`.
impl Serialize for TestProviderResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("success", &self.success)?;
        if self.success {
            map.serialize_entry("content", &self.content)?;
            map.serialize_entry("usage", &self.usage)?;
        } else if let Some(error) = &self.error {
            map.serialize_entry("error", error)?;
        }
        map.end()
    }
}

/// Lists all configured / authenticated providers (`GET /api/providers`).
pub async fn list_providers(state: &AppState) -> ApiResult<Vec<ProviderMetadata>> {
    Ok(state.pi.list_providers().await)
}

/// Returns editable provider configs with masked API keys
/// (`GET /api/providers/config`).
pub async fn get_provider_configs(
    state: &AppState,
) -> ApiResult<HashMap<String, EditableProviderConfig>> {
    Ok(state.pi.get_configs())
}

/// Saves the whole provider config map the web client sends
/// (`Record<providerId, ProviderConfig>`); providers missing from the map are
/// removed (`POST /api/providers/config`).
pub async fn save_provider_configs(
    state: &AppState,
    configs: &HashMap<String, SaveProviderConfigInput>,
) -> ApiResult<()> {
    state
        .pi
        .save_configs(configs)
        .map_err(|err| ApiError::bad_request(format!("Failed to save provider config: {err}")))
}

/// Fetches remote `{baseUrl}/models` models merged with the cached
/// `models-store.json` models (`POST /api/providers/fetch-models`). Never
/// fails: provider errors come back as `success: false` with an error message.
pub async fn fetch_models(
    state: &AppState,
    input: &FetchModelsInput,
) -> ApiResult<FetchModelsResult> {
    Ok(match state.pi.fetch_models(input).await {
        Ok(models) => FetchModelsResult {
            success: true,
            models: Some(models),
            error: None,
        },
        Err(err) => FetchModelsResult {
            success: false,
            models: None,
            error: Some(err.to_string()),
        },
    })
}

/// Sends a quick completion request through the provider's endpoint using the
/// resolved `auth.json` credential (`POST /api/providers/test`). Never fails:
/// provider errors come back as `success: false` with an error message.
pub async fn test_provider(
    state: &AppState,
    input: &TestProviderInput,
) -> ApiResult<TestProviderResult> {
    Ok(match state.pi.test_provider(input).await {
        Ok((content, usage)) => TestProviderResult {
            success: true,
            content: Some(content),
            usage,
            error: None,
        },
        Err(err) => TestProviderResult {
            success: false,
            content: None,
            usage: None,
            error: Some(err.to_string()),
        },
    })
}

/// Returns masked credential status from `~/.pi/agent/auth.json`
/// (`GET /api/pi/auth`).
pub async fn get_pi_auth(state: &AppState) -> ApiResult<Vec<serde_json::Value>> {
    Ok(state.pi.pi_auth_status())
}

/// Sets or deletes a provider credential (`POST /api/pi/auth`).
pub async fn set_pi_auth(state: &AppState, input: &SetPiAuthInput) -> ApiResult<()> {
    state
        .pi
        .set_pi_auth(input)
        .map_err(|err| ApiError::bad_request(format!("Failed to update pi auth: {err}")))
}
