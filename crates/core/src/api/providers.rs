//! Provider and native-`pi` auth facade.
//!
//! Thin wrappers over `PiProviderManager` (pure Rust handling of
//! `~/.pi/agent/auth.json`, `models.json`, `models-store.json` and Locagens
//! metadata files).

use crate::api::error::{ApiError, ApiResult};
use crate::pi_providers::{
    EditableProviderConfig, FetchModelsInput, LoginSession, OAuthFlow, OAuthProviderInfo, ProviderMetadata,
    SaveProviderConfigInput, SetPiAuthInput, TestProviderInput,
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

/// A started OAuth login; `flow` says what to show, then pass the whole value
/// to [`finish_oauth_login`].
///
/// - `device_code`: show `user_code` and `verification_uri`.
/// - `auth_url`: open `auth_url`; the redirect arrives on a loopback server,
///   or the user pastes it ([`submit_oauth_redirect`]).
///
/// `handle` is opaque (it carries private device codes and PKCE verifiers and
/// is redacted from `Debug`).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OAuthLoginStart {
    pub provider_id: String,
    pub flow: OAuthFlow,
    /// Device flow only.
    pub user_code: Option<String>,
    /// Device flow only: the URL to open (the "complete" URL when the server
    /// sent one).
    pub verification_uri: Option<String>,
    /// Authorization-URL flow only: the URL to open in the browser.
    pub auth_url: Option<String>,
    /// Seconds until the device code expires (0 for the authorization-URL flow).
    pub expires_in_secs: u64,
    /// Device flow poll interval (0 for the authorization-URL flow).
    pub interval_secs: u64,
    pub handle: LoginSession,
}

impl OAuthLoginStart {
    /// The URL the user should open, whichever the flow.
    pub fn url_to_open(&self) -> &str {
        self.auth_url.as_deref().or(self.verification_uri.as_deref()).unwrap_or_default()
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

/// Providers that support OAuth sign-in, with the flow each one uses.
pub fn oauth_providers() -> Vec<OAuthProviderInfo> {
    locagens_config::oauth::oauth_providers().to_vec()
}

/// Starts an OAuth login: requests a device code, or starts the loopback
/// receiver and builds the authorization URL.
pub async fn start_oauth_login(state: &AppState, provider_id: &str) -> ApiResult<OAuthLoginStart> {
    start_oauth_login_with_domain(state, provider_id, None).await
}

/// Like [`start_oauth_login`], for `github-copilot` on GitHub Enterprise:
/// `enterprise_domain` is the company's GitHub URL or domain.
pub async fn start_oauth_login_with_domain(
    state: &AppState,
    provider_id: &str,
    enterprise_domain: Option<&str>,
) -> ApiResult<OAuthLoginStart> {
    let provider_id = provider_id.trim();
    let Some(info) = locagens_config::oauth::oauth_provider_info(provider_id) else {
        return Err(ApiError::bad_request(format!(
            "Provider \"{provider_id}\" does not support OAuth login"
        )));
    };
    let session = state
        .pi
        .start_oauth_login(provider_id, enterprise_domain)
        .await
        .map_err(|err| ApiError::internal(format!("Failed to start OAuth login: {err}")))?;
    Ok(login_start(info.id, info.flow, session))
}

fn login_start(provider_id: &str, flow: OAuthFlow, session: LoginSession) -> OAuthLoginStart {
    let mut start = OAuthLoginStart {
        provider_id: provider_id.to_string(),
        flow,
        user_code: None,
        verification_uri: None,
        auth_url: None,
        expires_in_secs: 0,
        interval_secs: 0,
        handle: session.clone(),
    };
    match session {
        LoginSession::DeviceCode(device) => {
            start.user_code = Some(device.user_code);
            start.verification_uri = Some(device.verification_uri);
            start.expires_in_secs = device.expires_in_secs.ceil() as u64;
            start.interval_secs = device.interval_secs.map(|secs| secs.ceil() as u64).unwrap_or(5);
        }
        LoginSession::AuthUrl(auth) => start.auth_url = Some(auth.auth_url),
    }
    start
}

/// Waits until the user completes the login (device approval or the loopback
/// redirect) and stores the credential in `~/.locagens/auth.json`. Cancel by
/// dropping the future.
pub async fn finish_oauth_login(state: &AppState, start: &OAuthLoginStart) -> ApiResult<()> {
    state
        .pi
        .finish_oauth_login(&start.provider_id, &start.handle)
        .await
        .map_err(|err| ApiError::bad_request(format!("OAuth login failed: {err}")))
}

/// Manual fallback of an `auth_url` login: the user pasted the redirect URL
/// (or authorization code) from the browser. Completes the login, so drop a
/// pending [`finish_oauth_login`] future afterwards.
pub async fn submit_oauth_redirect(state: &AppState, start: &OAuthLoginStart, pasted_url: &str) -> ApiResult<()> {
    let LoginSession::AuthUrl(session) = &start.handle else {
        return Err(ApiError::bad_request("This sign-in does not take a pasted redirect"));
    };
    state
        .pi
        .submit_oauth_redirect(&start.provider_id, session, pasted_url)
        .await
        .map_err(|err| ApiError::bad_request(format!("OAuth login failed: {err}")))
}

/// Opens a verification URL in the default browser. Only https URLs are
/// opened, and the URL is handed to the OS opener directly (never through a
/// shell).
pub fn open_verification_url(url: &str) -> ApiResult<()> {
    let url = locagens_config::oauth::xai::validate_verification_uri(url)
        .map_err(|err| ApiError::bad_request(err.to_string()))?;
    #[cfg(windows)]
    let mut command = {
        let mut command = std::process::Command::new("rundll32");
        command.args(["url.dll,FileProtocolHandler", url.as_str()]);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(url.as_str());
        command
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url.as_str());
        command
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|err| ApiError::internal(format!("Could not open the browser: {err}")))
}
