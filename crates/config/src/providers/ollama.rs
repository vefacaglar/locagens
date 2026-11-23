//! Native Ollama discovery, host/URL normalization, and metadata matching.

use super::model_catalog::{model_reasoning_efforts, BUILTINS};
use super::model_refresh::{is_chat_model, CachedModelMetadata};
use super::PiProviderManager;
use crate::oauth::catalog::{CatalogModel, CHATGPT_MODELS, COPILOT_MODELS, KIMI_MODELS};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

const OLLAMA_TAGS_TIMEOUT: Duration = Duration::from_secs(2);
const OLLAMA_SHOW_TIMEOUT: Duration = Duration::from_secs(3);
const OLLAMA_SHOW_CONCURRENCY: usize = 8;

/// Discovered Ollama chat model with resolved metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DiscoveredOllamaModel {
    pub id: String,
    pub metadata: CachedModelMetadata,
}

/// True when a provider requires no API key: `"ollama"`, `"apple-fm"`, or
/// loopback URLs (`localhost`, `*.localhost`, `127.0.0.1`, `::1`, `0.0.0.0`).
pub fn is_keyless_provider(provider_id: &str, base_url: &str) -> bool {
    let provider_id = provider_id.trim();
    if provider_id.eq_ignore_ascii_case("ollama")
        || provider_id.eq_ignore_ascii_case(super::APPLE_FM_PROVIDER_ID)
    {
        return true;
    }
    let Ok(url) = reqwest::Url::parse(base_url.trim()) else {
        return false;
    };
    let Some(raw_host) = url.host_str() else {
        return false;
    };
    let host = raw_host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".localhost") {
        return true;
    }
    if host == "0.0.0.0" {
        return true;
    }
    host.parse::<IpAddr>()
        .is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified())
}

/// Normalizes `OLLAMA_BASE_URL` / `OLLAMA_HOST` values into an OpenAI-compatible
/// `.../v1` URL without duplicating `/v1`.
pub(super) fn normalize_ollama_base_url(
    raw: &str,
    default_port_when_missing: bool,
) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let candidate = if raw.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("http://[{raw}]")
    } else if raw.starts_with(':') {
        format!("http://localhost{raw}")
    } else if !raw.contains("://") {
        if raw.matches(':').count() > 1 && !raw.starts_with('[') {
            format!("http://[{raw}]")
        } else {
            format!("http://{raw}")
        }
    } else {
        raw.to_string()
    };

    let mut url = reqwest::Url::parse(&candidate).ok()?;
    if default_port_when_missing
        && url.scheme() == "http"
        && url.port().is_none()
        && (url.path().is_empty() || url.path() == "/")
    {
        let _ = url.set_port(Some(11434));
    }
    let trimmed_path = url.path().trim_end_matches('/');
    let next_path = if trimmed_path.is_empty() {
        "/v1".to_string()
    } else if trimmed_path == "/v1" || trimmed_path.ends_with("/v1") {
        trimmed_path.to_string()
    } else {
        format!("{trimmed_path}/v1")
    };
    url.set_path(&next_path);
    Some(url.to_string().trim_end_matches('/').to_string())
}

pub(super) fn ollama_env_configured() -> bool {
    ["OLLAMA_BASE_URL", "OLLAMA_HOST"]
        .into_iter()
        .any(|var| std::env::var(var).is_ok_and(|val| !val.trim().is_empty()))
}

/// Derives the native Ollama API root from an OpenAI-compatible `base_url`
/// by stripping trailing `/` and `/v1`.
pub(super) fn native_api_root(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    trimmed
        .strip_suffix("/v1")
        .map(|prefix| prefix.trim_end_matches('/'))
        .unwrap_or(trimmed)
        .to_string()
}

pub(super) fn format_ollama_display_name(model_id: &str) -> String {
    model_id
        .strip_suffix(":latest")
        .unwrap_or(model_id)
        .to_string()
}

#[derive(Clone, Debug)]
struct TagModelEntry {
    id: String,
    capabilities: Vec<String>,
}

