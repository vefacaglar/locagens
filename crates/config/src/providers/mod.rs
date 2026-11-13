//! Pure Rust provider, auth & model engine (Codex / Locagens compatible).
//!
//! Reads and writes the primary agent config directory `~/.locagens`
//! (`$LOCAGENS_HOME`, module alias kept as "pi agent dir" for callers):
//!
//! - `auth.json` — map of provider id → credential
//!   (`{ "type": "api_key", "key": "..." }`, `!command`, `$ENV_VAR` or OAuth).
//! - `models.json` — custom provider configs & model overrides.
//! - `models-store.json` — cached dynamic model catalog.
//!
//! Read-only credential fallbacks: `~/.codex` (`$CODEX_HOME`: `auth.json`
//! `OPENAI_API_KEY` and `config.toml` `[model_providers.*]`), the legacy
//! `~/.pi/agent` directory and OpenCode's `auth.json` (API keys only, lowest
//! precedence), so existing credentials keep working.
//!
//! Plus Locagens UI metadata under the app-support dir:
//! `locagens-providers-meta.json` (display names / model settings).

use crate::oauth::auth_lock::AuthFileLock;
use crate::oauth::callback_server::{CallbackParams, CallbackServer};
use crate::oauth::catalog::{self, CatalogModel};
use crate::oauth::xai::XaiOAuth;
use crate::oauth::{self, AuthUrlSession, LoginSession, OAuthClients, OAuthTokens};
use crate::paths;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

mod auth_store;
mod model_catalog;
mod model_fetch;

pub use model_catalog::model_reasoning_efforts;

#[cfg(test)]
mod credential_hygiene_tests;
#[cfg(test)]
mod tests;


/// Sentinel sent by the UI so existing API keys are never overwritten.
pub const PRESERVE_API_KEY: &str = "__LOCAGENS_PRESERVE_API_KEY__";

/// Environment-variable fallback chain for API keys.
const ENV_FALLBACKS: [&str; 13] = [
    "OPENCODE_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "DEEPSEEK_API_KEY",
    "OPENROUTER_API_KEY",
    "GROQ_API_KEY",
    "XAI_API_KEY",
    "MISTRAL_API_KEY",
    "MINIMAX_API_KEY",
    "MOONSHOT_API_KEY",
    "KIMI_API_KEY",
    "ZAI_API_KEY",
];

/// One credential entry of `~/.pi/agent/auth.json`.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "snake_case", default)]
pub struct AuthCredential {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `Debug` text for an optional secret: never the value.
fn redacted(secret: &Option<String>) -> &'static str {
    if secret.is_some() { "Some(<redacted>)" } else { "None" }
}

impl std::fmt::Debug for AuthCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthCredential")
            .field("kind", &self.kind)
            .field("key", &format_args!("{}", redacted(&self.key)))
            .field("extra_keys", &self.extra.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl std::fmt::Debug for SaveProviderConfigInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SaveProviderConfigInput")
            .field("provider_type", &self.provider_type)
            .field("base_url", &self.base_url)
            .field("api_key", &format_args!("{}", redacted(&self.api_key)))
            .field("models", &self.models)
            .field("display_name", &self.display_name)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for FetchModelsInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchModelsInput")
            .field("provider_type", &self.provider_type)
            .field("base_url", &self.base_url)
            .field("api_key", &format_args!("{}", redacted(&self.api_key)))
            .field("provider_id", &self.provider_id)
            .finish()
    }
}

impl std::fmt::Debug for TestProviderInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestProviderInput")
            .field("provider_id", &self.provider_id)
            .field("base_url", &self.base_url)
            .field("api_key", &format_args!("{}", redacted(&self.api_key)))
            .field("model", &self.model)
            .finish()
    }
}

impl std::fmt::Debug for SetPiAuthInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetPiAuthInput")
            .field("provider_id", &self.provider_id)
            .field("action", &self.action)
            .field("api_key", &format_args!("{}", redacted(&self.api_key)))
            .finish()
    }
}

impl AuthCredential {
    pub fn api_key(key: &str) -> Self {
        Self {
            kind: "api_key".to_string(),
            key: Some(key.to_string()),
            extra: Map::new(),
        }
    }
}

/// A model entry merged from built-ins, `models-store.json` and `models.json`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_efforts: Option<Vec<String>>,
}

/// Metadata returned by `list_providers` (UI contract: `type` is
/// `"openai-compatible"` or `"anthropic"`, `models` are plain model-id
/// strings, API keys are never included).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMetadata {
    pub id: String,
    pub display_name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub models: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_settings: Option<Value>,
}

/// Editable provider config returned by `get_configs` —
/// API keys are always masked, never exposed raw.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EditableProviderConfig {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub base_url: String,
    pub api: String,
    pub api_key: String,
    pub has_api_key: bool,
    pub models: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_settings: Option<Value>,
}

