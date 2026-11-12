//! Pure Rust `pi` provider, auth & model engine.
//!
//! Reads and writes `pi`'s configuration files under the `pi` agent directory
//! (`~/.pi/agent`, overridable via `PI_CODING_AGENT_DIR`):
//!
//! - `auth.json` — map of provider id → credential
//!   (`{ "type": "api_key", "key": "..." }`, `!command`, `$ENV_VAR` or OAuth).
//! - `models.json` — custom provider configs & model overrides.
//! - `models-store.json` — `pi`'s cached dynamic model catalog.
//!
//! Plus Locagens UI metadata under the app-support dir:
//! `locagens-providers-meta.json` (display names / model settings) and
//! `locagens-presets.json` (architect / coder / utility agent presets).

use crate::paths;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "snake_case", default)]
pub struct AuthCredential {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
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
#[derive(Serialize, Clone, Debug)]
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
}

/// Metadata returned by `GET /api/providers`.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMetadata {
    pub id: String,
    pub display_name: String,
    pub base_url: String,
    pub api: String,
    pub models: Vec<ModelInfo>,
    pub has_api_key: bool,
}

/// Editable provider config returned by `GET /api/providers/config` —
/// API keys are always masked, never exposed raw.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EditableProviderConfig {
    pub id: String,
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

/// Body of `POST /api/providers/config`.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SaveProviderConfigInput {
    pub provider_id: String,
    pub base_url: Option<String>,
    pub api: Option<String>,
    pub api_key: Option<String>,
    pub models: Option<Vec<String>>,
    pub display_name: Option<String>,
    pub model_settings: Option<Value>,
}

/// Body of `POST /api/providers/fetch-models`.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct FetchModelsInput {
    pub provider_id: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
}

/// Body of `POST /api/providers/test`.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct TestProviderInput {
    pub provider_id: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
}

/// Body of `POST /api/pi/auth`.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SetPiAuthInput {
    pub provider_id: String,
    pub action: String,
    pub api_key: Option<String>,
}

/// One agent preset (architect / coder / utility).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentPreset {
    pub provider_id: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
}

struct BuiltinProvider {
    id: &'static str,
    name: &'static str,
    base_url: &'static str,
    api: &'static str,
    models: &'static [&'static str],
    env_var: &'static str,
}

/// Built-in default provider definitions (used when not overridden in
/// `models.json` or `models-store.json`).
const BUILTINS: &[BuiltinProvider] = &[
    BuiltinProvider {
        id: "opencode-go",
        name: "OpenCode Go",
        base_url: "https://opencode.ai/zen/go/v1",
        api: "openai-compatible",
        models: &[
            "glm-5.3-flash",
            "glm-5.3",
            "gemini-4-argon",
            "claude-haiku-5-5",
            "deepseek-v4.1-flash",
            "kimi-k3.1-pro",
            "minimax-m3.1-flash",
            "qwen3.8-flash",
            "gpt-5.6-luna",
            "gpt-6-luna",
        ],
        env_var: "OPENCODE_API_KEY",
    },
    BuiltinProvider {
        id: "anthropic",
        name: "Anthropic",
        base_url: "https://api.anthropic.com/v1",
        api: "anthropic",
        models: &["claude-sonnet-4-5", "claude-opus-4-5", "claude-haiku-4-5"],
        env_var: "ANTHROPIC_API_KEY",
    },
    BuiltinProvider {
        id: "openai",
        name: "OpenAI",
        base_url: "https://api.openai.com/v1",
        api: "openai-compatible",
        models: &["gpt-5", "gpt-4.1", "o4-mini"],
        env_var: "OPENAI_API_KEY",
    },
    BuiltinProvider {
        id: "google",
        name: "Google",
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        api: "openai-compatible",
        models: &["gemini-2.5-pro", "gemini-2.5-flash"],
        env_var: "GEMINI_API_KEY",
    },
    BuiltinProvider {
        id: "openrouter",
        name: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        api: "openai-compatible",
        models: &[],
        env_var: "OPENROUTER_API_KEY",
    },
    BuiltinProvider {
        id: "deepseek",
        name: "DeepSeek",
        base_url: "https://api.deepseek.com/v1",
        api: "openai-compatible",
        models: &["deepseek-chat", "deepseek-reasoner"],
        env_var: "DEEPSEEK_API_KEY",
    },
    BuiltinProvider {
        id: "groq",
        name: "Groq",
        base_url: "https://api.groq.com/openai/v1",
        api: "openai-compatible",
        models: &[],
        env_var: "GROQ_API_KEY",
    },
    BuiltinProvider {
        id: "xai",
        name: "xAI",
        base_url: "https://api.x.ai/v1",
        api: "openai-compatible",
        models: &[],
        env_var: "XAI_API_KEY",
    },
    BuiltinProvider {
        id: "ollama",
        name: "Ollama",
        base_url: "http://localhost:11434/v1",
        api: "openai-compatible",
        models: &[],
        env_var: "",
    },
];