fn parse_capabilities(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_tags_response(body: &Value) -> Vec<TagModelEntry> {
    let Some(models) = body.get("models").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for entry in models {
        let Some(id) = entry
            .get("name")
            .or_else(|| entry.get("model"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if !seen.insert(id.to_string()) {
            continue;
        }
        entries.push(TagModelEntry {
            id: id.to_string(),
            capabilities: parse_capabilities(entry.get("capabilities")),
        });
    }
    entries
}

fn parse_openai_models_response(body: &Value) -> Vec<String> {
    let entries = match body {
        Value::Array(items) => items.as_slice(),
        Value::Object(_) => body
            .get("data")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]),
        _ => &[],
    };
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for entry in entries {
        let id = match entry {
            Value::String(s) => s.trim(),
            _ => entry
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default(),
        };
        if !id.is_empty() && seen.insert(id.to_string()) {
            ids.push(id.to_string());
        }
    }
    ids
}

fn extract_context_length(show: &Value) -> Option<i64> {
    let model_info = show.get("model_info")?.as_object()?;
    model_info
        .iter()
        .filter(|(key, _)| key.ends_with(".context_length"))
        .filter_map(|(_, value)| {
            value
                .as_i64()
                .or_else(|| value.as_u64().and_then(|u| i64::try_from(u).ok()))
        })
        .filter(|&len| len > 0)
        .max()
}

fn parse_show_thinking(show: &Value) -> Option<bool> {
    let thinking = show.get("thinking")?;
    match thinking {
        Value::Bool(enabled) => Some(*enabled),
        Value::Object(map) => {
            if let Some(values) = map.get("values").and_then(Value::as_array) {
                if values.is_empty() {
                    return None;
                }
                let supports_thinking = values.iter().any(|item| match item {
                    Value::Bool(b) => *b,
                    Value::String(s) => {
                        let norm = s.trim().to_ascii_lowercase();
                        !norm.is_empty() && !matches!(norm.as_str(), "false" | "off" | "none")
                    }
                    _ => false,
                });
                return Some(supports_thinking);
            }
            map.get("default").and_then(Value::as_bool)
        }
        _ => None,
    }
}

fn is_embedding_only(capabilities: &HashSet<String>) -> bool {
    capabilities.contains("embedding") && !capabilities.contains("completion")
}

pub(super) fn build_ollama_model(
    id: &str,
    tag_caps: &[String],
    show: Option<&Value>,
) -> Option<DiscoveredOllamaModel> {
    let mut capabilities: HashSet<String> = tag_caps.iter().cloned().collect();
    if let Some(show_val) = show {
        capabilities.extend(parse_capabilities(show_val.get("capabilities")));
    }

    if is_embedding_only(&capabilities) || !is_chat_model(id) {
        return None;
    }

    let mut context_window = show.and_then(extract_context_length);
    let mut reasoning = if capabilities.contains("thinking") {
        Some(true)
    } else {
        show.and_then(parse_show_thinking)
    };

    if context_window.is_none() || reasoning.is_none() {
        let fallback = fallback_match_metadata(id);
        if context_window.is_none() {
            context_window = fallback.context_window;
        }
        if reasoning.is_none() {
            reasoning = fallback.reasoning;
        }
    }

    Some(DiscoveredOllamaModel {
        id: id.to_string(),
        metadata: CachedModelMetadata {
            name: Some(format_ollama_display_name(id)),
            context_window,
            max_tokens: None,
            reasoning,
        },
    })
}

/// Normalizes an Ollama model id for catalog/family matching:
/// strips registry/namespace prefix, drops `:tag`, lowercases, replaces `_`
/// with `-`, and normalizes common punctuation/version variants.
fn normalize_ollama_name(id: &str) -> String {
    let bare = id.trim().rsplit('/').next().unwrap_or(id.trim());
    let tagless = bare.split(':').next().unwrap_or(bare);
    let mut norm = tagless.to_ascii_lowercase().replace('_', "-");
    for suffix in ["-gguf", "-mlx", "-nvfp4", "-instruct", "-chat"] {
        if let Some(stripped) = norm.strip_suffix(suffix) {
            norm = stripped.to_string();
        }
    }
    norm
}

/// Canonicalizes minor separator differences like `gemma-4` <-> `gemma4` or
/// `v2-6` <-> `v2.6` for fuzzy matching against `BUILTINS` / `CatalogModel`s.
fn compact_key(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect()
}

fn all_catalog_models() -> impl Iterator<Item = &'static CatalogModel> {
    CHATGPT_MODELS
        .iter()
        .chain(COPILOT_MODELS.iter())
        .chain(KIMI_MODELS.iter())
}

#[derive(Clone, Copy, Default)]
struct FallbackMetadata {
    context_window: Option<i64>,
    reasoning: Option<bool>,
}

