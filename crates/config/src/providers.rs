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
//! `OPENAI_API_KEY` and `config.toml` `[model_providers.*]`) and the legacy
//! `~/.pi/agent` directory, so existing credentials keep working.
//!
//! Plus Locagens UI metadata under the app-support dir:
//! `locagens-providers-meta.json` (display names / model settings).

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

/// Metadata returned by `GET /api/providers` (web client contract: `type` is
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

/// Editable provider config returned by `GET /api/providers/config` —
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

/// One entry of the map submitted to `POST /api/providers/config`
/// (`{ [providerId]: { type, displayName, baseUrl, apiKey, models, modelSettings } }`).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
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

/// Body of `POST /api/providers/fetch-models` — `providerId` is optional;
/// when absent the `baseUrl`/`apiKey`/`type` from the body are used directly.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct FetchModelsInput {
    #[serde(rename = "type", alias = "api")]
    pub provider_type: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub provider_id: Option<String>,
}

/// Body of `POST /api/providers/test`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct TestProviderInput {
    pub provider_id: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
}

/// Body of `POST /api/pi/auth`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SetPiAuthInput {
    pub provider_id: String,
    pub action: String,
    pub api_key: Option<String>,
}

/// Explicit reasoning-effort levels for a model catalog entry: an array of
/// level strings (`reasoningEfforts`) wins, otherwise the set of
/// non-null level keys in `thinkingLevelMap` (canonical order). `None` when
/// the entry carries neither field.
fn parse_reasoning_efforts(model: &Value) -> Option<Vec<String>> {
    if let Some(list) = model.get("reasoningEfforts").and_then(Value::as_array) {
        return Some(
            list.iter()
                .filter_map(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect(),
        );
    }
    if let Some(map) = model.get("thinkingLevelMap").and_then(Value::as_object) {
        const CANONICAL_ORDER: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];
        let efforts: Vec<String> = CANONICAL_ORDER
            .iter()
            .filter(|level| map.get(**level).and_then(Value::as_str).is_some())
            .map(|level| level.to_string())
            .collect();
        return Some(efforts);
    }
    None
}

/// Supported reasoning-effort levels for a model, using explicit catalog
/// metadata (`thinkingLevelMap` / `reasoningEfforts`) when known and model
/// capability fallbacks otherwise.
pub fn model_reasoning_efforts(
    model_id: &str,
    reasoning: Option<bool>,
    explicit: Option<&[String]>,
) -> Vec<String> {
    if let Some(list) = explicit {
        return list.to_vec();
    }
    if reasoning == Some(false) {
        return Vec::new();
    }
    let id = model_id.to_lowercase();
    // Models with no reasoning-effort control at all.
    if id.contains("gpt-4")
        || id.contains("haiku-4")
        || id == "deepseek-chat"
        || id == "deepseek-reasoner"
        || id.contains("deepseek-r")
        || id.contains("qwen3-coder")
        || id.contains("qwen3.5")
        || id.contains("qwen3.7")
        || id.contains("qwen3.8")
        || id.contains("gemma")
        || id.contains("kimi-k2")
        || id.contains("mimo-")
        || id.contains("longcat")
        || id.contains("iceman")
        || id.contains("embed")
        || (id.contains("minimax-m3") && !id.contains("minimax-m3.1"))
    {
        return Vec::new();
    }
    const MODEL_SETS: [(&[&str], &[&str]); 6] = [
        (
            &["gpt-5.6", "gpt-6", "beam-free", "minimax-m3.1"],
            &["low", "medium", "high", "xhigh", "max"],
        ),
        (&["gpt-5", "grok-4", "codex"], &["low", "medium", "high", "xhigh"]),
        (
            &["muse-spark", "muse-wonder"],
            &["minimal", "low", "medium", "high", "xhigh"],
        ),
        (&["glm-5.3", "deepseek-v4.1", "kimi-k3.1"], &["low", "high", "max"]),
        (&["glm-5.2", "deepseek-v4-pro", "claude-haiku-5"], &["high", "max"]),
        (
            &["gemini-2.5", "gemini-3", "gemini-4", "claude-sonnet", "claude-opus", "o1", "o3", "o4", "step-5"],
            &["low", "medium", "high"],
        ),
    ];
    for (prefixes, efforts) in MODEL_SETS {
        if prefixes.iter().any(|prefix| id.starts_with(prefix)) {
            return efforts.iter().map(|e| e.to_string()).collect();
        }
    }
    if reasoning == Some(true) {
        return ["low", "medium", "high"]
            .iter()
            .map(|e| e.to_string())
            .collect();
    }
    Vec::new()
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

/// Maps a fallback-chain `auth.json` entry onto a Locagens provider
/// credential. Codex stores `{ "OPENAI_API_KEY": "sk-…", "tokens": … }` while
/// Locagens/`pi` store `{ "<provider>": { "type": "api_key", "key": … } }`.
fn codex_credential(key: &str, value: &Value) -> Vec<(String, Value)> {
    if key == "OPENAI_API_KEY" {
        if let Some(api_key) = value.as_str().filter(|k| !k.trim().is_empty()) {
            return vec![(
                "openai".to_string(),
                json!({ "type": "api_key", "key": api_key.trim() }),
            )];
        }
        return Vec::new();
    }
    // Codex ChatGPT-login `auth.json`: the OAuth `tokens.access_token` is a
    // usable OpenAI credential when `OPENAI_API_KEY` is absent/null.
    if key == "tokens" {
        let access_token = value
            .get("access_token")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|token| !token.is_empty());
        if let Some(access_token) = access_token {
            return vec![(
                "openai".to_string(),
                json!({ "type": "api_key", "key": access_token }),
            )];
        }
        return Vec::new();
    }
    // pi-style credential entries pass through keyed by provider id.
    match value {
        Value::Object(_) if value.get("type").is_some() || value.get("key").is_some() => {
            vec![(key.to_string(), value.clone())]
        }
        _ => Vec::new(),
    }
}