fn builtin(id: &str) -> Option<&'static BuiltinProvider> {
    BUILTINS.iter().find(|provider| provider.id == id)
}

/// True when a provider talks the Anthropic Messages API.
fn is_anthropic(provider_id: &str, api: &str, base_url: &str) -> bool {
    api == "anthropic"
        || api == "anthropic-messages"
        || provider_id == "anthropic"
        || base_url.contains("anthropic")
}

pub struct PiProviderManager {
    pi_agent_dir: PathBuf,
    app_support_dir: PathBuf,
    http: reqwest::Client,
    /// Cache of `!command` stdout results, keyed by command string.
    command_cache: Mutex<HashMap<String, String>>,
}

impl PiProviderManager {
    pub fn new() -> Self {
        Self::with_dirs(paths::pi_agent_dir(), paths::app_support_dir())
    }

    pub fn with_dirs(pi_agent_dir: PathBuf, app_support_dir: PathBuf) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self {
            pi_agent_dir,
            app_support_dir,
            http,
            command_cache: Mutex::new(HashMap::new()),
        }
    }

    // ------------------------------------------------------------------
    // File paths
    // ------------------------------------------------------------------

    fn auth_path(&self) -> PathBuf {
        self.pi_agent_dir.join("auth.json")
    }

    fn models_path(&self) -> PathBuf {
        self.pi_agent_dir.join("models.json")
    }

    fn models_store_path(&self) -> PathBuf {
        self.pi_agent_dir.join("models-store.json")
    }

    fn meta_path(&self) -> PathBuf {
        self.app_support_dir.join("locagens-providers-meta.json")
    }

    fn presets_path(&self) -> PathBuf {
        self.app_support_dir.join("locagens-presets.json")
    }

    // ------------------------------------------------------------------
    // Raw file helpers (atomic JSON writes)
    // ------------------------------------------------------------------

    fn read_json(path: &Path) -> Value {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or(Value::Null)
    }

    fn write_json(path: &Path, value: &Value) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(value)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    fn read_auth_map(&self) -> Map<String, Value> {
        match Self::read_json(&self.auth_path()) {
            Value::Object(map) => map,
            _ => Map::new(),
        }
    }

    fn write_auth_map(&self, map: Map<String, Value>) -> Result<()> {
        Self::write_json(&self.auth_path(), &Value::Object(map))
    }

    fn read_models_json(&self) -> Value {
        Self::read_json(&self.models_path())
    }

    /// `models.json` entry for a provider (`providers.<id>`), if any.
    fn models_json_provider(&self, provider_id: &str) -> Option<Map<String, Value>> {
        let models = self.read_models_json();
        models
            .get("providers")?
            .get(provider_id)?
            .as_object()
            .cloned()
    }

    // ------------------------------------------------------------------
    // Credential resolution
    // ------------------------------------------------------------------

    /// Resolves the API key for a provider: explicit override first, then
    /// `auth.json` (literal, `!command` stdout, or `$ENV_VAR`), then the
    /// environment-variable fallback chain.
    pub async fn resolve_api_key(
        &self,
        provider_id: &str,
        override_key: Option<&str>,
    ) -> Option<String> {
        if let Some(key) = override_key.map(str::trim).filter(|k| !k.is_empty()) {
            let resolved = self.expand_key_value(key, provider_id).await;
            if !resolved.is_empty() {
                return Some(resolved);
            }
        }

        let auth = self.read_auth_map();
        if let Some(credential) = auth.get(provider_id) {
            let kind = credential
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if kind == "api_key" || kind.is_empty() {
                if let Some(key) = credential
                    .get("key")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|k| !k.is_empty())
                {
                    let resolved = self.expand_key_value(key, provider_id).await;
                    if !resolved.is_empty() {
                        return Some(resolved);
                    }
                }
            }
        }

        // Environment fallback: provider's preferred variable first, then the
        // shared chain.
        let mut candidates: Vec<&str> = Vec::new();
        if let Some(builtin) = builtin(provider_id) {
            if !builtin.env_var.is_empty() {
                candidates.push(builtin.env_var);
            }
        }
        for var in ENV_FALLBACKS {
            if !candidates.contains(&var) {
                candidates.push(var);
            }
        }
        candidates
            .into_iter()
            .find_map(|var| std::env::var(var).ok().filter(|v| !v.trim().is_empty()))
            .map(|value| value.trim().to_string())
    }

    /// Expands `!command` (shell stdout, cached) and `$ENV_VAR` forms.
    async fn expand_key_value(&self, raw: &str, provider_id: &str) -> String {
        let raw = raw.trim();
        if let Some(command) = raw.strip_prefix('!') {
            let command = command.trim();
            if let Some(cached) = self.command_cache.lock().expect("cache poisoned").get(command) {
                return cached.clone();
            }
            match self.run_key_command(command).await {
                Ok(stdout) => {
                    self.command_cache
                        .lock()
                        .expect("cache poisoned")
                        .insert(command.to_string(), stdout.clone());
                    stdout
                }
                Err(err) => {
                    tracing::error!(
                        "[Providers] `!command` credential for {provider_id} failed: {err}"
                    );
                    String::new()
                }
            }
        } else if let Some(var) = raw.strip_prefix('$') {
            std::env::var(var.trim())
                .ok()
                .filter(|v| !v.trim().is_empty())
                .map(|v| v.trim().to_string())
                .unwrap_or_default()
        } else {
            raw.to_string()
        }
    }

    async fn run_key_command(&self, command: &str) -> Result<String> {
        #[cfg(windows)]
        let mut cmd = {
            let mut c = tokio::process::Command::new("cmd");
            c.args(["/C", command]);
            c
        };
        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", command]);
            c
        };
        cmd.stdin(std::process::Stdio::null());
        let output = cmd.output().await?;
        if !output.status.success() {
            anyhow::bail!(
                "credential command exited with {}",
                output.status
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub fn has_api_key(&self, provider_id: &str) -> bool {
        self.read_auth_map().contains_key(provider_id)
    }

    // ------------------------------------------------------------------
    // Catalog assembly
    // ------------------------------------------------------------------

    /// Models for a provider cached in `pi`'s `models-store.json`.
    fn models_store_models(&self, provider_id: &str) -> Vec<ModelInfo> {
        let store = Self::read_json(&self.models_store_path());
        store
            .get(provider_id)
            .and_then(|provider| provider.get("models"))
            .and_then(Value::as_array)
            .map(|models| {
                models
                    .iter()
                    .filter_map(|model| {
                        let id = model.get("id")?.as_str()?.to_string();
                        Some(ModelInfo {
                            name: model
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or(&id)
                                .to_string(),
                            api: model
                                .get("api")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            base_url: model
                                .get("baseUrl")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            context_window: model
                                .get("contextWindow")
                                .and_then(Value::as_i64),
                            max_tokens: model.get("maxTokens").and_then(Value::as_i64),
                            reasoning: model.get("reasoning").and_then(Value::as_bool),
                            cost: model.get("cost").cloned(),
                            id,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Models declared in a `models.json` provider entry.
    fn models_json_models(entry: Option<&Map<String, Value>>) -> Vec<ModelInfo> {
        entry
            .and_then(|entry| entry.get("models"))
            .and_then(Value::as_array)
            .map(|models| {
                models
                    .iter()
                    .filter_map(|model| match model {
                        Value::String(id) => Some(ModelInfo {
                            id: id.clone(),
                            name: id.clone(),
                            api: None,
                            base_url: None,
                            context_window: None,
                            max_tokens: None,
                            reasoning: None,
                            cost: None,
                        }),
                        Value::Object(object) => {
                            let id = object.get("id")?.as_str()?.to_string();
                            Some(ModelInfo {
                                name: object
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or(&id)
                                    .to_string(),
                                api: object.get("api").and_then(Value::as_str).map(str::to_string),
                                base_url: object
                                    .get("baseUrl")
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                context_window: object
                                    .get("contextWindow")
                                    .and_then(Value::as_i64),
                                max_tokens: object.get("maxTokens").and_then(Value::as_i64),
                                reasoning: object.get("reasoning").and_then(Value::as_bool),
                                cost: object.get("cost").cloned(),
                                id,
                            })
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Provider ids to expose: built-ins plus anything present in `auth.json`
    /// or `models.json`.
    fn provider_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = BUILTINS.iter().map(|p| p.id.to_string()).collect();
        for key in self.read_auth_map().keys() {
            if !ids.contains(key) {
                ids.push(key.clone());
            }
        }
        if let Value::Object(providers) = self.read_models_json().get("providers").cloned().unwrap_or(Value::Null) {
            for key in providers.keys() {
                if !ids.contains(key) {
                    ids.push(key.clone());
                }
            }
        }
        ids
    }

    /// Resolved display name / model settings from
    /// `locagens-providers-meta.json`.
    fn meta_entry(&self, provider_id: &str) -> Option<Map<String, Value>> {
        let meta = Self::read_json(&self.meta_path());
        meta.get("providers")?
            .get(provider_id)?
            .as_object()
            .cloned()
    }

    /// Base URL for a provider: `models.json` override → built-in →
    /// first `models-store.json` model baseUrl.
    fn resolve_base_url(&self, provider_id: &str) -> String {
        if let Some(url) = self
            .models_json_provider(provider_id)
            .as_ref()
            .and_then(|entry| entry.get("baseUrl"))
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
        {
            return url.to_string();
        }
        if let Some(builtin) = builtin(provider_id) {
            return builtin.base_url.to_string();
        }
        self.models_store_models(provider_id)
            .into_iter()
            .find_map(|model| model.base_url)
            .unwrap_or_default()
    }

    /// API kind for a provider: `models.json` override → built-in → inferred
    /// from `models-store.json` models → `openai-compatible`.
    fn resolve_api(&self, provider_id: &str) -> String {
        if let Some(api) = self
            .models_json_provider(provider_id)
            .as_ref()
            .and_then(|entry| entry.get("api"))
            .and_then(Value::as_str)
            .filter(|api| !api.is_empty())
        {
            return api.to_string();
        }
        if let Some(builtin) = builtin(provider_id) {
            return builtin.api.to_string();
        }
        if let Some(api) = self
            .models_store_models(provider_id)
            .into_iter()
            .find_map(|model| model.api)
        {
            return api;
        }
        "openai-compatible".to_string()
    }

    /// Merged model catalog: built-ins ← `models-store.json` ← `models.json`.
    fn merged_models(&self, provider_id: &str) -> Vec<ModelInfo> {
        let mut models: Vec<ModelInfo> = builtin(provider_id)
            .map(|provider| {
                provider
                    .models
                    .iter()
                    .map(|id| ModelInfo {
                        id: id.to_string(),
                        name: id.to_string(),
                        api: None,
                        base_url: None,
                        context_window: None,
                        max_tokens: None,
                        reasoning: None,
                        cost: None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        for store_model in self.models_store_models(provider_id) {
            if let Some(existing) = models.iter_mut().find(|m| m.id == store_model.id) {
                *existing = store_model;
            } else {
                models.push(store_model);
            }
        }

        let entry = self.models_json_provider(provider_id);
        for model in Self::models_json_models(entry.as_ref()) {
            if let Some(existing) = models.iter_mut().find(|m| m.id == model.id) {
                existing.name = model.name;
                if model.api.is_some() {
                    existing.api = model.api;
                }
                if model.base_url.is_some() {
                    existing.base_url = model.base_url;
                }
                existing.context_window = model.context_window;
                existing.max_tokens = model.max_tokens;
                existing.reasoning = model.reasoning;
                existing.cost = model.cost;
            } else {
                models.push(model);
            }
        }

        models
    }

    /// All configured / authenticated providers for `GET /api/providers`.
    pub async fn list_providers(&self) -> Vec<ProviderMetadata> {
        let auth = self.read_auth_map();
        self.provider_ids()
            .into_iter()
            .map(|id| {
                let display_name = self
                    .meta_entry(&id)
                    .as_ref()
                    .and_then(|entry| entry.get("displayName"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        builtin(&id)
                            .map(|p| p.name.to_string())
                            .unwrap_or_else(|| id.clone())
                    });
                ProviderMetadata {
                    has_api_key: auth.contains_key(&id),
                    models: self.merged_models(&id),
                    base_url: self.resolve_base_url(&id),
                    api: self.resolve_api(&id),
                    display_name,
                    id,
                }
            })
            .collect()
    }

    /// `GET /api/providers/config` — editable configs with masked keys.
    pub fn get_configs(&self) -> HashMap<String, EditableProviderConfig> {
        let mut configs = HashMap::new();
        for id in self.provider_ids() {
            let models: Vec<String> = self
                .merged_models(&id)
                .into_iter()
                .map(|model| model.id)
                .collect();
            let meta = self.meta_entry(&id);
            let has_api_key = self.has_api_key(&id);
            configs.insert(
                id.clone(),
                EditableProviderConfig {
                    id: id.clone(),
                    base_url: self.resolve_base_url(&id),
                    api: self.resolve_api(&id),
                    api_key: if has_api_key {
                        PRESERVE_API_KEY.to_string()
                    } else {
                        String::new()
                    },
                    has_api_key,
                    models,
                    display_name: meta
                        .as_ref()
                        .and_then(|m| m.get("displayName"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    model_settings: meta
                        .as_ref()
                        .and_then(|m| m.get("modelSettings"))
                        .cloned(),
                },
            );
        }
        configs
    }

    /// `POST /api/providers/config` — saves API keys to `auth.json`, custom
    /// `baseUrl`/`models` to `models.json`, and UI metadata to
    /// `locagens-providers-meta.json`.
    pub fn save_config(&self, input: &SaveProviderConfigInput) -> Result<()> {
        if input.provider_id.trim().is_empty() {
            anyhow::bail!("providerId is required");
        }

        // 1. API key → auth.json (preserve existing keys when masked).
        if let Some(api_key) = input.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
            if api_key != PRESERVE_API_KEY {
                let mut auth = self.read_auth_map();
                auth.insert(input.provider_id.clone(), serde_json::to_value(AuthCredential::api_key(api_key))?);
                self.write_auth_map(auth)?;
            }
        }

        // 2. baseUrl / models / api → models.json.
        let mut models_doc = self.read_models_json();
        if !models_doc.is_object() {
            models_doc = json!({});
        }
        let needs_provider_entry = input.base_url.is_some() || input.models.is_some() || input.api.is_some();
        if needs_provider_entry {
            let object = models_doc.as_object_mut().unwrap();
            let providers = object
                .entry("providers")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .unwrap();
            let entry = providers
                .entry(input.provider_id.clone())
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .unwrap();
            match input.base_url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
                Some(url) => {
                    entry.insert("baseUrl".into(), json!(url));
                }
                None => {
                    entry.remove("baseUrl");
                }
            }
            match input.api.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
                Some(api) => {
                    entry.insert("api".into(), json!(api));
                }
                None => {
                    entry.remove("api");
                }
            }
            match input.models.as_ref() {
                Some(models) if !models.is_empty() => {
                    entry.insert(
                        "models".into(),
                        Value::Array(
                            models
                                .iter()
                                .map(|id| json!({ "id": id.trim(), "name": id.trim() }))
                                .collect(),
                        ),
                    );
                }
                _ => {
                    entry.remove("models");
                }
            }
            Self::write_json(&self.models_path(), &models_doc)?;
        }

        // 3. displayName / modelSettings → locagens-providers-meta.json.
        if input.display_name.is_some() || input.model_settings.is_some() {
            let mut meta = Self::read_json(&self.meta_path());
            if !meta.is_object() {
                meta = json!({});
            }
            let object = meta.as_object_mut().unwrap();
            let providers = object
                .entry("providers")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .unwrap();
            let entry = providers
                .entry(input.provider_id.clone())
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .unwrap();
            if let Some(name) = input.display_name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                entry.insert("displayName".into(), json!(name));
            }
            if let Some(settings) = input.model_settings.clone() {
                entry.insert("modelSettings".into(), settings);
            }
            Self::write_json(&self.meta_path(), &meta)?;
        }

        Ok(())
    }

    // ------------------------------------------------------------------
    // Remote model fetching
    // ------------------------------------------------------------------

    /// `POST /api/providers/fetch-models` — queries `{baseUrl}/models` with
    /// the provider's resolved key and merges in `models-store.json` models.
    pub async fn fetch_models(&self, input: &FetchModelsInput) -> Result<Vec<ModelInfo>> {
        let base_url = input
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.resolve_base_url(&input.provider_id));
        if base_url.is_empty() {
            anyhow::bail!("No baseUrl configured for provider \"{}\"", input.provider_id);
        }
        let base_url = base_url.trim_end_matches('/');
        let api = self.resolve_api(&input.provider_id);

        let mut models: Vec<ModelInfo> = self.models_store_models(&input.provider_id);

        if !is_anthropic(&input.provider_id, &api, &base_url) {
            let api_key = self
                .resolve_api_key(&input.provider_id, input.api_key.as_deref())
                .await;
            let mut request = self.http.get(format!("{base_url}/models"));
            if let Some(key) = api_key.as_ref().filter(|k| !k.is_empty()) {
                request = request.bearer_auth(key);
            }
            let response = request.send().await?;
            if !response.status().is_success() {
                anyhow::bail!(
                    "Model fetch failed with status {}",
                    response.status()
                );
            }
            let body: Value = response.json().await.unwrap_or(Value::Null);
            let entries = match &body {
                Value::Array(array) => array.clone(),
                Value::Object(_) => body
                    .get("data")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            for entry in entries {
                let Some(id) = entry.get("id").and_then(Value::as_str) else {
                    continue;
                };
                if models.iter().any(|m| m.id == id) {
                    continue;
                }
                models.push(ModelInfo {
                    id: id.to_string(),
                    name: entry
                        .get("name")
                        .or_else(|| entry.get("display_name"))
                        .and_then(Value::as_str)
                        .unwrap_or(id)
                        .to_string(),
                    api: None,
                    base_url: None,
                    context_window: entry
                        .get("context_window")
                        .and_then(Value::as_i64),
                    max_tokens: entry.get("max_tokens").and_then(Value::as_i64),
                    reasoning: None,
                    cost: None,
                });
            }
        }

        Ok(models)
    }

    // ------------------------------------------------------------------
    // Connectivity test
    // ------------------------------------------------------------------

    /// `POST /api/providers/test` — sends a tiny completion request and
    /// returns the first token of content plus usage.
    pub async fn test_provider(
        &self,
        input: &TestProviderInput,
    ) -> Result<(String, Option<Value>)> {
        let base_url = input
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.resolve_base_url(&input.provider_id));
        if base_url.is_empty() {
            anyhow::bail!("No baseUrl configured for provider \"{}\"", input.provider_id);
        }
        let base_url = base_url.trim_end_matches('/');
        let api = self.resolve_api(&input.provider_id);
        let api_key = self
            .resolve_api_key(&input.provider_id, input.api_key.as_deref())
            .await
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No API key available for provider \"{}\"",
                    input.provider_id
                )
            })?;

        let model = input
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                self.merged_models(&input.provider_id)
                    .first()
                    .map(|model| model.id.clone())
                    .unwrap_or_else(|| "default".to_string())
            });

        let (content, usage) = if is_anthropic(&input.provider_id, &api, &base_url) {
                let response = self
                    .http
                    .post(format!("{base_url}/messages"))
                    .header("x-api-key", &api_key)
                    .header("anthropic-version", "2023-06-01")
                    .json(&json!({
                        "model": model,
                        "max_tokens": 32,
                        "messages": [{ "role": "user", "content": "Reply with the single word: pong" }]
                    }))
                    .send()
                    .await?;
                let status = response.status();
                let body: Value = response.json().await.unwrap_or(Value::Null);
                if !status.is_success() {
                    anyhow::bail!("Test failed with status {status}: {}", summarize_error(&body));
                }
                let content = body
                    .get("content")
                    .and_then(Value::as_array)
                    .and_then(|blocks| blocks.first())
                    .and_then(|block| block.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let usage = body.get("usage").cloned();
                (content, usage)
            } else {
                let mut request = self
                    .http
                    .post(format!("{base_url}/chat/completions"))
                    .bearer_auth(&api_key)
                    .json(&json!({
                        "model": model,
                        "max_tokens": 32,
                        "messages": [{ "role": "user", "content": "Reply with the single word: pong" }]
                    }));
                // OpenRouter etiquette.
                if base_url.contains("openrouter") {
                    request = request.header("X-Title", "Locagens");
                }
                let response = request.send().await?;
                let status = response.status();
                let body: Value = response.json().await.unwrap_or(Value::Null);
                if !status.is_success() {
                    anyhow::bail!("Test failed with status {status}: {}", summarize_error(&body));
                }
                let content = body
                    .pointer("/choices/0/message/content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let usage = body.get("usage").cloned();
                (content, usage)
            };

        Ok((content, usage))
    }

    // ------------------------------------------------------------------
    // Native `pi` auth endpoints
    // ------------------------------------------------------------------

    /// `GET /api/pi/auth` — masked credential status for each provider in
    /// `auth.json`.
    pub fn pi_auth_status(&self) -> Vec<Value> {
        self.read_auth_map()
            .iter()
            .map(|(provider_id, credential)| {
                let kind = credential
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                let key_preview = credential
                    .get("key")
                    .and_then(Value::as_str)
                    .map(|key| {
                        let key = key.trim();
                        if key.starts_with('!') {
                            "command".to_string()
                        } else if key.starts_with('$') {
                            "environment".to_string()
                        } else if key.len() <= 8 {
                            "***".to_string()
                        } else {
                            format!("{}***{}", &key[..4], &key[key.len() - 4..])
                        }
                    });
                json!({
                    "providerId": provider_id,
                    "type": kind,
                    "hasCredential": true,
                    "keyPreview": key_preview,
                })
            })
            .collect()
    }

    /// `POST /api/pi/auth` — set or delete a provider credential in
    /// `auth.json`.
    pub fn set_pi_auth(&self, input: &SetPiAuthInput) -> Result<()> {
        if input.provider_id.trim().is_empty() {
            anyhow::bail!("providerId is required");
        }
        let mut auth = self.read_auth_map();
        match input.action.as_str() {
            "delete" | "remove" => {
                if auth.remove(input.provider_id.trim()).is_none() {
                    anyhow::bail!("No credential for provider \"{}\"", input.provider_id);
                }
            }
            "set" | "save" => {
                let key = input
                    .api_key
                    .as_deref()
                    .map(str::trim)
                    .filter(|k| !k.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("apiKey is required to set a credential"))?;
                auth.insert(
                    input.provider_id.trim().to_string(),
                    serde_json::to_value(AuthCredential::api_key(key))?,
                );
            }
            _ => anyhow::bail!(
                "Invalid action. Must be 'set' or 'delete'"
            ),
        }
        self.write_auth_map(auth)
    }

    // ------------------------------------------------------------------
    // Agent presets
    // ------------------------------------------------------------------

    /// Default presets, used when `locagens-presets.json` is missing or
    /// incomplete.
    fn default_presets() -> Map<String, Value> {
        let preset = |model: &str| {
            json!({
                "providerId": "opencode-go",
                "model": model,
                "reasoningEffort": Value::Null,
            })
        };
        Map::from_iter([
            ("architect".to_string(), preset("glm-5.3-flash")),
            ("coder".to_string(), preset("glm-5.3-flash")),
            ("utility".to_string(), preset("glm-5.3-flash")),
        ])
    }

    /// `GET /api/agent-presets` — presets from `locagens-presets.json`
    /// overlaid on defaults.
    pub fn get_presets(&self) -> Value {
        let mut presets = Self::default_presets();
        if let Value::Object(stored) = Self::read_json(&self.presets_path()) {
            for (name, value) in stored {
                if presets.contains_key(&name) {
                    presets.insert(name, value);
                }
            }
        }
        Value::Object(presets)
    }

    /// `PUT /api/agent-presets` — merges the submitted presets into
    /// `locagens-presets.json`.
    pub fn save_presets(&self, body: &Value) -> Result<Value> {
        let Value::Object(submitted) = body else {
            anyhow::bail!("Expected an object of preset name → AgentPreset");
        };
        let mut presets = Self::default_presets();
        for (name, value) in submitted.iter() {
            if !presets.contains_key(name) {
                anyhow::bail!("Unknown preset name \"{name}\". Expected architect, coder or utility.");
            }
            serde_json::from_value::<AgentPreset>(value.clone())
                .map_err(|err| anyhow::anyhow!("Invalid preset \"{name}\": {err}"))?;
        }
        if let Value::Object(stored) = Self::read_json(&self.presets_path()) {
            for (name, value) in stored {
                if presets.contains_key(&name) {
                    presets.insert(name, value);
                }
            }
        }
        for (name, value) in submitted {
            presets.insert(name.clone(), value.clone());
        }
        Self::write_json(&self.presets_path(), &Value::Object(presets.clone()))?;
        Ok(Value::Object(presets))
    }
}

/// Short human-readable error summary from an API error body.
fn summarize_error(body: &Value) -> String {
    body.pointer("/error/message")
        .or_else(|| body.pointer("/error"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dirs(label: &str) -> (PathBuf, PathBuf) {
        let unique = uuid::Uuid::new_v4().to_string();
        let agent = std::env::temp_dir().join(format!("locagens-pi-{label}-{unique}-agent"));
        let support = std::env::temp_dir().join(format!("locagens-pi-{label}-{unique}-support"));
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::create_dir_all(&support).unwrap();
        (agent, support)
    }

    #[tokio::test]
    async fn auth_json_roundtrip_and_key_resolution() {
        let (agent, support) = temp_dirs("auth");
        let manager = PiProviderManager::with_dirs(agent.clone(), support.clone());

        // No credential, no env → None.
        std::env::remove_var("LOCAGENS_TEST_KEY_VAR");
        assert!(manager.resolve_api_key("acme", None).await.is_none());

        // Set credential and read it back masked.
        manager
            .set_pi_auth(&SetPiAuthInput {
                provider_id: "acme".into(),
                action: "set".into(),
                api_key: Some("sk-abcdef123456".into()),
            })
            .unwrap();
        let status = manager.pi_auth_status();
        assert_eq!(status.len(), 1);
        assert_eq!(status[0]["providerId"], "acme");
        assert_eq!(status[0]["keyPreview"], "sk-a***3456");
        assert_eq!(
            manager.resolve_api_key("acme", None).await.as_deref(),
            Some("sk-abcdef123456")
        );

        // Delete credential.
        manager
            .set_pi_auth(&SetPiAuthInput {
                provider_id: "acme".into(),
                action: "delete".into(),
                api_key: None,
            })
            .unwrap();
        assert!(!manager.has_api_key("acme"));
        let _ = std::fs::remove_dir_all(agent);
        let _ = std::fs::remove_dir_all(support);
    }

    #[tokio::test]
    async fn env_var_credential_resolution() {
        let (agent, support) = temp_dirs("env");
        let manager = PiProviderManager::with_dirs(agent.clone(), support.clone());
        std::env::set_var("LOCAGENS_TEST_KEY_VAR", "from-env");
        assert_eq!(
            manager
                .resolve_api_key("acme", Some("$LOCAGENS_TEST_KEY_VAR"))
                .await
                .as_deref(),
            Some("from-env")
        );
        std::env::remove_var("LOCAGENS_TEST_KEY_VAR");
        let _ = std::fs::remove_dir_all(agent);
        let _ = std::fs::remove_dir_all(support);
    }

    #[test]
    fn builtin_catalog_and_configs_mask_keys() {
        let (agent, support) = temp_dirs("catalog");
        let manager = PiProviderManager::with_dirs(agent.clone(), support.clone());
        assert!(manager.get_presets()["architect"]["model"] == "glm-5.3-flash");
        manager
            .save_config(&SaveProviderConfigInput {
                provider_id: "anthropic".into(),
                api_key: Some("sk-ant-live-key".into()),
                display_name: Some("Anthropic (work)".into()),
                model_settings: Some(json!({ "temperature": 0.2 })),
                ..Default::default()
            })
            .unwrap();
        let configs = manager.get_configs();
        let anthropic = configs.get("anthropic").unwrap();
        assert_eq!(anthropic.api_key, PRESERVE_API_KEY);
        assert!(anthropic.has_api_key);
        assert_eq!(anthropic.display_name.as_deref(), Some("Anthropic (work)"));
        assert_eq!(anthropic.base_url, "https://api.anthropic.com/v1");
        assert!(anthropic.models.contains(&"claude-sonnet-4-5".to_string()));

        let providers = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(manager.list_providers());
        let anthropic_meta = providers.iter().find(|p| p.id == "anthropic").unwrap();
        assert_eq!(anthropic_meta.display_name, "Anthropic (work)");
        assert!(anthropic_meta.has_api_key);

        // Saved auth.json is a real credential file, not exposing metadata.
        let auth = PiProviderManager::read_json(&agent.join("auth.json"));
        assert_eq!(auth["anthropic"]["key"], "sk-ant-live-key");
        let _ = std::fs::remove_dir_all(agent);
        let _ = std::fs::remove_dir_all(support);
    }

    #[test]
    fn presets_roundtrip_validation() {
        let (agent, support) = temp_dirs("presets");
        let manager = PiProviderManager::with_dirs(agent.clone(), support.clone());
        let saved = manager
            .save_presets(&json!({
                "architect": { "providerId": "anthropic", "model": "claude-sonnet-4-5", "reasoningEffort": "high" }
            }))
            .unwrap();
        assert_eq!(saved["architect"]["model"], "claude-sonnet-4-5");

        // Unknown preset names are rejected.
        assert!(manager
            .save_presets(&json!({
                "bogus": { "providerId": "x", "model": "y" }
            }))
            .is_err());
        assert_eq!(manager.get_presets()["coder"]["model"], "glm-5.3-flash");
        let _ = std::fs::remove_dir_all(agent);
        let _ = std::fs::remove_dir_all(support);
    }
}
