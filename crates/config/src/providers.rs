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
        api: "openai-responses",
        models: &["grok-4.7", "grok-4.6", "grok-4.5", "grok-4.3"],
        env_var: "XAI_API_KEY",
    },
    BuiltinProvider {
        id: "github-copilot",
        name: "GitHub Copilot",
        base_url: "https://api.individual.githubcopilot.com",
        api: "openai-compatible",
        // Models come from the Copilot catalog (each with its own API).
        models: &[],
        env_var: "COPILOT_GITHUB_TOKEN",
    },
    BuiltinProvider {
        id: "kimi-coding",
        name: "Kimi Code",
        base_url: "https://api.kimi.com/coding/v1",
        api: "openai-compatible",
        models: &[],
        env_var: "KIMI_API_KEY",
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
    // Codex ChatGPT-login `tokens` are intentionally ignored: that OAuth
    // access token does not work against api.openai.com/v1 (Phase 2 adds
    // ChatGPT login as its own provider).
    if key == "tokens" {
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

/// Maps an OpenCode `auth.json` entry (`{ "type": "api", "key": "..." }`)
/// onto an api-key credential. OpenCode's other entry types (`oauth`,
/// `wellknown`) are not shared.
fn opencode_credential(value: &Value) -> Option<Value> {
    if value.get("type").and_then(Value::as_str) != Some("api") {
        return None;
    }
    let key = value.get("key").and_then(Value::as_str).map(str::trim).filter(|key| !key.is_empty())?;
    Some(json!({ "type": "api_key", "key": key }))
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
        // Among the fallbacks the first listed directory wins a conflict.
        let mut merged = Map::new();
        for directory in self.fallback_dirs.iter() {
            let path = directory.join("auth.json");
            match Self::read_json(&path) {
                Value::Object(map) => {
                    for (key, value) in map {
                        for (provider_id, entry) in codex_credential(&key, &value) {
                            merged.entry(provider_id).or_insert(entry);
                        }
                    }
                }
                _ => continue,
            }
        }
        // OpenCode last: it only fills providers nothing else knows.
        if let Some(Value::Object(map)) = self.opencode_dir.as_ref().map(|dir| Self::read_json(&dir.join("auth.json"))) {
            for (provider_id, entry) in map {
                if let Some(credential) = opencode_credential(&entry) {
                    merged.entry(provider_id).or_insert(credential);
                }
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

    /// Strict read of one `auth.json`: a missing file is empty, a file that is
    /// not a JSON object is an error so it is never silently overwritten.
    fn read_auth_file(path: &Path) -> Result<Map<String, Value>> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
            Err(err) => return Err(err.into()),
        };
        if raw.trim().is_empty() {
            return Ok(Map::new());
        }
        match serde_json::from_str(&raw) {
            Ok(Value::Object(map)) => Ok(map),
            _ => anyhow::bail!("{} is not a valid auth.json; fix or remove it first", path.display()),
        }
    }

    /// Read-modify-write of the PRIMARY `auth.json` only, under its lock, so
    /// fallback (`~/.codex`, `~/.pi`) credentials are never copied into it.
    fn update_primary_auth(&self, change: impl FnOnce(&mut Map<String, Value>) -> Result<()>) -> Result<()> {
        let path = self.auth_path();
        let _lock = AuthFileLock::acquire_blocking(&path)?;
        let mut map = Self::read_auth_file(&path)?;
        change(&mut map)?;
        Self::write_json(&path, &Value::Object(map))
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
            if kind == "oauth" {
                // A failed refresh is final: do not silently fall back to env keys.
                return self.resolve_oauth_access(provider_id, credential).await;
            }
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

    // ------------------------------------------------------------------
    // OAuth login & token refresh
    // ------------------------------------------------------------------

    /// Starts an OAuth login. Device-code providers return a code to show;
    /// authorization-URL providers start a loopback receiver and return the
    /// URL to open. `enterprise_domain` is only used by `github-copilot`.
    pub async fn start_oauth_login(&self, provider_id: &str, enterprise_domain: Option<&str>) -> Result<LoginSession> {
        match provider_id {
            "xai" => Ok(LoginSession::DeviceCode(self.oauth.xai.start().await?)),
            "github-copilot" => Ok(LoginSession::DeviceCode(self.oauth.copilot.start(enterprise_domain).await?)),
            "kimi-coding" => Ok(LoginSession::DeviceCode(self.oauth.kimi.start().await?)),
            "openai" => {
                let (session, server) = self.oauth.chatgpt.start(&self.device_id()?).await?;
                self.remember_login(provider_id, server);
                Ok(LoginSession::AuthUrl(session))
            }
            "openrouter" => {
                let (session, server) = self.oauth.openrouter.start().await?;
                self.remember_login(provider_id, server);
                Ok(LoginSession::AuthUrl(session))
            }
            _ => anyhow::bail!("Provider \"{provider_id}\" does not support OAuth login"),
        }
    }

    /// Waits for the user to complete the login (device approval or the
    /// loopback redirect), then stores the tokens in the primary
    /// `auth.json`. Cancel by dropping the future.
    pub async fn finish_oauth_login(&self, provider_id: &str, session: &LoginSession) -> Result<()> {
        let tokens = match session {
            LoginSession::DeviceCode(device) => self.poll_device_login(provider_id, device).await?,
            LoginSession::AuthUrl(auth) => self.await_redirect_login(provider_id, auth).await?,
        };
        self.store_oauth_tokens(provider_id, &tokens).await
    }

    /// Manual fallback of an authorization-URL login: the user pasted the
    /// redirect URL (or code) from the browser. Ends the loopback wait.
    pub async fn submit_oauth_redirect(&self, provider_id: &str, session: &AuthUrlSession, pasted: &str) -> Result<()> {
        let params = match provider_id {
            "openai" => self.oauth.chatgpt.parse_pasted_redirect(session, pasted)?,
            "openrouter" => self.oauth.openrouter.parse_pasted_redirect(pasted)?,
            _ => anyhow::bail!("Provider \"{provider_id}\" does not support pasting a redirect"),
        };
        let tokens = self.exchange_redirect(provider_id, session, &params).await?;
        self.store_oauth_tokens(provider_id, &tokens).await?;
        if let Some(server) = self.pending_logins.lock().expect("pending logins poisoned").remove(provider_id) {
            server.shutdown();
        }
        Ok(())
    }

    async fn poll_device_login(&self, provider_id: &str, session: &oauth::device_code::DeviceCodeSession) -> Result<OAuthTokens> {
        match provider_id {
            "xai" => self.oauth.xai.poll(session).await,
            "github-copilot" => self.oauth.copilot.poll(session).await,
            "kimi-coding" => self.oauth.kimi.poll(session).await,
            _ => anyhow::bail!("Provider \"{provider_id}\" does not support OAuth device login"),
        }
    }

    async fn await_redirect_login(&self, provider_id: &str, session: &AuthUrlSession) -> Result<OAuthTokens> {
        let server = self
            .pending_logins
            .lock()
            .expect("pending logins poisoned")
            .get(provider_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No sign-in is waiting for a redirect; start the login again"))?;
        let _forget_on_exit = PendingLoginGuard { manager: self, provider_id, server: server.clone() };
        let params = server.wait().await?;
        self.exchange_redirect(provider_id, session, &params).await
    }

    async fn exchange_redirect(&self, provider_id: &str, session: &AuthUrlSession, params: &CallbackParams) -> Result<OAuthTokens> {
        match provider_id {
            "openai" => self.oauth.chatgpt.exchange(session, params).await,
            "openrouter" => self.oauth.openrouter.exchange(session, params).await,
            _ => anyhow::bail!("Provider \"{provider_id}\" does not support OAuth redirect login"),
        }
    }

    /// Keeps the loopback receiver of a started login; a receiver left over
    /// from an earlier attempt is stopped.
    fn remember_login(&self, provider_id: &str, server: CallbackServer) {
        let previous = self
            .pending_logins
            .lock()
            .expect("pending logins poisoned")
            .insert(provider_id.to_string(), Arc::new(server));
        if let Some(previous) = previous {
            previous.shutdown();
        }
    }

    async fn store_oauth_tokens(&self, provider_id: &str, tokens: &OAuthTokens) -> Result<()> {
        let path = self.auth_path();
        let _lock = AuthFileLock::acquire(&path).await?;
        let mut map = Self::read_auth_file(&path)?;
        map.insert(provider_id.to_string(), tokens.to_credential());
        Self::write_json(&path, &Value::Object(map))
    }

    /// Per-installation UUID sent as the host id of "Sign in with ChatGPT";
    /// created once next to `auth.json` (`<primary dir>/device-id`).
    pub fn device_id(&self) -> Result<String> {
        let path = self.primary_dir.join("device-id");
        if let Some(existing) = std::fs::read_to_string(&path)
            .ok()
            .map(|text| text.trim().to_lowercase())
            .filter(|id| id.len() == 36 && uuid::Uuid::parse_str(id).is_ok())
        {
            return Ok(existing);
        }
        let created = uuid::Uuid::new_v4().to_string();
        std::fs::create_dir_all(&self.primary_dir)?;
        std::fs::write(&path, &created)?;
        Ok(created)
    }

    /// Access token of an OAuth credential, refreshing it when expired.
    async fn resolve_oauth_access(&self, provider_id: &str, credential: &Value) -> Option<String> {
        if let Some(access) = valid_oauth_access(credential) {
            return Some(access);
        }
        match self.refresh_oauth_credential(provider_id).await {
            Ok(access) => Some(access),
            Err(err) => {
                tracing::error!("[Providers] OAuth token refresh for {provider_id} failed: {err}");
                None
            }
        }
    }

    /// Refreshes under the lock of the `auth.json` the credential came from
    /// (primary or a fallback dir), picking up a refresh another process
    /// already did, and writes the result back to that same file.
    async fn refresh_oauth_credential(&self, provider_id: &str) -> Result<String> {
        let path = self
            .oauth_source_path(provider_id)
            .ok_or_else(|| anyhow::anyhow!("no stored OAuth credential found"))?;
        let _lock = AuthFileLock::acquire(&path).await?;
        let mut map = Self::read_auth_file(&path)?;
        let entry = map
            .get(provider_id)
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no stored OAuth credential found"))?;
        if let Some(access) = valid_oauth_access(&Value::Object(entry.clone())) {
            return Ok(access);
        }
        let refresh_token = entry
            .get("refresh")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .ok_or_else(|| anyhow::anyhow!("stored OAuth credential has no refresh token"))?;
        let tokens = self.refresh_tokens(provider_id, refresh_token, &entry).await?;
        let mut updated = entry;
        tokens.apply_to(&mut updated);
        map.insert(provider_id.to_string(), Value::Object(updated));
        Self::write_json(&path, &Value::Object(map))?;
        Ok(tokens.access)
    }

    async fn refresh_tokens(&self, provider_id: &str, refresh_token: &str, entry: &Map<String, Value>) -> Result<OAuthTokens> {
        match provider_id {
            "xai" => self.oauth.xai.refresh(refresh_token).await,
            "openai" => self.oauth.chatgpt.refresh(entry).await,
            "github-copilot" => self.oauth.copilot.refresh(entry).await,
            "kimi-coding" => self.oauth.kimi.refresh(refresh_token).await,
            "openrouter" => anyhow::bail!("OpenRouter keys do not refresh; sign in again"),
            _ => anyhow::bail!("Provider \"{provider_id}\" does not support OAuth refresh"),
        }
    }

    /// The winning OAuth credential of a provider, if it signed in with OAuth.
    fn oauth_credential(&self, provider_id: &str) -> Option<Value> {
        self.read_auth_map()
            .remove(provider_id)
            .filter(|credential| credential.get("type").and_then(Value::as_str) == Some("oauth"))
    }

    /// Chat API base URL for a Copilot access token: its `proxy-ep` host, else
    /// the enterprise or individual endpoint of the stored credential.
    pub fn copilot_base_url(&self, access_token: &str) -> String {
        let domain = self
            .oauth_credential("github-copilot")
            .and_then(|credential| credential.get("enterpriseUrl").and_then(Value::as_str).and_then(oauth::github_copilot::normalize_domain));
        oauth::github_copilot::base_url_from_token(access_token, domain.as_deref())
    }

    /// Models an OAuth login unlocks, when the provider has a fixed list:
    /// the ChatGPT chat models (`openai` OAuth only), the Copilot models the
    /// account may use, and the Kimi Code models.
    fn oauth_catalog(&self, provider_id: &str) -> Option<Vec<CatalogModel>> {
        match provider_id {
            "openai" => self.oauth_credential("openai").map(|_| catalog::CHATGPT_MODELS.to_vec()),
            "github-copilot" => {
                let allowed: Option<Vec<String>> = self
                    .oauth_credential("github-copilot")
                    .and_then(|credential| credential.get("availableModelIds").cloned())
                    .and_then(|ids| serde_json::from_value(ids).ok());
                let models = catalog::COPILOT_MODELS.iter().filter(|model| allowed.as_ref().map_or(true, |ids| ids.iter().any(|id| id == model.id)));
                Some(models.copied().collect())
            }
            "kimi-coding" => Some(catalog::KIMI_MODELS.to_vec()),
            _ => None,
        }
    }

    /// The `auth.json` holding the winning pi-style credential for a
    /// provider, in the same precedence as `read_auth_map`: primary first,
    /// then fallbacks last-to-first (a later fallback wins the merge).
    fn oauth_source_path(&self, provider_id: &str) -> Option<PathBuf> {
        std::iter::once(self.auth_path())
            .chain(self.fallback_dirs.iter().rev().map(|dir| dir.join("auth.json")))
            .find(|path| {
                Self::read_json(path)
                    .get(provider_id)
                    .map(|entry| entry.get("type").and_then(Value::as_str) == Some("oauth"))
                    .unwrap_or(false)
            })
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

    /// Provider ids to expose: everything the user configured, i.e. any
    /// `auth.json` entry, `models.json` / Codex `config.toml` provider, and
    /// built-ins whose own env var (or configured `envKey`) is set.
    fn provider_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.read_auth_map().keys().cloned().collect();
        if let Value::Object(providers) = self.read_models_json().get("providers").cloned().unwrap_or(Value::Null) {
            ids.extend(providers.keys().cloned());
        }
        ids.extend(self.codex_config_providers().keys().cloned());
        for provider in BUILTINS {
            if self.builtin_env_configured(provider) {
                ids.push(provider.id.to_string());
            }
        }
        let mut unique: Vec<String> = Vec::new();
        for id in ids {
            if !unique.contains(&id) {
                unique.push(id);
            }
        }
        unique
    }

    /// True when the built-in's own env var or its configured `envKey` is set.
    fn builtin_env_configured(&self, provider: &BuiltinProvider) -> bool {
        let env_key = self
            .models_json_provider(provider.id)
            .and_then(|entry| entry.get("envKey").and_then(Value::as_str).map(str::to_string));
        [Some(provider.env_var.to_string()), env_key]
            .into_iter()
            .flatten()
            .filter(|var| !var.is_empty())
            .any(|var| std::env::var(&var).map(|value| !value.trim().is_empty()).unwrap_or(false))
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
        // "Sign in with ChatGPT" tokens only work against the Responses API.
        if provider_id == "openai" && self.oauth_credential("openai").is_some() {
            return "openai-responses".to_string();
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
        if let Some(catalog_models) = self.oauth_catalog(provider_id) {
            models = catalog_models.into_iter().map(catalog_model_info).collect();
        }

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

    /// Providers the user can sign in to: every built-in in catalog order,
    /// then any other provider found in auth / models.json / Codex config.
    /// Statuses carry no key or token values.
    pub fn login_providers(&self) -> Vec<LoginProvider> {
        let mut ids: Vec<String> = BUILTINS.iter().map(|p| p.id.to_string()).collect();
        for id in self.provider_ids() {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        let auth = self.read_auth_map();
        ids.into_iter()
            .map(|id| LoginProvider {
                name: self.display_name(&id),
                oauth_label: oauth::oauth_provider_info(&id).map(|info| info.login_label.to_string()),
                oauth_flow: oauth::oauth_provider_info(&id).map(|info| info.flow),
                api_key_env: builtin(&id).map(|p| p.env_var).filter(|var| !var.is_empty()).map(str::to_string),
                status: self.login_status(&id, auth.get(&id)),
                id,
            })
            .collect()
    }

    fn display_name(&self, provider_id: &str) -> String {
        self.meta_entry(provider_id)
            .and_then(|entry| entry.get("displayName").and_then(Value::as_str).map(str::to_string))
            .or_else(|| builtin(provider_id).map(|p| p.name.to_string()))
            .unwrap_or_else(|| provider_id.to_string())
    }

    fn login_status(&self, provider_id: &str, credential: Option<&Value>) -> LoginStatus {
        if let Some(credential) = credential {
            let is_oauth = credential.get("type").and_then(Value::as_str) == Some("oauth");
            return if is_oauth { LoginStatus::OAuth } else { LoginStatus::ApiKey };
        }
        let env_key = self
            .models_json_provider(provider_id)
            .and_then(|entry| entry.get("envKey").and_then(Value::as_str).map(str::to_string));
        let builtin_var = builtin(provider_id).map(|p| p.env_var.to_string());
        [builtin_var, env_key]
            .into_iter()
            .flatten()
            .filter(|var| !var.is_empty())
            .find(|var| std::env::var(var).is_ok_and(|value| !value.trim().is_empty()))
            .map_or(LoginStatus::SignedOut, LoginStatus::Env)
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
                let credential = serde_json::to_value(AuthCredential::api_key(api_key))?;
                self.update_primary_auth(|auth| {
                    auth.insert(provider_id.to_string(), credential);
                    Ok(())
                })?;
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
        for id in &removed {
            if let Some(providers) = models_doc.get_mut("providers").and_then(Value::as_object_mut) {
                providers.remove(id);
            }
            if let Some(providers) = meta.get_mut("providers").and_then(Value::as_object_mut) {
                providers.remove(id);
            }
        }
        Self::write_json(&self.models_path(), &models_doc)?;
        Self::write_json(&self.meta_path(), &meta)?;
        self.update_primary_auth(|auth| {
            for id in &removed {
                auth.remove(id);
            }
            Ok(())
        })
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
                let mut status = json!({
                    "providerId": provider_id,
                    "type": kind,
                    "hasCredential": true,
                    "keyPreview": key_preview,
                });
                if kind == "oauth" {
                    // Expiry only; the access/refresh tokens never leave auth.json.
                    let expires = credential.get("expires").and_then(Value::as_i64).unwrap_or(0);
                    status["keyPreview"] = json!("oauth");
                    status["expiresAt"] = json!(expires);
                    status["expired"] = json!(expires <= oauth::now_ms());
                }
                status
            })
            .collect()
    }

    /// `POST /api/pi/auth` — set or delete a provider credential in
    /// `auth.json`.
    pub fn set_pi_auth(&self, input: &SetPiAuthInput) -> Result<()> {
        if input.provider_id.trim().is_empty() {
            anyhow::bail!("providerId is required");
        }
        let provider_id = input.provider_id.trim();
        match input.action.as_str() {
            "delete" | "remove" => self.update_primary_auth(|auth| {
                if auth.remove(provider_id).is_none() {
                    anyhow::bail!("No credential for provider \"{}\"", input.provider_id);
                }
                Ok(())
            }),
            "set" | "save" => {
                let key = input
                    .api_key
                    .as_deref()
                    .map(str::trim)
                    .filter(|k| !k.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("apiKey is required to set a credential"))?;
                let credential = serde_json::to_value(AuthCredential::api_key(key))?;
                self.update_primary_auth(|auth| {
                    auth.insert(provider_id.to_string(), credential);
                    Ok(())
                })
            }
            _ => anyhow::bail!("Invalid action. Must be 'set' or 'delete'"),
        }
    }

}

fn catalog_model_info(model: CatalogModel) -> ModelInfo {
    ModelInfo {
        id: model.id.to_string(),
        name: model.id.to_string(),
        api: Some(model.api.to_string()),
        context_window: Some(model.context_window),
        ..ModelInfo::default()
    }
}

/// Drops the loopback receiver of a login when its wait ends (success,
/// failure or cancellation), unless a newer login replaced it already.
struct PendingLoginGuard<'a> {
    manager: &'a PiProviderManager,
    provider_id: &'a str,
    server: Arc<CallbackServer>,
}

impl Drop for PendingLoginGuard<'_> {
    fn drop(&mut self) {
        let mut pending = self.manager.pending_logins.lock().expect("pending logins poisoned");
        if pending.get(self.provider_id).is_some_and(|current| Arc::ptr_eq(current, &self.server)) {
            pending.remove(self.provider_id);
        }
        self.server.shutdown();
    }
}

/// `access` of an OAuth credential whose `expires` (epoch ms) is still ahead.
fn valid_oauth_access(credential: &Value) -> Option<String> {
    let expires = credential.get("expires").and_then(Value::as_i64)?;
    if expires <= oauth::now_ms() {
        return None;
    }
    credential
        .get("access")
        .and_then(Value::as_str)
        .filter(|access| !access.is_empty())
        .map(str::to_string)
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
    async fn codex_chatgpt_tokens_are_ignored_but_openai_api_key_works() {
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

        // ChatGPT-login `tokens` alone do NOT configure openai.
        let value = json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": { "access_token": "ey-oauth-token", "refresh_token": "r", "account_id": "acc" }
        });
        PiProviderManager::write_json(&codex.join("auth.json"), &value).unwrap();
        assert!(manager.resolve_api_key("openai", None).await.is_none());
        assert!(!manager.has_api_key("openai"));
        assert!(manager.list_providers().await.is_empty());

        // A pi provider next to a ChatGPT-mode codex file lists only the pi one.
        write_auth(&agent, json!({ "opencode-go": { "type": "api_key", "key": "k-12345678" } }));
        let listed: Vec<String> = manager.list_providers().await.into_iter().map(|p| p.id).collect();
        assert_eq!(listed, vec!["opencode-go".to_string()]);

        // A non-empty OPENAI_API_KEY still maps to openai.
        let value = json!({
            "OPENAI_API_KEY": "sk-explicit-key",
            "tokens": { "access_token": "ey-oauth-token" }
        });
        PiProviderManager::write_json(&codex.join("auth.json"), &value).unwrap();
        assert_eq!(
            manager.resolve_api_key("openai", None).await.as_deref(),
            Some("sk-explicit-key")
        );

        // Null key and empty tokens configure nothing.
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
        write_auth(&agent, json!({
            "openai": { "type": "api_key", "key": "sk-openai-12345678" },
            "anthropic": { "type": "api_key", "key": "sk-ant-12345678" },
        }));
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

    // ------------------------------------------------------------------
    // OAuth credentials
    // ------------------------------------------------------------------

    use crate::oauth::device_code::PollTiming;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Mock xAI endpoints. `/token` answers refresh and device-token posts
    /// with `token_body`; `hits` counts token requests.
    async fn spawn_xai_mock(token_status: u16, token_body: Value) -> (XaiOAuth, Arc<AtomicUsize>) {
        use axum::{routing::post, Json, Router};
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let app = Router::new()
            .route(
                "/device",
                post(|| async {
                    Json(json!({
                        "device_code": "dev-secret",
                        "user_code": "WXYZ-0001",
                        "verification_uri": "https://auth.x.ai/device",
                        "interval": 0.01,
                        "expires_in": 5
                    }))
                }),
            )
            .route(
                "/token",
                post(move || {
                    let counter = counter.clone();
                    let body = token_body.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        (axum::http::StatusCode::from_u16(token_status).unwrap(), Json(body))
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let timing = PollTiming {
            default_interval: Duration::from_millis(10),
            min_interval: Duration::from_millis(5),
            slow_down_step: Duration::from_millis(10),
        };
        (XaiOAuth::with_endpoints(&format!("{base}/device"), &format!("{base}/token"), timing), hits)
    }

    use std::time::Duration;

    fn write_auth(dir: &Path, value: Value) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("auth.json"), serde_json::to_string_pretty(&value).unwrap()).unwrap();
    }

    fn read_auth(dir: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join("auth.json")).unwrap()).unwrap()
    }

    fn oauth_entry(access: &str, refresh: &str, expires: i64) -> Value {
        json!({ "type": "oauth", "access": access, "refresh": refresh, "expires": expires, "accountId": "acct-1" })
    }

    #[tokio::test]
    async fn valid_oauth_credential_resolves_without_network() {
        let tmp = tempfile::tempdir().unwrap();
        let (agent, support) = (tmp.path().join("agent"), tmp.path().join("support"));
        write_auth(&agent, json!({ "xai": oauth_entry("acc-live", "ref-live", oauth::now_ms() + 600_000) }));
        let (xai, hits) = spawn_xai_mock(500, json!({})).await;
        let manager = PiProviderManager::with_dirs(agent.clone(), support).with_xai_oauth(xai);
        assert_eq!(manager.resolve_api_key("xai", None).await.as_deref(), Some("acc-live"));
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn expired_oauth_credential_refreshes_into_its_source_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (agent, support, pi_dir) =
            (tmp.path().join("agent"), tmp.path().join("support"), tmp.path().join("pi"));
        // Credential lives only in the fallback dir; the refresh must go back there.
        write_auth(&pi_dir, json!({ "xai": oauth_entry("acc-old", "ref-old", 1), "other": { "type": "api_key", "key": "k" } }));
        // Token response omits refresh_token: the previous one is kept.
        let (xai, hits) = spawn_xai_mock(200, json!({ "access_token": "acc-new", "expires_in": 3600 })).await;
        let manager =
            PiProviderManager::with_chain(agent.clone(), support, vec![pi_dir.clone()]).with_xai_oauth(xai);

        assert_eq!(manager.resolve_api_key("xai", None).await.as_deref(), Some("acc-new"));
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        let stored = read_auth(&pi_dir);
        assert_eq!(stored["xai"]["access"], "acc-new");
        assert_eq!(stored["xai"]["refresh"], "ref-old");
        assert_eq!(stored["xai"]["accountId"], "acct-1");
        assert!(stored["xai"]["expires"].as_i64().unwrap() > oauth::now_ms());
        assert_eq!(stored["other"]["key"], "k");
        assert!(!agent.join("auth.json").exists(), "primary must stay untouched");
        assert!(!pi_dir.join("auth.json.lock").exists(), "lock dir must be released");

        // Second resolve uses the refreshed token without another request.
        assert_eq!(manager.resolve_api_key("xai", None).await.as_deref(), Some("acc-new"));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_oauth_refresh_resolves_to_none() {
        let tmp = tempfile::tempdir().unwrap();
        let (agent, support) = (tmp.path().join("agent"), tmp.path().join("support"));
        write_auth(&agent, json!({ "xai": oauth_entry("acc-old", "ref-old", 1) }));
        let (xai, _) = spawn_xai_mock(400, json!({ "error": "invalid_grant" })).await;
        let manager = PiProviderManager::with_dirs(agent.clone(), support).with_xai_oauth(xai);
        assert!(manager.resolve_api_key("xai", None).await.is_none());
        assert_eq!(read_auth(&agent)["xai"]["access"], "acc-old");
        assert!(!agent.join("auth.json.lock").exists());
    }

    #[tokio::test]
    async fn oauth_login_stores_credential_in_primary_only() {
        let tmp = tempfile::tempdir().unwrap();
        let (agent, support, pi_dir) =
            (tmp.path().join("agent"), tmp.path().join("support"), tmp.path().join("pi"));
        write_auth(&pi_dir, json!({ "other": { "type": "api_key", "key": "fallback-secret" } }));
        let (xai, _) = spawn_xai_mock(
            200,
            json!({ "access_token": "acc-login", "refresh_token": "ref-login", "expires_in": 3600 }),
        )
        .await;
        let manager =
            PiProviderManager::with_chain(agent.clone(), support, vec![pi_dir.clone()]).with_xai_oauth(xai);

        let session = manager.start_oauth_login("xai", None).await.unwrap();
        match &session {
            LoginSession::DeviceCode(device) => assert_eq!(device.user_code, "WXYZ-0001"),
            LoginSession::AuthUrl(_) => panic!("xai is a device-code login"),
        }
        manager.finish_oauth_login("xai", &session).await.unwrap();

        let primary = read_auth(&agent);
        assert_eq!(primary["xai"]["type"], "oauth");
        assert_eq!(primary["xai"]["access"], "acc-login");
        assert_eq!(primary["xai"]["refresh"], "ref-login");
        assert!(primary.get("other").is_none());
        assert!(manager.start_oauth_login("acme", None).await.is_err());
    }

    #[test]
    fn set_pi_auth_does_not_copy_fallback_credentials_into_primary() {
        let tmp = tempfile::tempdir().unwrap();
        let (agent, support, pi_dir) =
            (tmp.path().join("agent"), tmp.path().join("support"), tmp.path().join("pi"));
        write_auth(&pi_dir, json!({ "groq": { "type": "api_key", "key": "fallback-secret" } }));
        let manager = PiProviderManager::with_chain(agent.clone(), support, vec![pi_dir.clone()]);

        manager
            .set_pi_auth(&SetPiAuthInput {
                provider_id: "acme".into(),
                action: "set".into(),
                api_key: Some("sk-acme-12345678".into()),
            })
            .unwrap();
        let primary = read_auth(&agent);
        assert_eq!(primary["acme"]["key"], "sk-acme-12345678");
        assert!(primary.get("groq").is_none(), "fallback credential leaked into primary");
        assert!(!agent.join("auth.json.lock").exists());

        // The fallback credential still resolves, and its file is unchanged.
        assert!(manager.has_api_key("groq"));
        assert_eq!(read_auth(&pi_dir)["groq"]["key"], "fallback-secret");
    }

    #[test]
    fn set_pi_auth_refuses_to_overwrite_a_corrupt_auth_file() {
        let tmp = tempfile::tempdir().unwrap();
        let agent = tmp.path().join("agent");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(agent.join("auth.json"), "{ not json").unwrap();
        let manager = PiProviderManager::with_dirs(agent.clone(), tmp.path().join("support"));
        let result = manager.set_pi_auth(&SetPiAuthInput {
            provider_id: "acme".into(),
            action: "set".into(),
            api_key: Some("sk-acme-12345678".into()),
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(agent.join("auth.json")).unwrap(), "{ not json");
    }

    #[test]
    fn auth_status_for_oauth_never_contains_tokens() {
        let tmp = tempfile::tempdir().unwrap();
        let agent = tmp.path().join("agent");
        let expires = oauth::now_ms() + 600_000;
        write_auth(&agent, json!({
            "xai": oauth_entry("acc-secret-value", "ref-secret-value", expires),
            "gone": oauth_entry("acc-secret-2", "ref-secret-2", 1),
        }));
        let manager = PiProviderManager::with_dirs(agent, tmp.path().join("support"));
        let status = manager.pi_auth_status();
        let raw = serde_json::to_string(&status).unwrap();
        assert!(!raw.contains("secret"), "{raw}");
        let xai = status.iter().find(|s| s["providerId"] == "xai").unwrap();
        assert_eq!(xai["type"], "oauth");
        assert_eq!(xai["keyPreview"], "oauth");
        assert_eq!(xai["expiresAt"], expires);
        assert_eq!(xai["expired"], false);
        let gone = status.iter().find(|s| s["providerId"] == "gone").unwrap();
        assert_eq!(gone["expired"], true);
    }

    #[test]
    fn xai_builtin_uses_responses_api_with_grok_models() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = PiProviderManager::with_dirs(tmp.path().join("agent"), tmp.path().join("support"));
        assert_eq!(manager.resolve_endpoint("xai"), ("https://api.x.ai/v1".to_string(), "openai-responses".to_string()));
        assert!(manager.model_info("xai", "grok-4.7").is_some());
        assert!(manager.model_info("xai", "grok-4.3").is_some());
    }

    #[tokio::test]
    async fn list_providers_only_returns_configured_providers() {
        let _guard = ENV_VAR_TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        for var in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "DEEPSEEK_API_KEY", "GROQ_API_KEY", "XAI_API_KEY"] {
            std::env::remove_var(var);
        }
        let tmp = tempfile::tempdir().unwrap();
        let agent = tmp.path().join("agent");
        let manager = PiProviderManager::with_dirs(agent.clone(), tmp.path().join("support"));
        let ids = |providers: Vec<ProviderMetadata>| providers.into_iter().map(|p| p.id).collect::<Vec<_>>();

        // Nothing configured: no built-ins.
        assert!(ids(manager.list_providers().await).is_empty());

        // auth.json entry (api key or oauth) lists the built-in, and only it.
        write_auth(&agent, json!({
            "anthropic": { "type": "api_key", "key": "sk-ant-12345678" },
            "xai": oauth_entry("a", "r", 1),
        }));
        let listed = ids(manager.list_providers().await);
        assert!(listed.contains(&"anthropic".to_string()) && listed.contains(&"xai".to_string()));
        assert!(!listed.contains(&"openai".to_string()) && !listed.contains(&"deepseek".to_string()));

        // Only its own env var set.
        std::env::set_var("GROQ_API_KEY", "gsk-test");
        let listed = ids(manager.list_providers().await);
        std::env::remove_var("GROQ_API_KEY");
        assert!(listed.contains(&"groq".to_string()));
        assert!(!listed.contains(&"openai".to_string()));

        // A shared-chain env var of another provider does not list this one.
        std::env::set_var("OPENAI_API_KEY", "sk-test");
        let listed = ids(manager.list_providers().await);
        std::env::remove_var("OPENAI_API_KEY");
        assert!(listed.contains(&"openai".to_string()));
        assert!(!listed.contains(&"deepseek".to_string()));

        // models.json custom provider.
        std::fs::write(
            agent.join("models.json"),
            json!({ "providers": { "my-local": { "baseUrl": "http://localhost:1/v1" } } }).to_string(),
        )
        .unwrap();
        assert!(ids(manager.list_providers().await).contains(&"my-local".to_string()));
    }

    #[test]
    fn login_providers_report_status_without_secrets() {
        let _guard = ENV_VAR_TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        for provider in BUILTINS {
            if !provider.env_var.is_empty() {
                std::env::remove_var(provider.env_var);
            }
        }
        std::env::set_var("GROQ_API_KEY", "gsk-secret-env");
        std::env::set_var("ACME_TEST_KEY", "acme-secret-env");
        let tmp = tempfile::tempdir().unwrap();
        let primary = tmp.path().join("primary");
        let fallback = tmp.path().join("fallback");
        write_auth(&primary, json!({
            "anthropic": { "type": "api_key", "key": "sk-secret-key" },
            "xai": oauth_entry("acc-secret", "ref-secret", oauth::now_ms() + 600_000),
        }));
        std::fs::write(
            primary.join("models.json"),
            json!({ "providers": { "acme": { "baseUrl": "http://localhost:1/v1", "envKey": "ACME_TEST_KEY" } } }).to_string(),
        )
        .unwrap();
        let manager = PiProviderManager::with_chain(primary, tmp.path().join("support"), vec![fallback]);
        let rows = manager.login_providers();
        std::env::remove_var("GROQ_API_KEY");
        std::env::remove_var("ACME_TEST_KEY");

        let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(
            &ids[..11],
            ["opencode-go", "anthropic", "openai", "google", "openrouter", "deepseek", "groq", "xai", "github-copilot", "kimi-coding", "ollama"]
        );
        assert_eq!(ids[11], "acme");
        let row = |id: &str| rows.iter().find(|row| row.id == id).unwrap();
        assert_eq!(row("anthropic").status, LoginStatus::ApiKey);
        assert_eq!(row("xai").status, LoginStatus::OAuth);
        assert_eq!(row("groq").status, LoginStatus::Env("GROQ_API_KEY".into()));
        assert_eq!(row("acme").status, LoginStatus::Env("ACME_TEST_KEY".into()));
        assert_eq!(row("openai").status, LoginStatus::SignedOut);
        assert!(row("xai").oauth_label.is_some() && row("groq").oauth_label.is_none());
        assert_eq!(row("openai").oauth_flow, Some(oauth::OAuthFlow::AuthUrl));
        assert_eq!(row("github-copilot").oauth_flow, Some(oauth::OAuthFlow::DeviceCode));
        assert_eq!(row("ollama").api_key_env, None);

        let raw = serde_json::to_string(&rows).unwrap();
        assert!(!raw.contains("secret"), "{raw}");
    }

    // ------------------------------------------------------------------
    // ChatGPT / Copilot / OpenRouter / Kimi / OpenCode through the manager
    // ------------------------------------------------------------------

    use crate::oauth::openai::ChatgptOAuth;
    use crate::oauth::openrouter::OpenRouterOAuth;

    /// Mock answering `/token` (form) and `/keys` (JSON) with `(status, body)`.
    async fn spawn_token_mock(status: u16, body: Value) -> String {
        use axum::{routing::post, Router};
        let answer = move || {
            let body = body.clone();
            async move { (axum::http::StatusCode::from_u16(status).unwrap(), axum::Json(body)) }
        };
        let app = Router::new().route("/token", post(answer.clone())).route("/keys", post(answer));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base
    }

    fn chatgpt_grant() -> Value {
        json!({ "id_token": "idt", "access_token": "acc-gpt", "refresh_token": "ref-gpt", "scope": "openid chatgpt.tokens.use.direct", "expires_in": 3600 })
    }

    fn chatgpt_clients(base: &str) -> OAuthClients {
        OAuthClients {
            chatgpt: ChatgptOAuth::with_endpoints("https://auth.example.test/authorize", &format!("{base}/token"), 0),
            ..OAuthClients::default()
        }
    }

    #[tokio::test]
    async fn chatgpt_login_over_loopback_unlocks_the_responses_models() {
        let tmp = tempfile::tempdir().unwrap();
        let (agent, support) = (tmp.path().join("agent"), tmp.path().join("support"));
        let base = spawn_token_mock(200, chatgpt_grant()).await;
        let manager = PiProviderManager::with_dirs(agent.clone(), support).with_oauth_clients(chatgpt_clients(&base));
        assert_eq!(manager.resolve_api("openai"), "openai-compatible", "API-key behavior before sign-in");

        let LoginSession::AuthUrl(session) = manager.start_oauth_login("openai", None).await.unwrap() else {
            panic!("ChatGPT sign-in is an authorization-URL login");
        };
        assert!(session.auth_url.contains("agent_name_hint=Locagens"));
        let redirect = format!("{}?code=c1&state={}&client_id=issued", session.redirect_uri, session.state.clone().unwrap());
        let login = LoginSession::AuthUrl(session);
        let finish = manager.finish_oauth_login("openai", &login);
        let browser = async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            reqwest::get(redirect).await.unwrap().status()
        };
        let (result, browser_status) = tokio::join!(finish, browser);
        result.unwrap();
        assert_eq!(browser_status, 200);

        let stored = read_auth(&agent);
        assert_eq!(stored["openai"]["type"], "oauth");
        assert_eq!(stored["openai"]["clientId"], "issued");
        assert_eq!(manager.resolve_api_key("openai", None).await.as_deref(), Some("acc-gpt"));
        assert!(manager.pending_logins.lock().unwrap().is_empty(), "receiver released");
        assert_eq!(manager.device_id().unwrap().len(), 36);

        assert_eq!(manager.resolve_api("openai"), "openai-responses");
        let model = manager.model_info("openai", "gpt-5.5").unwrap();
        assert_eq!(model.api.as_deref(), Some("openai-responses"));
        assert_eq!(model.context_window, Some(272_000));
        assert!(manager.model_info("openai", "o4-mini").is_some());
        assert_eq!(manager.merged_models("openai").len(), 44);
    }

    #[tokio::test]
    async fn pasted_redirect_completes_openrouter_login_and_stops_the_receiver() {
        let tmp = tempfile::tempdir().unwrap();
        let (agent, support) = (tmp.path().join("agent"), tmp.path().join("support"));
        let base = spawn_token_mock(200, json!({ "key": "sk-or-secret" })).await;
        let clients = OAuthClients {
            openrouter: OpenRouterOAuth::with_endpoints("https://openrouter.example.test/auth", &format!("{base}/keys")),
            ..OAuthClients::default()
        };
        let manager = PiProviderManager::with_dirs(agent.clone(), support).with_oauth_clients(clients);
        let LoginSession::AuthUrl(session) = manager.start_oauth_login("openrouter", None).await.unwrap() else {
            panic!("OpenRouter sign-in is an authorization-URL login");
        };
        manager.submit_oauth_redirect("openrouter", &session, "paste-code").await.unwrap();

        let stored = read_auth(&agent);
        assert_eq!(stored["openrouter"], json!({ "type": "oauth", "access": "sk-or-secret", "refresh": "", "expires": 9_007_199_254_740_991_i64 }));
        assert_eq!(manager.resolve_api_key("openrouter", None).await.as_deref(), Some("sk-or-secret"));
        assert!(manager.pending_logins.lock().unwrap().is_empty());
        assert!(manager.submit_oauth_redirect("groq", &session, "x").await.is_err());
    }

    #[tokio::test]
    async fn chatgpt_refresh_dispatches_with_the_stored_client_id() {
        let tmp = tempfile::tempdir().unwrap();
        let (agent, support) = (tmp.path().join("agent"), tmp.path().join("support"));
        let mut entry = oauth_entry("acc-old", "ref-old", 1);
        entry["clientId"] = json!("issued");
        write_auth(&agent, json!({ "openai": entry }));
        let base = spawn_token_mock(200, chatgpt_grant()).await;
        let manager = PiProviderManager::with_dirs(agent.clone(), support).with_oauth_clients(chatgpt_clients(&base));
        assert_eq!(manager.resolve_api_key("openai", None).await.as_deref(), Some("acc-gpt"));
        let stored = read_auth(&agent);
        assert_eq!(stored["openai"]["refresh"], "ref-gpt");
        assert_eq!(stored["openai"]["clientId"], "issued");
        assert_eq!(stored["openai"]["accountId"], "acct-1", "unrelated fields survive a refresh");

        // Without the issued client id the credential cannot refresh.
        write_auth(&agent, json!({ "openai": oauth_entry("acc-old", "ref-old", 1) }));
        assert_eq!(manager.resolve_api_key("openai", None).await, None);
    }

    #[tokio::test]
    async fn openrouter_keys_never_refresh() {
        let tmp = tempfile::tempdir().unwrap();
        let (agent, support) = (tmp.path().join("agent"), tmp.path().join("support"));
        let permanent = json!({ "type": "oauth", "access": "sk-or-1", "refresh": "", "expires": 9_007_199_254_740_991_i64 });
        write_auth(&agent, json!({ "openrouter": permanent }));
        let manager = PiProviderManager::with_dirs(agent.clone(), support);
        assert_eq!(manager.resolve_api_key("openrouter", None).await.as_deref(), Some("sk-or-1"));
        assert!(manager.refresh_tokens("openrouter", "", &Map::new()).await.is_err());
    }

    #[test]
    fn device_id_is_created_once_and_reused() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = PiProviderManager::with_dirs(tmp.path().join("agent"), tmp.path().join("support"));
        let first = manager.device_id().unwrap();
        assert_eq!(manager.device_id().unwrap(), first);
        assert!(uuid::Uuid::parse_str(&first).is_ok());
        std::fs::write(tmp.path().join("agent").join("device-id"), "garbage").unwrap();
        assert_ne!(manager.device_id().unwrap(), "garbage");
    }

    #[tokio::test]
    async fn opencode_api_keys_are_the_lowest_precedence_read_only_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let (agent, support, pi_dir, opencode) =
            (tmp.path().join("agent"), tmp.path().join("support"), tmp.path().join("pi"), tmp.path().join("opencode"));
        write_auth(&opencode, json!({
            "opencode-go": { "type": "api", "key": "oc-go-key" },
            "huggingface": { "type": "api", "key": "oc-hf-key" },
            "blank": { "type": "api", "key": "  " },
            "mixed-oauth": { "type": "oauth", "access": "oc-acc", "refresh": "oc-ref", "expires": 1 },
            "wellknown-co": { "type": "wellknown", "key": "X", "token": "t" },
        }));
        write_auth(&pi_dir, json!({ "opencode-go": { "type": "api_key", "key": "pi-key" } }));
        write_auth(&agent, json!({ "groq": { "type": "api_key", "key": "primary-key" } }));
        let manager = PiProviderManager::with_chain(agent.clone(), support, vec![pi_dir]).with_opencode_dir(opencode.clone());

        assert_eq!(manager.resolve_api_key("opencode-go", None).await.as_deref(), Some("pi-key"), "pi overrides OpenCode");
        assert_eq!(manager.resolve_api_key("huggingface", None).await.as_deref(), Some("oc-hf-key"));
        assert_eq!(manager.read_auth_map()["huggingface"], json!({ "type": "api_key", "key": "oc-hf-key" }));
        for ignored in ["blank", "mixed-oauth", "wellknown-co"] {
            assert!(!manager.has_api_key(ignored), "{ignored}");
        }
        assert_eq!(manager.resolve_api_key("groq", None).await.as_deref(), Some("primary-key"));

        // Saving a key writes the primary file only; OpenCode's file is untouched.
        let before = std::fs::read_to_string(opencode.join("auth.json")).unwrap();
        manager
            .set_pi_auth(&SetPiAuthInput { provider_id: "huggingface".into(), action: "set".into(), api_key: Some("new-key-12345".into()) })
            .unwrap();
        assert_eq!(manager.resolve_api_key("huggingface", None).await.as_deref(), Some("new-key-12345"));
        assert_eq!(std::fs::read_to_string(opencode.join("auth.json")).unwrap(), before);
        assert!(read_auth(&agent).get("mixed-oauth").is_none());
    }

    #[tokio::test]
    async fn copilot_credential_narrows_models_and_picks_the_base_url() {
        let tmp = tempfile::tempdir().unwrap();
        let (agent, support) = (tmp.path().join("agent"), tmp.path().join("support"));
        let manager = PiProviderManager::with_dirs(agent.clone(), support);
        assert_eq!(manager.merged_models("github-copilot").len(), 27, "everything before sign-in");

        let mut entry = oauth_entry("tid=1;proxy-ep=proxy.business.githubcopilot.com", "gho-ref", oauth::now_ms() + 600_000);
        entry["availableModelIds"] = json!(["claude-sonnet-5", "gpt-6-sol", "gemini-4-argon"]);
        write_auth(&agent, json!({ "github-copilot": entry }));
        let ids: Vec<String> = manager.merged_models("github-copilot").into_iter().map(|model| model.id).collect();
        assert_eq!(ids, ["claude-sonnet-5", "gemini-4-argon", "gpt-6-sol"]);
        assert_eq!(manager.model_info("github-copilot", "claude-sonnet-5").unwrap().api.as_deref(), Some("anthropic-messages"));
        assert_eq!(manager.model_info("github-copilot", "gemini-4-argon").unwrap().api.as_deref(), Some("openai-completions"));
        assert_eq!(manager.model_info("github-copilot", "gpt-6-sol").unwrap().api.as_deref(), Some("openai-responses"));
        assert_eq!(
            manager.copilot_base_url("tid=1;proxy-ep=proxy.business.githubcopilot.com"),
            "https://api.business.githubcopilot.com"
        );

        let mut enterprise = oauth_entry("tid=1", "gho-ref", oauth::now_ms() + 600_000);
        enterprise["enterpriseUrl"] = json!("co.ghe.com");
        write_auth(&agent, json!({ "github-copilot": enterprise }));
        assert_eq!(manager.copilot_base_url("tid=1"), "https://copilot-api.co.ghe.com");
    }

    #[test]
    fn kimi_coding_builtin_serves_the_subscription_models() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = PiProviderManager::with_dirs(tmp.path().join("agent"), tmp.path().join("support"));
        assert_eq!(
            manager.resolve_endpoint("kimi-coding"),
            ("https://api.kimi.com/coding/v1".to_string(), "openai-compatible".to_string())
        );
        let model = manager.model_info("kimi-coding", "kimi-for-coding").unwrap();
        assert_eq!(model.context_window, Some(1_048_576));
        assert_eq!(manager.merged_models("kimi-coding").len(), 6);
    }

    #[test]
    fn auth_status_for_the_new_oauth_providers_never_contains_secrets() {
        let tmp = tempfile::tempdir().unwrap();
        let agent = tmp.path().join("agent");
        let expires = oauth::now_ms() + 600_000;
        let mut chatgpt = oauth_entry("gpt-access-secret", "gpt-refresh-secret", expires);
        chatgpt["clientId"] = json!("client-secret-id");
        let mut copilot = oauth_entry("tid=copilot-secret", "gho-github-secret", expires);
        copilot["enterpriseUrl"] = json!("co.ghe.com");
        write_auth(&agent, json!({
            "openai": chatgpt,
            "github-copilot": copilot,
            "openrouter": { "type": "oauth", "access": "sk-or-secret", "refresh": "", "expires": 9_007_199_254_740_991_i64 },
            "kimi-coding": oauth_entry("kimi-access-secret", "kimi-refresh-secret", expires),
        }));
        let manager = PiProviderManager::with_dirs(agent, tmp.path().join("support"));
        let raw = serde_json::to_string(&manager.pi_auth_status()).unwrap() + &serde_json::to_string(&manager.login_providers()).unwrap();
        assert!(!raw.contains("secret"), "{raw}");
        assert!(raw.contains("github-copilot") && raw.contains("kimi-coding"));
    }
}