/// One entry of the map passed to `save_configs`
/// (`{ [providerId]: { type, displayName, baseUrl, apiKey, models, modelSettings } }`).
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SaveProviderConfigInput {
    #[serde(rename = "type", alias = "api")]
    pub provider_type: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub models: Option<Vec<String>>,
    pub display_name: Option<String>,
    pub model_settings: Option<Value>,
}

/// Input for `fetch_models` — `providerId` is optional;
/// when absent the `baseUrl`/`apiKey`/`type` from the body are used directly.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct FetchModelsInput {
    #[serde(rename = "type", alias = "api")]
    pub provider_type: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub provider_id: Option<String>,
}

/// Input for `test_provider`.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct TestProviderInput {
    pub provider_id: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
}

/// Input for `set_pi_auth`.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SetPiAuthInput {
    pub provider_id: String,
    pub action: String,
    pub api_key: Option<String>,
}

/// How a provider is currently authenticated (never carries secret values).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LoginStatus {
    OAuth,
    ApiKey,
    /// Name of the environment variable that supplies the key.
    Env(String),
    SignedOut,
}

/// One row of the `lcgn auth` provider table.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LoginProvider {
    pub id: String,
    pub name: String,
    /// `Some(label)` when OAuth sign-in is supported.
    pub oauth_label: Option<String>,
    /// How the OAuth sign-in is completed (device code or authorization URL).
    pub oauth_flow: Option<oauth::OAuthFlow>,
    /// Built-in API-key environment variable; `None` for keyless providers.
    pub api_key_env: Option<String>,
    pub status: LoginStatus,
}

pub struct PiProviderManager {
    /// Primary agent config directory (`~/.locagens`). Writes go here.
    primary_dir: PathBuf,
    /// Read-only credential/config fallbacks: `~/.codex`, `~/.pi/agent`.
    fallback_dirs: Vec<PathBuf>,
    /// OpenCode's data directory (`auth.json` with `type: "api"` keys). Read
    /// only, and below every other source in precedence.
    opencode_dir: Option<PathBuf>,
    app_support_dir: PathBuf,
    http: reqwest::Client,
    /// Cache of `!command` stdout results, keyed by command string.
    command_cache: Mutex<HashMap<String, String>>,
    oauth: OAuthClients,
    /// Loopback receivers of started authorization-URL logins, by provider.
    pending_logins: Mutex<HashMap<String, Arc<CallbackServer>>>,
}

impl PiProviderManager {
    pub fn new() -> Self {
        Self::with_chain(
            paths::locagens_home(),
            paths::app_support_dir(),
            vec![paths::codex_home(), paths::pi_agent_dir()],
        )
        .with_opencode_dir(paths::opencode_data_dir())
    }

    /// Single-directory constructor (tests, CLI overrides).
    pub fn with_dirs(pi_agent_dir: PathBuf, app_support_dir: PathBuf) -> Self {
        Self::with_chain(pi_agent_dir, app_support_dir, Vec::new())
    }

    /// Primary directory plus read-only fallback chain
    /// (`~/.locagens` → `~/.codex` → `~/.pi/agent` in production).
    pub fn with_chain(primary_dir: PathBuf, app_support_dir: PathBuf, fallback_dirs: Vec<PathBuf>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self {
            primary_dir,
            fallback_dirs,
            opencode_dir: None,
            app_support_dir,
            http,
            command_cache: Mutex::new(HashMap::new()),
            oauth: OAuthClients::default(),
            pending_logins: Mutex::new(HashMap::new()),
        }
    }

    /// Adds OpenCode's data directory as the lowest-precedence read-only
    /// credential source.
    pub fn with_opencode_dir(mut self, dir: PathBuf) -> Self {
        self.opencode_dir = Some(dir);
        self
    }

    /// Replaces the xAI OAuth endpoints (tests point them at a mock server).
    pub fn with_xai_oauth(mut self, xai_oauth: XaiOAuth) -> Self {
        self.oauth.xai = xai_oauth;
        self
    }

    /// Replaces every OAuth client (tests point them at mock servers).
    pub fn with_oauth_clients(mut self, oauth: OAuthClients) -> Self {
        self.oauth = oauth;
        self
    }

    fn auth_path(&self) -> PathBuf {
        self.primary_dir.join("auth.json")
    }

    fn models_path(&self) -> PathBuf {
        self.primary_dir.join("models.json")
    }

    fn models_store_path(&self) -> PathBuf {
        self.primary_dir.join("models-store.json")
    }

    fn meta_path(&self) -> PathBuf {
        self.app_support_dir.join("locagens-providers-meta.json")
    }
}

impl Default for PiProviderManager {
    fn default() -> Self {
        Self::new()
    }
}
