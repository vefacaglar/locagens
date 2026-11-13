use super::*;

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

pub(super) struct BuiltinProvider {
    id: &'static str,
    name: &'static str,
    base_url: &'static str,
    api: &'static str,
    models: &'static [&'static str],
    pub(super) env_var: &'static str,
}

/// Built-in default provider definitions (used when not overridden in
/// `models.json` or `models-store.json`).
pub(super) const BUILTINS: &[BuiltinProvider] = &[
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

pub(super) fn builtin(id: &str) -> Option<&'static BuiltinProvider> {
    BUILTINS.iter().find(|provider| provider.id == id)
}

/// True when a provider talks the Anthropic Messages API.
pub(super) fn is_anthropic(provider_id: &str, api: &str, base_url: &str) -> bool {
    api == "anthropic"
        || api == "anthropic-messages"
        || provider_id == "anthropic"
        || base_url.contains("anthropic")
}

/// Web-client provider type for a resolved `api` kind: `anthropic-messages`
/// (and friends) map to `"anthropic"`, the Codex Responses WireAPI
/// (`openai-responses` / `responses`) keeps its own kind, and everything else
/// falls back to `"openai-compatible"`.
pub(super) fn provider_type(api: &str) -> &'static str {
    if api.contains("anthropic") {
        "anthropic"
    } else if api == "openai-responses" || api == "responses" {
        "openai-responses"
    } else {
        "openai-compatible"
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

impl PiProviderManager {
    fn read_models_json(&self) -> Value {
        Self::read_json(&self.models_path())
    }

    /// `models.json` entry for a provider (`providers.<id>`), if any. Falls
    /// back to fallback-chain `models.json` files and to Codex's
    /// `config.toml` `[model_providers.*]` entries.
    pub(super) fn models_json_provider(&self, provider_id: &str) -> Option<Map<String, Value>> {
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

    /// Models for a provider cached in `pi`'s `models-store.json` — fallback
    /// directories first (oldest to newest), then the primary agent dir whose
    /// entries win for matching model ids.
    pub(super) fn models_store_models(&self, provider_id: &str) -> Vec<ModelInfo> {
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
    pub(super) fn resolve_base_url(&self, provider_id: &str) -> String {
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
    pub(super) fn resolve_api(&self, provider_id: &str) -> String {
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
    pub(super) fn merged_models(&self, provider_id: &str) -> Vec<ModelInfo> {
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

    /// All configured / authenticated providers, in
    /// the UI's `ProviderMetadata[]` contract shape.
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

    /// Editable configs with masked keys.
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

    /// Accepts the whole
    /// `Record<providerId, ProviderConfig>` map the UI sends:
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
}