/// True when a provider talks the Anthropic Messages API.
fn is_anthropic(provider_id: &str, api: &str, base_url: &str) -> bool {
    api == "anthropic"
        || api == "anthropic-messages"
        || provider_id == "anthropic"
        || base_url.contains("anthropic")
}

/// Web-client provider type for a resolved `api` kind: `anthropic-messages`
/// (and friends) map to `"anthropic"`, the Codex Responses WireAPI
/// (`openai-responses` / `responses`) keeps its own kind, and everything else
/// falls back to `"openai-compatible"`.
fn provider_type(api: &str) -> &'static str {
    if api.contains("anthropic") {
        "anthropic"
    } else if api == "openai-responses" || api == "responses" {
        "openai-responses"
    } else {
        "openai-compatible"
    }
}

pub struct PiProviderManager {
    /// Primary agent config directory (`~/.locagens`). Writes go here.
    primary_dir: PathBuf,
    /// Read-only credential/config fallbacks: `~/.codex`, `~/.pi/agent`.
    fallback_dirs: Vec<PathBuf>,
    app_support_dir: PathBuf,
    http: reqwest::Client,
    /// Cache of `!command` stdout results, keyed by command string.
    command_cache: Mutex<HashMap<String, String>>,
}

impl PiProviderManager {
    pub fn new() -> Self {
        Self::with_chain(
            paths::locagens_home(),
            paths::app_support_dir(),
            vec![paths::codex_home(), paths::pi_agent_dir()],
        )
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
            app_support_dir,
            http,
            command_cache: Mutex::new(HashMap::new()),
        }
    }

    // ------------------------------------------------------------------
    // File paths
    // ------------------------------------------------------------------

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
        // Fallback chain first (codex + pi), primary last so it wins.
        let mut merged = Map::new();
        for directory in self.fallback_dirs.iter() {
            let path = directory.join("auth.json");
            match Self::read_json(&path) {
                Value::Object(map) => {
                    // An explicit non-empty `OPENAI_API_KEY` must win over a
                    // `tokens.access_token` from the same file, regardless of
                    // JSON key order.
                    let mut explicit_openai: Option<Value> = None;
                    let mut tokens_openai: Option<Value> = None;
                    let mut others: Vec<(String, Value)> = Vec::new();
                    for (key, value) in map {
                        if key == "OPENAI_API_KEY" {
                            explicit_openai =
                                codex_credential(&key, &value).into_iter().next().map(|(_, entry)| entry);
                        } else {
                            let from_tokens = key == "tokens";
                            for (provider_id, entry) in codex_credential(&key, &value) {
                                if from_tokens && provider_id == "openai" {
                                    tokens_openai = Some(entry.clone());
                                }
                                others.push((provider_id, entry));
                            }
                        }
                    }
                    for (provider_id, entry) in others {
                        merged.entry(provider_id).or_insert(entry);
                    }
                    if let Some(entry) = explicit_openai {
                        // Only override a tokens-derived openai entry from this
                        // same file (or nothing at all); credentials from
                        // earlier chain entries keep their fallback priority.
                        let overwritable = match merged.get("openai") {
                            Some(existing) => tokens_openai.as_ref() == Some(existing),
                            None => true,
                        };
                        if overwritable {
                            merged.insert("openai".to_string(), entry);
                        }
                    }
                }
                _ => continue,
            }
        }
        match Self::read_json(&self.auth_path()) {
            Value::Object(map) => {
                for (key, value) in map {
                    merged.insert(key, value);
                }
            }
            _ => {}
        }
        merged
    }

    fn write_auth_map(&self, map: Map<String, Value>) -> Result<()> {
        Self::write_json(&self.auth_path(), &Value::Object(map))
    }

    fn read_models_json(&self) -> Value {
        Self::read_json(&self.models_path())
    }

    /// `models.json` entry for a provider (`providers.<id>`), if any. Falls
    /// back to fallback-chain `models.json` files and to Codex's
    /// `config.toml` `[model_providers.*]` entries.
    fn models_json_provider(&self, provider_id: &str) -> Option<Map<String, Value>> {
        let models = self.read_models_json();
        if let Some(entry) = models
            .get("providers")
            .and_then(|providers| providers.get(provider_id))
            .and_then(Value::as_object)
            .cloned()
        {
            return Some(entry);
        }
        for directory in &self.fallback_dirs {
            let models = Self::read_json(&directory.join("models.json"));
            if let Some(entry) = models
                .get("providers")
                .and_then(|providers| providers.get(provider_id))
                .and_then(Value::as_object)
                .cloned()
            {
                return Some(entry);
            }
        }
        self.codex_config_providers()
            .get(provider_id)
            .and_then(Value::as_object)
            .cloned()
    }

    /// `[model_providers.*]` entries from Codex `config.toml` fallback
    /// directories, converted to `models.json`-shaped entries.
    fn codex_config_providers(&self) -> Map<String, Value> {
        let mut result = Map::new();
        for directory in &self.fallback_dirs {
            let Ok(raw) = std::fs::read_to_string(directory.join("config.toml")) else {
                continue;
            };
            let Ok(doc) = toml::from_str::<toml::Value>(&raw) else {
                continue;
            };
            let Some(providers) = doc.get("model_providers").and_then(toml::Value::as_table) else {
                continue;
            };
            for (name, entry) in providers {
                let base_url = entry
                    .get("base_url")
                    .and_then(toml::Value::as_str)
                    .unwrap_or_default();
                let env_key = entry
                    .get("env_key")
                    .and_then(toml::Value::as_str)
                    .unwrap_or_default();
                let mut converted = Map::new();
                converted.insert("name".into(), json!(name));
                if !base_url.is_empty() {
                    converted.insert("baseUrl".into(), json!(base_url));
                }
                if !env_key.is_empty() {
                    converted.insert("envKey".into(), json!(env_key));
                }
                // Codex `wire_api` ("chat" / "responses") maps onto the
                // matching API adapter: `responses` keeps the Codex
                // Responses WireAPI, everything else stays chat-completions.
                let wire_api = entry
                    .get("wire_api")
                    .and_then(toml::Value::as_str)
                    .unwrap_or_default();
                let api = if wire_api.eq_ignore_ascii_case("responses")
                    || wire_api.eq_ignore_ascii_case("openai-responses")
                {
                    "openai-responses"
                } else {
                    "openai-compatible"
                };
                converted.insert("api".into(), json!(api));
                result.insert(name.clone(), Value::Object(converted));
            }
        }
        result
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

        // Codex `config.toml` `env_key` fallback.
        if let Some(entry) = self.models_json_provider(provider_id) {
            if let Some(env_key) = entry.get("envKey").and_then(Value::as_str).filter(|k| !k.is_empty()) {
                if let Ok(value) = std::env::var(env_key) {
                    let value = value.trim().to_string();
                    if !value.is_empty() {
                        return Some(value);
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

    /// Resolved `(base_url, api)` pair for a provider — used by the
    /// orchestrator's streaming LLM client.
    pub fn resolve_endpoint(&self, provider_id: &str) -> (String, String) {
        (self.resolve_base_url(provider_id), self.resolve_api(provider_id))
    }

    /// A single merged model entry by id, if known.
    pub fn model_info(&self, provider_id: &str, model_id: &str) -> Option<ModelInfo> {
        self.merged_models(provider_id)
            .into_iter()
            .find(|model| model.id == model_id)
    }

    // ------------------------------------------------------------------
    // Catalog assembly
    // ------------------------------------------------------------------

    /// Models for a provider cached in `pi`'s `models-store.json` — fallback
    /// directories first (oldest to newest), then the primary agent dir whose
    /// entries win for matching model ids.
    fn models_store_models(&self, provider_id: &str) -> Vec<ModelInfo> {
        let mut models: Vec<ModelInfo> = Vec::new();
        for directory in self.fallback_dirs.iter().rev() {
            for model in Self::parse_models_store_file(
                &directory.join("models-store.json"),
                provider_id,
            ) {
                if let Some(existing) = models.iter_mut().find(|m| m.id == model.id) {
                    *existing = model;
                } else {
                    models.push(model);
                }
            }
        }
        for model in Self::parse_models_store_file(&self.models_store_path(), provider_id) {
            if let Some(existing) = models.iter_mut().find(|m| m.id == model.id) {
                *existing = model;
            } else {
                models.push(model);
            }
        }
        models
    }

    /// Models for one provider entry of a single `models-store.json` file.
    fn parse_models_store_file(path: &Path, provider_id: &str) -> Vec<ModelInfo> {
        let store = Self::read_json(path);
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
                            reasoning_efforts: parse_reasoning_efforts(model),
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
                            reasoning_efforts: None,
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
                                reasoning_efforts: parse_reasoning_efforts(&Value::Object(object.clone())),
                                id,
                            })
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `modelSettings` map (model id → settings) for a provider, merged from
    /// `locagens-providers-meta.json` plus details derived from
    /// `models.json` / `models-store.json` (contextLimit, pricing).
    fn model_settings_for(&self, provider_id: &str, models: &[ModelInfo]) -> Option<Value> {
        let mut map = self
            .meta_entry(provider_id)
            .as_ref()
            .and_then(|entry| entry.get("modelSettings"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for model in models {
            let efforts = model_reasoning_efforts(
                &model.id,
                model.reasoning,
                model.reasoning_efforts.as_deref(),
            );
            if !map.contains_key(&model.id) {
                if let Some(settings) = Self::derived_model_settings(model) {
                    map.insert(model.id.clone(), settings);
                }
            }
            // Every model entry carries its supported reasoning efforts —
            // including UI-entered meta entries that predate this field.
            if let Some(entry) = map.get_mut(&model.id).and_then(Value::as_object_mut) {
                entry
                    .entry("reasoningEfforts".to_string())
                    .or_insert_with(|| json!(efforts));
            }
        }
        if map.is_empty() {
            None
        } else {
            Some(Value::Object(map))
        }
    }

    /// Per-model settings derived from a merged `ModelInfo` (supported
    /// reasoning efforts, context window and pricing tiers) — used when no
    /// UI-entered settings exist yet. Always carries `reasoningEfforts`.
    fn derived_model_settings(model: &ModelInfo) -> Option<Value> {
        let mut settings = Map::new();
        let efforts = model_reasoning_efforts(
            &model.id,
            model.reasoning,
            model.reasoning_efforts.as_deref(),
        );
        settings.insert("reasoningEfforts".into(), json!(efforts));
        if let Some(context_limit) = model.context_window {
            settings.insert("contextLimit".into(), json!(context_limit));
        }
        if let Some(cost) = model.cost.as_ref().and_then(Value::as_object) {
            let rate = |key: &str| cost.get(key).and_then(Value::as_f64);
            if rate("input").is_some() || rate("output").is_some() {
                let mut tier = json!({});
                if let Some(input) = rate("input") {
                    tier["inputRate"] = json!(input);
                }
                if let Some(output) = rate("output") {
                    tier["outputRate"] = json!(output);
                }
                if let Some(read) = rate("cacheRead") {
                    tier["cacheReadRate"] = json!(read);
                }
                if let Some(write) = rate("cacheWrite") {
                    tier["cacheWriteRate"] = json!(write);
                }
                settings.insert("pricing".into(), json!({ "tiers": [tier] }));
            }
        }
        if settings.is_empty() {
            None
        } else {
            Some(Value::Object(settings))
        }
    }

    /// Provider ids to expose: built-ins plus anything present in `auth.json`,
    /// `models.json` or Codex `config.toml`.
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
        for key in self.codex_config_providers().keys() {
            if !ids.contains(key) {
                ids.push(key.clone());
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
                        reasoning_efforts: None,
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
                if model.reasoning_efforts.is_some() {
                    existing.reasoning_efforts = model.reasoning_efforts;
                }
            } else {
                models.push(model);
            }
        }

        models
    }

    /// All configured / authenticated providers for `GET /api/providers`, in
    /// the web client's `ProviderMetadata[]` contract shape.
    pub async fn list_providers(&self) -> Vec<ProviderMetadata> {
        self.provider_ids()
            .into_iter()
            .map(|id| {
                let api = self.resolve_api(&id);
                let models = self.merged_models(&id);
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
                    kind: provider_type(&api).to_string(),
                    models: models.iter().map(|model| model.id.clone()).collect(),
                    model_settings: self.model_settings_for(&id, &models),
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
                    kind: provider_type(&self.resolve_api(&id)).to_string(),
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

    /// `POST /api/providers/config` — accepts the whole
    /// `Record<providerId, ProviderConfig>` map the web client sends:
    /// persists every entry (API keys → `auth.json`, `type`/`baseUrl`/
    /// `models` → `models.json`, UI metadata →
    /// `locagens-providers-meta.json`) and removes providers that previously
    /// existed in the editable config but are missing from the submitted map
    /// (including their `auth.json` credential).
    pub fn save_configs(&self, entries: &HashMap<String, SaveProviderConfigInput>) -> Result<()> {
        for (provider_id, entry) in entries {
            self.save_single_provider(provider_id, entry)?;
        }
        self.delete_missing_providers(entries.keys())?;
        Ok(())
    }

    /// Persists one provider entry: API key → `auth.json`,
    /// `type`/`baseUrl`/`models` → `models.json`, displayName/modelSettings →
    /// `locagens-providers-meta.json`. An `apiKey` equal to
    /// [`PRESERVE_API_KEY`] keeps the existing credential untouched.
    fn save_single_provider(&self, provider_id: &str, input: &SaveProviderConfigInput) -> Result<()> {
        let provider_id = provider_id.trim();
        if provider_id.is_empty() {
            anyhow::bail!("providerId is required");
        }

        // 1. API key → auth.json (preserve existing keys when masked).
        if let Some(api_key) = input.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
            if api_key != PRESERVE_API_KEY {
                let mut auth = self.read_auth_map();
                auth.insert(
                    provider_id.to_string(),
                    serde_json::to_value(AuthCredential::api_key(api_key))?,
                );
                self.write_auth_map(auth)?;
            }
        }

        // 2. type / baseUrl / models → models.json (`type: "anthropic"` is
        //    stored as pi's `anthropic-messages` api kind; `openai-responses`
        //    / `responses` keep the Codex Responses WireAPI kind).
        let api = input
            .provider_type
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| {
                if t.eq_ignore_ascii_case("anthropic") {
                    "anthropic-messages".to_string()
                } else if t.eq_ignore_ascii_case("openai-responses")
                    || t.eq_ignore_ascii_case("responses")
                {
                    "openai-responses".to_string()
                } else {
                    t.to_string()
                }
            });
        let mut models_doc = self.read_models_json();
        if !models_doc.is_object() {
            models_doc = json!({});
        }
        let needs_provider_entry = input.base_url.is_some() || input.models.is_some() || api.is_some();
        if needs_provider_entry {
            let object = models_doc.as_object_mut().unwrap();
            let providers = object
                .entry("providers")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .unwrap();
            let entry = providers
                .entry(provider_id.to_string())
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
            match api.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
                Some(api) => {
                    entry.insert("api".into(), json!(api));
                }
                None => {
                    // The provider_type was not changed in this save: keep an
                    // explicitly configured Responses WireAPI instead of
                    // clobbering it back to chat-completions.
                    let is_responses =
                        entry.get("api").and_then(Value::as_str) == Some("openai-responses");
                    if !is_responses {
                        entry.remove("api");
                    }
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
                .entry(provider_id.to_string())
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

    /// Removes providers that previously existed in the editable config but
    /// are missing from the submitted map: their `models.json` entry, UI
    /// metadata and `auth.json` credential are all deleted.
    fn delete_missing_providers<'a, I>(&self, kept: I) -> Result<()>
    where
        I: Iterator<Item = &'a String>,
    {
        let kept: Vec<&str> = kept.map(|id| id.as_str()).collect();
        let removed: Vec<String> = self
            .provider_ids()
            .into_iter()
            .filter(|id| !kept.contains(&id.as_str()))
            .collect();
        if removed.is_empty() {
            return Ok(());
        }
        let mut models_doc = self.read_models_json();
        let mut meta = Self::read_json(&self.meta_path());
        let mut auth = self.read_auth_map();
        for id in &removed {
            if let Some(providers) = models_doc.get_mut("providers").and_then(Value::as_object_mut) {
                providers.remove(id);
            }
            if let Some(providers) = meta.get_mut("providers").and_then(Value::as_object_mut) {
                providers.remove(id);
            }
            auth.remove(id);
        }
        Self::write_json(&self.models_path(), &models_doc)?;
        Self::write_json(&self.meta_path(), &meta)?;
        self.write_auth_map(auth)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Remote model fetching
    // ------------------------------------------------------------------

    /// `POST /api/providers/fetch-models` — queries the provider's model
    /// list endpoint (openai-compatible `GET {baseUrl}/models`, Anthropic
    /// `/v1/models`) and returns plain model ids. When `providerId` is
    /// missing, the `baseUrl`/`apiKey`/`type` from the request body are used
    /// directly; when present, fetched ids are merged with the provider's
    /// cached `models-store.json` models.
    pub async fn fetch_models(&self, input: &FetchModelsInput) -> Result<Vec<String>> {
        let provider_id = input
            .provider_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string);
        let base_url = input
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string)
            .or_else(|| provider_id.as_ref().map(|id| self.resolve_base_url(id)));
        let base_url = base_url
            .filter(|url| !url.is_empty())
            .ok_or_else(|| anyhow::anyhow!("No baseUrl provided or configured"))?
            .trim_end_matches('/')
            .to_string();

        // Anthropic when an explicit `type` says so, or inferred from the
        // provider / base URL when `type` is absent.
        let explicit_type = input
            .provider_type
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty());
        let anthropic = match explicit_type {
            Some(t) => t.contains("anthropic"),
            None => provider_id
                .as_ref()
                .map(|id| is_anthropic(id, &self.resolve_api(id), &base_url))
                .unwrap_or_else(|| base_url.contains("anthropic")),
        };

        let supplied_key = input
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(str::to_string);
        let api_key = match supplied_key {
            Some(key) => Some(key),
            None => match provider_id.as_ref() {
                Some(id) => self.resolve_api_key(id, None).await,
                None => None,
            },
        };

        let mut models: Vec<String> = provider_id
            .as_ref()
            .map(|id| {
                self.models_store_models(id)
                    .into_iter()
                    .map(|model| model.id)
                    .collect()
            })
            .unwrap_or_default();

        let request = if anthropic {
            let url = if base_url.ends_with("/v1") {
                format!("{base_url}/models")
            } else {
                format!("{base_url}/v1/models")
            };
            let mut request = self.http.get(url).header("anthropic-version", "2023-06-01");
            if let Some(key) = api_key.as_ref().filter(|k| !k.is_empty()) {
                request = request.header("x-api-key", key);
            }
            request
        } else {
            let mut request = self.http.get(format!("{base_url}/models"));
            if let Some(key) = api_key.as_ref().filter(|k| !k.is_empty()) {
                request = request.bearer_auth(key);
            }
            request
        };

        let response = request.send().await?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            anyhow::bail!(
                "Model fetch failed with status {status}: {}",
                summarize_error(&body)
            );
        }
        let entries: Vec<Value> = match &body {
            Value::Array(array) => array.clone(),
            Value::Object(_) => body
                .get("data")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        for entry in &entries {
            let id = match entry {
                Value::String(id) => id.clone(),
                _ => entry
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            };
            if id.is_empty() || models.iter().any(|existing| existing == &id) {
                continue;
            }
            models.push(id);
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
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::sync::Arc;

    fn temp_dirs(label: &str) -> (PathBuf, PathBuf) {
        let unique = uuid::Uuid::new_v4().to_string();
        let agent = std::env::temp_dir().join(format!("locagens-pi-{label}-{unique}-agent"));
        let support = std::env::temp_dir().join(format!("locagens-pi-{label}-{unique}-support"));
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::create_dir_all(&support).unwrap();
        (agent, support)
    }

    /// Serializes tests that touch `LOCAGENS_TEST_KEY_VAR` (env vars are
    /// process-global, so parallel `set_var`/`remove_var` would race).
    static ENV_VAR_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[tokio::test]
    async fn auth_json_roundtrip_and_key_resolution() {
        let _guard = ENV_VAR_TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let _guard = ENV_VAR_TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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

    #[tokio::test]
    async fn codex_oauth_tokens_in_auth_json_resolve_for_openai() {
        let (agent, support) = temp_dirs("codex-tokens");
        let codex = std::env::temp_dir().join(format!(
            "locagens-pi-codex-tokens-fallback-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&codex).unwrap();
        let manager = PiProviderManager::with_chain(
            agent.clone(),
            support.clone(),
            vec![codex.clone()],
        );

        // `OPENAI_API_KEY: null` + OAuth `tokens.access_token` → openai works.
        let value = json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "access_token": "  ey-oauth-token  ",
                "refresh_token": "r",
                "account_id": "acc"
            }
        });
        PiProviderManager::write_json(&codex.join("auth.json"), &value).unwrap();
        assert_eq!(
            manager.resolve_api_key("openai", None).await.as_deref(),
            Some("ey-oauth-token")
        );

        // An explicit non-empty OPENAI_API_KEY wins over tokens.
        let value = json!({
            "OPENAI_API_KEY": "sk-explicit-key",
            "tokens": { "access_token": "ey-oauth-token" }
        });
        PiProviderManager::write_json(&codex.join("auth.json"), &value).unwrap();
        assert_eq!(
            manager.resolve_api_key("openai", None).await.as_deref(),
            Some("sk-explicit-key")
        );

        // Empty/missing tokens fall back cleanly.
        let value = json!({ "OPENAI_API_KEY": null, "tokens": { "access_token": "" } });
        PiProviderManager::write_json(&codex.join("auth.json"), &value).unwrap();
        assert!(manager.read_auth_map().get("openai").is_none());

        let _ = std::fs::remove_dir_all(agent);
        let _ = std::fs::remove_dir_all(support);
        let _ = std::fs::remove_dir_all(codex);
    }

    #[test]
    fn builtin_catalog_and_configs_mask_keys() {
        let (agent, support) = temp_dirs("catalog");
        let manager = PiProviderManager::with_dirs(agent.clone(), support.clone());

        let mut entries = HashMap::new();
        entries.insert(
            "anthropic".to_string(),
            SaveProviderConfigInput {
                provider_type: Some("anthropic".into()),
                api_key: Some("sk-ant-live-key".into()),
                display_name: Some("Anthropic (work)".into()),
                model_settings: Some(json!({ "claude-sonnet-4-5": { "contextLimit": 200000 } })),
                ..Default::default()
            },
        );
        manager.save_configs(&entries).unwrap();

        let configs = manager.get_configs();
        let anthropic = configs.get("anthropic").unwrap();
        assert_eq!(anthropic.kind, "anthropic");
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
        assert_eq!(anthropic_meta.kind, "anthropic");
        assert!(anthropic_meta.models.contains(&"claude-sonnet-4-5".to_string()));
        assert!(anthropic_meta.models.iter().all(|model| !model.is_empty()));
        assert_eq!(
            anthropic_meta.model_settings.as_ref().unwrap()["claude-sonnet-4-5"]["contextLimit"],
            200000
        );

        // No API keys are ever included in the provider metadata.
        let raw = serde_json::to_string(&providers).unwrap();
        assert!(!raw.contains("sk-"));
        assert!(!raw.contains("apiKey"));

        // Saved auth.json is a real credential file, not exposing metadata.
        let auth = PiProviderManager::read_json(&agent.join("auth.json"));
        assert_eq!(auth["anthropic"]["key"], "sk-ant-live-key");
        let _ = std::fs::remove_dir_all(agent);
        let _ = std::fs::remove_dir_all(support);
    }

    #[test]
    fn provider_metadata_contract_shape() {
        let (agent, support) = temp_dirs("meta");
        std::fs::write(
            agent.join("models-store.json"),
            json!({
                "openai": { "models": [
                    { "id": "gpt-x", "name": "GPT X", "contextWindow": 128000, "cost": { "input": 1.5, "output": 6.0, "cacheRead": 0.15 } }
                ]}
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            agent.join("models.json"),
            json!({
                "providers": {
                    "acme": { "baseUrl": "https://acme.example/v1", "api": "anthropic-messages", "models": ["fast-model"] }
                }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            support.join("locagens-providers-meta.json"),
            json!({
                "providers": {
                    "anthropic": { "displayName": "Anthropic", "modelSettings": { "claude-opus-4-5": { "contextLimit": 200000 } } }
                }
            })
            .to_string(),
        )
        .unwrap();
        let manager = PiProviderManager::with_dirs(agent.clone(), support.clone());

        let providers = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(manager.list_providers());

        // models-store details derive contextLimit / pricing settings.
        let openai = providers.iter().find(|p| p.id == "openai").unwrap();
        assert_eq!(openai.kind, "openai-compatible");
        assert!(openai.models.contains(&"gpt-x".to_string()));
        let settings = openai.model_settings.as_ref().unwrap();
        assert_eq!(settings["gpt-x"]["contextLimit"], 128000);
        assert_eq!(settings["gpt-x"]["pricing"]["tiers"][0]["inputRate"], 1.5);
        assert_eq!(settings["gpt-x"]["pricing"]["tiers"][0]["outputRate"], 6.0);
        assert_eq!(settings["gpt-x"]["pricing"]["tiers"][0]["cacheReadRate"], 0.15);

        // UI-entered meta settings win, and `type` is "anthropic".
        let anthropic = providers.iter().find(|p| p.id == "anthropic").unwrap();
        assert_eq!(anthropic.kind, "anthropic");
        assert_eq!(anthropic.display_name, "Anthropic");
        assert_eq!(
            anthropic.model_settings.as_ref().unwrap()["claude-opus-4-5"]["contextLimit"],
            200000
        );

        // `anthropic-messages` api maps to the "anthropic" client type.
        let acme = providers.iter().find(|p| p.id == "acme").unwrap();
        assert_eq!(acme.kind, "anthropic");
        assert_eq!(acme.models, vec!["fast-model".to_string()]);

        let _ = std::fs::remove_dir_all(agent);
        let _ = std::fs::remove_dir_all(support);
    }

    #[test]
    fn config_map_save_deletes_missing_providers() {
        let (agent, support) = temp_dirs("configs");
        let manager = PiProviderManager::with_dirs(agent.clone(), support.clone());

        let mut entries = HashMap::new();
        entries.insert(
            "anthropic".to_string(),
            SaveProviderConfigInput {
                api_key: Some("sk-ant".into()),
                ..Default::default()
            },
        );
        entries.insert(
            "acme".to_string(),
            SaveProviderConfigInput {
                provider_type: Some("openai-compatible".into()),
                base_url: Some("https://acme.example/v1".into()),
                api_key: Some("sk-acme".into()),
                models: Some(vec!["m1".into(), "m2".into()]),
                display_name: Some("Acme".into()),
                model_settings: Some(json!({ "m1": { "contextLimit": 4096 } })),
            },
        );
        entries.insert(
            "grox".to_string(),
            SaveProviderConfigInput {
                provider_type: Some("openai-compatible".into()),
                base_url: Some("https://grox.example/v1".into()),
                api_key: Some("sk-grox".into()),
                display_name: Some("Grox".into()),
                ..Default::default()
            },
        );
        manager.save_configs(&entries).unwrap();

        // Grox exists in the editable config after the first save.
        assert!(manager.get_configs().contains_key("grox"));

        let auth = PiProviderManager::read_json(&agent.join("auth.json"));
        assert_eq!(auth["acme"]["key"], "sk-acme");
        assert_eq!(auth["anthropic"]["key"], "sk-ant");
        let models_doc = PiProviderManager::read_json(&agent.join("models.json"));
        assert_eq!(models_doc["providers"]["acme"]["baseUrl"], "https://acme.example/v1");
        assert_eq!(models_doc["providers"]["acme"]["api"], "openai-compatible");
        let meta = PiProviderManager::read_json(&support.join("locagens-providers-meta.json"));
        assert_eq!(meta["providers"]["acme"]["displayName"], "Acme");
        assert_eq!(meta["providers"]["acme"]["modelSettings"]["m1"]["contextLimit"], 4096);

        // The second map keeps only acme (with the preserve sentinel);
        // anthropic and grox are missing, so they are deleted entirely.
        let mut entries = HashMap::new();
        entries.insert(
            "acme".to_string(),
            SaveProviderConfigInput {
                api_key: Some(PRESERVE_API_KEY.into()),
                ..Default::default()
            },
        );
        manager.save_configs(&entries).unwrap();

        let auth = PiProviderManager::read_json(&agent.join("auth.json"));
        assert_eq!(auth["acme"]["key"], "sk-acme");
        assert!(auth.get("anthropic").is_none());
        assert!(auth.get("grox").is_none());
        let models_doc = PiProviderManager::read_json(&agent.join("models.json"));
        assert!(models_doc["providers"].get("grox").is_none());
        assert!(models_doc["providers"].get("acme").is_some());
        let meta = PiProviderManager::read_json(&support.join("locagens-providers-meta.json"));
        assert!(meta["providers"].get("grox").is_none());
        assert!(meta["providers"].get("acme").is_some());
        assert!(!manager.get_configs().contains_key("grox"));
        let _ = std::fs::remove_dir_all(agent);
        let _ = std::fs::remove_dir_all(support);
    }

    /// Tiny local HTTP server: serves queued JSON responses for sequential
    /// requests and logs each raw request for assertions.
    fn spawn_model_server(responses: Vec<Value>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let queue = Arc::new(Mutex::new(responses));
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let log_clone = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let read = stream.read(&mut chunk).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                    if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                log_clone
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buffer).to_string());
                let body = queue
                    .lock()
                    .unwrap()
                    .pop()
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "{}".to_string());
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        (format!("http://127.0.0.1:{port}"), log)
    }

    #[tokio::test]
    async fn fetch_models_openai_compatible_without_provider_id() {
        let (agent, support) = temp_dirs("fetch-openai");
        let manager = PiProviderManager::with_dirs(agent.clone(), support.clone());
        let (base_url, log) = spawn_model_server(vec![json!({
            "data": [ { "id": "gpt-remote-1" }, { "id": "gpt-remote-2" } ]
        })]);

        let models = manager
            .fetch_models(&FetchModelsInput {
                provider_type: Some("openai-compatible".into()),
                base_url: Some(base_url),
                api_key: Some("sk-openai-test".into()),
                provider_id: None,
            })
            .await
            .unwrap();
        assert_eq!(
            models,
            vec!["gpt-remote-1".to_string(), "gpt-remote-2".to_string()]
        );

        let requests = log.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert!(request.starts_with("GET /models "), "unexpected request: {request}");
        assert!(request.to_lowercase().contains("authorization: bearer sk-openai-test"));

        let _ = std::fs::remove_dir_all(agent);
        let _ = std::fs::remove_dir_all(support);
    }

    #[tokio::test]
    async fn fetch_models_anthropic_uses_v1_models() {
        let (agent, support) = temp_dirs("fetch-anthropic");
        let manager = PiProviderManager::with_dirs(agent.clone(), support.clone());
        let (base_url, log) = spawn_model_server(vec![json!({
            "data": [ { "id": "claude-remote" } ]
        })]);

        let models = manager
            .fetch_models(&FetchModelsInput {
                provider_type: Some("anthropic".into()),
                base_url: Some(base_url),
                api_key: Some("sk-ant-test".into()),
                provider_id: None,
            })
            .await
            .unwrap();
        assert_eq!(models, vec!["claude-remote".to_string()]);

        let requests = log.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert!(request.starts_with("GET /v1/models "), "unexpected request: {request}");
        assert!(request.to_lowercase().contains("x-api-key: sk-ant-test"));
        assert!(request.to_lowercase().contains("anthropic-version: 2023-06-01"));

        let _ = std::fs::remove_dir_all(agent);
        let _ = std::fs::remove_dir_all(support);
    }

    #[tokio::test]
    async fn fetch_models_with_provider_id_merges_store() {
        let (agent, support) = temp_dirs("fetch-merge");
        std::fs::write(
            agent.join("models-store.json"),
            json!({ "mystore": { "models": [ { "id": "stored-model" } ] } }).to_string(),
        )
        .unwrap();
        let manager = PiProviderManager::with_dirs(agent.clone(), support.clone());
        let (base_url, log) = spawn_model_server(vec![json!({
            "data": [ { "id": "stored-model" }, { "id": "remote-model" } ]
        })]);

        let models = manager
            .fetch_models(&FetchModelsInput {
                provider_type: Some("openai-compatible".into()),
                base_url: Some(base_url),
                api_key: None,
                provider_id: Some("mystore".into()),
            })
            .await
            .unwrap();
        assert_eq!(
            models,
            vec!["stored-model".to_string(), "remote-model".to_string()]
        );

        let requests = log.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("GET /models "));

        let _ = std::fs::remove_dir_all(agent);
        let _ = std::fs::remove_dir_all(support);
    }

    #[tokio::test]
    async fn codex_wire_api_responses_resolves_to_openai_responses() {
        let (agent, support) = temp_dirs("wireapi");
        let codex = std::env::temp_dir().join(format!("locagens-pi-wireapi-codex-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&codex).unwrap();
        std::fs::write(
            codex.join("config.toml"),
            r#"
[model_providers.custom]
name = "Custom"
base_url = "https://custom.example.com/v1"
env_key = "CUSTOM_API_KEY"
wire_api = "responses"

[model_providers.chatonly]
name = "ChatOnly"
base_url = "https://chat.example.com/v1"
wire_api = "chat"
"#,
        )
        .unwrap();
        let manager = PiProviderManager::with_chain(agent.clone(), support.clone(), vec![codex.clone()]);

        // `wire_api = "responses"` keeps the Codex Responses WireAPI.
        assert_eq!(manager.resolve_api("custom"), "openai-responses");
        assert_eq!(provider_type(&manager.resolve_api("custom")), "openai-responses");
        // Any other wire_api (or none) stays chat-completions.
        assert_eq!(manager.resolve_api("chatonly"), "openai-compatible");
        assert_eq!(provider_type("anthropic-messages"), "anthropic");
        assert_eq!(provider_type("responses"), "openai-responses");
        assert_eq!(provider_type("openai-compatible"), "openai-compatible");

        let _ = std::fs::remove_dir_all(agent);
        let _ = std::fs::remove_dir_all(support);
        let _ = std::fs::remove_dir_all(codex);
    }

    #[test]
    fn models_store_thinking_level_map_populates_reasoning_efforts() {
        let (agent, support) = temp_dirs("efforts");
        let fallback =
            std::env::temp_dir().join(format!("locagens-pi-efforts-fallback-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&fallback).unwrap();

        // Fallback catalog: `thinkingLevelMap` metadata plus an empty map.
        PiProviderManager::write_json(
            &fallback.join("models-store.json"),
            &json!({
                "acme": { "models": [
                    { "id": "gemini-4-argon", "name": "Gemini 4 Argon", "thinkingLevelMap": { "low": "low", "medium": "medium", "high": "high" } },
                    { "id": "no-effort-model", "name": "No Effort", "thinkingLevelMap": {} }
                ]}
            }),
        )
        .unwrap();

        // Primary catalog: `thinkingLevelMap` and explicit `reasoningEfforts`.
        PiProviderManager::write_json(
            &agent.join("models-store.json"),
            &json!({
                "acme": { "models": [
                    { "id": "gpt-5.6-luna", "name": "GPT 5.6 Luna", "thinkingLevelMap": { "low": "low", "medium": "medium", "high": "high", "xhigh": "xhigh", "max": "max" } },
                    { "id": "glm-5.3-flash", "name": "GLM Flash", "reasoningEfforts": ["low", "high", "max"] }
                ]}
            }),
        )
        .unwrap();

        let manager =
            PiProviderManager::with_chain(agent.clone(), support.clone(), vec![fallback.clone()]);
        let models = manager.models_store_models("acme");
        let efforts = |id: &str| {
            models
                .iter()
                .find(|model| model.id == id)
                .map(|model| model.reasoning_efforts.clone().unwrap_or_default())
                .unwrap_or_else(|| panic!("model {id} missing"))
        };

        // Read from the fallback models-store.json.
        assert_eq!(
            efforts("gemini-4-argon"),
            vec!["low".to_string(), "medium".to_string(), "high".to_string()]
        );
        assert_eq!(efforts("gpt-5.6-luna"), vec![
            "low".to_string(),
            "medium".to_string(),
            "high".to_string(),
            "xhigh".to_string(),
            "max".to_string()
        ]);
        assert_eq!(
            efforts("glm-5.3-flash"),
            vec!["low".to_string(), "high".to_string(), "max".to_string()]
        );
        // An empty thinkingLevelMap means no effort levels.
        assert!(efforts("no-effort-model").is_empty());

        let _ = std::fs::remove_dir_all(agent);
        let _ = std::fs::remove_dir_all(support);
        let _ = std::fs::remove_dir_all(fallback);
    }

}