fn matches_family_prefix(normalized: &str, compact: &str, prefix: &str) -> bool {
    if normalized == prefix
        || normalized.starts_with(&format!("{prefix}-"))
        || normalized.starts_with(&format!("{prefix}."))
    {
        return true;
    }
    let cp = compact_key(prefix);
    if let Some(rest) = compact.strip_prefix(&cp) {
        return rest.is_empty()
            || !cp.ends_with(|c: char| c.is_ascii_digit())
            || !rest.starts_with(|c: char| c.is_ascii_digit());
    }
    false
}

pub(super) fn fallback_match_metadata(model_id: &str) -> CachedModelMetadata {
    let normalized = normalize_ollama_name(model_id);
    let compact = compact_key(&normalized);
    let mut matched = FallbackMetadata::default();

    // 1. Match against curated CatalogModels (provides exact context_window).
    if let Some(catalog) = all_catalog_models().find(|entry| {
        let entry_norm = entry.id.to_ascii_lowercase();
        entry_norm == normalized || compact_key(&entry_norm) == compact
    }) {
        matched.context_window = Some(catalog.context_window);
    }

    // 2. Match reasoning capability via `BUILTINS` and `model_reasoning_efforts`.
    let matched_builtin = BUILTINS.iter().flat_map(|p| p.models.iter()).find(|id| {
        let b = id.to_ascii_lowercase();
        let bc = compact_key(&b);
        b == normalized || bc == compact || b.starts_with(&format!("{normalized}-"))
    });
    if let Some(builtin_id) = matched_builtin {
        if !model_reasoning_efforts(builtin_id, None, None).is_empty() {
            matched.reasoning = Some(true);
        }
    }
    if matched.reasoning.is_none() && !model_reasoning_efforts(&normalized, None, None).is_empty() {
        matched.reasoning = Some(true);
    }

    // 3. Family defaults for common local open-weights models.
    const FAMILIES: [(&[&str], i64, Option<bool>); 20] = [
        (&["gemma4", "gemma-4"], 262_144, Some(true)),
        (&["gemma3", "gemma-3"], 131_072, Some(false)),
        (&["gemma2", "gemma-2"], 8_192, Some(false)),
        (&["gemma"], 8_192, Some(false)),
        (&["mimo-v2.6", "mimo-v2-6", "mimo-v2"], 262_144, Some(true)),
        (&["qwen3.5", "qwen3-5"], 262_144, Some(true)),
        (&["qwen3-coder"], 262_144, Some(false)),
        (&["qwen3"], 131_072, Some(true)),
        (&["qwen4"], 262_144, Some(true)),
        (&["qwq"], 32_768, Some(true)),
        (&["qwen2.5", "qwen2-5"], 32_768, Some(false)),
        (&["deepseek-r1", "deepseek-reasoner", "deepseek-v4"], 131_072, Some(true)),
        (&["deepseek-v3", "deepseek-coder"], 131_072, Some(false)),
        (&["gpt-oss"], 131_072, Some(true)),
        (&["glm-5"], 131_072, Some(true)),
        (&["glm-4"], 131_072, None),
        (&["llama4", "llama-4", "llama3.3", "llama-3.3", "llama3.2", "llama-3.2", "llama3.1", "llama-3.1"], 131_072, Some(false)),
        (&["llama3", "llama-3"], 8_192, Some(false)),
        (&["devstral", "ministral"], 131_072, Some(false)),
        (&["mistral", "codestral", "mixtral"], 32_768, Some(false)),
    ];

    for (prefixes, ctx, reasoning) in FAMILIES {
        if prefixes
            .iter()
            .any(|prefix| matches_family_prefix(&normalized, &compact, prefix))
        {
            if matched.context_window.is_none() {
                matched.context_window = Some(ctx);
            }
            if matched.reasoning.is_none() {
                matched.reasoning = reasoning;
            }
            break;
        }
    }

    if matched.context_window.is_none()
        && (normalized.starts_with("kimi") || normalized == "k3" || normalized.starts_with("k3."))
    {
        matched.context_window = Some(262_144);
        if matched.reasoning.is_none() {
            matched.reasoning = Some(true);
        }
    }
    if matched.context_window.is_none()
        && (normalized.starts_with("phi4") || normalized.starts_with("phi-4"))
    {
        matched.context_window = Some(16_384);
    }

    CachedModelMetadata {
        name: Some(format_ollama_display_name(model_id)),
        context_window: matched.context_window,
        max_tokens: None,
        reasoning: matched.reasoning,
    }
}

impl PiProviderManager {
    /// Discovers installed Ollama chat models using native `/api/tags` +
    /// `/api/show`, falling back to OpenAI-compatible `GET {baseUrl}/models`
    /// when `/api/tags` returns 404.
    pub(super) async fn discover_ollama_models(
        &self,
        base_url: &str,
        api_key: Option<&str>,
    ) -> Result<Vec<DiscoveredOllamaModel>> {
        let base_url = base_url.trim().trim_end_matches('/');
        let root = native_api_root(base_url);
        let auth_key = api_key.map(str::trim).filter(|k| !k.is_empty());

        let mut tags_req = self
            .http
            .get(format!("{root}/api/tags"))
            .timeout(OLLAMA_TAGS_TIMEOUT);
        if let Some(key) = auth_key {
            tags_req = tags_req.bearer_auth(key);
        }
        let tags_res = tags_req.send().await?;
        let tags_status = tags_res.status();

        if tags_status == reqwest::StatusCode::NOT_FOUND {
            // OpenAI-compatible proxy fallback when native `/api/tags` is absent.
            let mut models_req = self
                .http
                .get(format!("{base_url}/models"))
                .timeout(OLLAMA_TAGS_TIMEOUT);
            if let Some(key) = auth_key {
                models_req = models_req.bearer_auth(key);
            }
            let models_res = models_req.send().await?;
            let status = models_res.status();
            let body: Value = models_res.json().await.unwrap_or(Value::Null);
            if !status.is_success() {
                anyhow::bail!("Ollama model fetch failed with status {status}");
            }
            return Ok(parse_openai_models_response(&body)
                .into_iter()
                .filter(|id| is_chat_model(id))
                .map(|id| DiscoveredOllamaModel {
                    metadata: fallback_match_metadata(&id),
                    id,
                })
                .collect());
        }

        if !tags_status.is_success() {
            anyhow::bail!("Ollama /api/tags failed with status {tags_status}");
        }

        let tags_body: Value = tags_res.json().await?;
        let entries = parse_tags_response(&tags_body);
        if entries.is_empty() {
            return Ok(Vec::new());
        }

        let semaphore = Arc::new(Semaphore::new(OLLAMA_SHOW_CONCURRENCY));
        let mut join_set = JoinSet::new();
        let show_url = format!("{root}/api/show");
        let auth_owned = auth_key.map(str::to_string);

        for (index, entry) in entries.iter().enumerate() {
            let permit = semaphore.clone().acquire_owned().await?;
            let client = self.http.clone();
            let url = show_url.clone();
            let model_name = entry.id.clone();
            let key = auth_owned.clone();
            join_set.spawn(async move {
                let _permit = permit;
                let mut req = client
                    .post(&url)
                    .timeout(OLLAMA_SHOW_TIMEOUT)
                    .json(&json!({ "name": model_name }));
                if let Some(k) = key.as_deref() {
                    req = req.bearer_auth(k);
                }
                let show = match req.send().await {
                    Ok(resp) if resp.status().is_success() => resp.json::<Value>().await.ok(),
                    _ => None,
                };
                (index, show)
            });
        }

        let mut shows: Vec<Option<Value>> = vec![None; entries.len()];
        while let Some(joined) = join_set.join_next().await {
            if let Ok((index, show)) = joined {
                if index < shows.len() {
                    shows[index] = show;
                }
            }
        }

        let discovered = entries
            .into_iter()
            .zip(shows)
            .filter_map(|(entry, show)| {
                build_ollama_model(&entry.id, &entry.capabilities, show.as_ref())
            })
            .collect();

        Ok(discovered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_ollama_hosts_and_base_urls() {
        assert_eq!(
            normalize_ollama_base_url("127.0.0.1:11434", true).as_deref(),
            Some("http://127.0.0.1:11434/v1")
        );
        assert_eq!(
            normalize_ollama_base_url("mac-studio.local", true).as_deref(),
            Some("http://mac-studio.local:11434/v1")
        );
        assert_eq!(
            normalize_ollama_base_url("http://host:11434", true).as_deref(),
            Some("http://host:11434/v1")
        );
        assert_eq!(
            normalize_ollama_base_url("http://host:11434/v1", true).as_deref(),
            Some("http://host:11434/v1")
        );
        assert_eq!(
            normalize_ollama_base_url("http://host:11434/v1/", false).as_deref(),
            Some("http://host:11434/v1")
        );
        assert_eq!(
            normalize_ollama_base_url(":11434", true).as_deref(),
            Some("http://localhost:11434/v1")
        );
        assert_eq!(
            normalize_ollama_base_url("::1", true).as_deref(),
            Some("http://[::1]:11434/v1")
        );
        assert_eq!(
            normalize_ollama_base_url("https://ollama.example.com/proxy", false).as_deref(),
            Some("https://ollama.example.com/proxy/v1")
        );
        assert_eq!(normalize_ollama_base_url("   ", true), None);
    }

    #[test]
    fn keyless_provider_matches_ollama_and_loopback_urls() {
        assert!(is_keyless_provider("ollama", "https://remote.example.com/v1"));
        assert!(is_keyless_provider("apple-fm", "https://remote.example.com/v1"));
        for url in [
            "http://localhost:11434/v1",
            "http://sub.localhost:8080/v1",
            "http://127.0.0.1:11434/v1",
            "http://[::1]:11434/v1",
            "http://0.0.0.0:11434/v1",
        ] {
            assert!(is_keyless_provider("custom", url), "{url}");
        }
        for url in [
            "https://api.openai.com/v1",
            "http://192.168.1.10:11434/v1",
            "http://localhost.evil.com/v1",
            "not a url",
        ] {
            assert!(!is_keyless_provider("custom", url), "{url}");
        }
    }

    #[test]
    fn native_api_root_strips_v1_and_trailing_slashes() {
        assert_eq!(native_api_root("http://localhost:11434/v1"), "http://localhost:11434");
        assert_eq!(native_api_root("http://localhost:11434/v1/"), "http://localhost:11434");
        assert_eq!(native_api_root("http://host:11434/"), "http://host:11434");
        assert_eq!(native_api_root("https://proxy.example/ollama/v1"), "https://proxy.example/ollama");
    }

    #[test]
    fn parses_show_metadata_and_filters_embedding_models() {
        let gemma_show = json!({
            "thinking": { "values": [false, true], "default": true },
            "model_info": { "gemma4_unified.context_length": 262144 },
            "capabilities": ["completion", "vision", "tools", "thinking"]
        });
        let gemma = build_ollama_model("gemma4:12b", &["completion".into()], Some(&gemma_show)).unwrap();
        assert_eq!(gemma.metadata.name.as_deref(), Some("gemma4:12b"));
        assert_eq!(gemma.metadata.context_window, Some(262_144));
        assert_eq!(gemma.metadata.reasoning, Some(true));

        let mimo = build_ollama_model("mimo-v2.6:latest", &["completion".into()], Some(&json!({
            "model_info": { "qwen3_5.context_length": 262144 },
            "capabilities": ["completion", "thinking"]
        }))).unwrap();
        assert_eq!(mimo.metadata.name.as_deref(), Some("mimo-v2.6"));
        assert_eq!(mimo.metadata.context_window, Some(262_144));
        assert_eq!(mimo.metadata.reasoning, Some(true));

        // Embedding-only model caught via capabilities even if name lacks "embed".
        let embed = build_ollama_model("bge-m3:latest", &[], Some(&json!({
            "capabilities": ["embedding"]
        })));
        assert!(embed.is_none());

        // Dual embedding+completion model is kept.
        let dual = build_ollama_model("custom-dual:latest", &[], Some(&json!({
            "capabilities": ["embedding", "completion"],
            "thinking": false
        }))).unwrap();
        assert_eq!(dual.metadata.reasoning, Some(false));
    }

    #[test]
    fn fallback_matching_supplies_metadata_when_show_omits_it() {
        let gemma = fallback_match_metadata("gemma4:12b");
        assert_eq!(gemma.context_window, Some(262_144));
        assert_eq!(gemma.reasoning, Some(true));

        let mimo = fallback_match_metadata("mimo-v2.6:latest");
        assert_eq!(mimo.context_window, Some(262_144));
        assert_eq!(mimo.reasoning, Some(true));
        assert_eq!(mimo.name.as_deref(), Some("mimo-v2.6"));

        let qwen = fallback_match_metadata("hf.co/unsloth/Qwen3_5-9B-GGUF:Q4_K_M");
        assert_eq!(qwen.context_window, Some(262_144));
        assert_eq!(qwen.reasoning, Some(true));

        let llama = fallback_match_metadata("llama3.3:70b");
        assert_eq!(llama.context_window, Some(131_072));
        assert_eq!(llama.reasoning, Some(false));

        let gpt_oss = fallback_match_metadata("gpt-oss:20b");
        assert_eq!(gpt_oss.context_window, Some(131_072));
        assert_eq!(gpt_oss.reasoning, Some(true));
    }
}
