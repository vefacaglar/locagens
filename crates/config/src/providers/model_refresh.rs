//! Live model lists: each provider's `GET /models` result, cached in
//! `models-live.json` under the Locagens home with its fetch time. Once a
//! provider has a live list it decides which models are offered (plus any
//! the user declared in `models.json`), so stale catalog entries — the
//! built-in defaults or an old `pi` `models-store.json` — drop out.

use super::apple_fm::{discover_apple_fm_models, APPLE_FM_MODEL_LIST_MAX_AGE, APPLE_FM_PROVIDER_ID};
use super::ollama::is_keyless_provider;
use super::*;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How old a live list may get before a refresh fetches it again.
pub const MODEL_LIST_MAX_AGE: Duration = Duration::from_secs(6 * 60 * 60);
/// Short TTL for local Ollama model discovery.
pub const OLLAMA_MODEL_LIST_MAX_AGE: Duration = Duration::from_secs(15);

/// Optional cached per-model metadata stored alongside live model ids in
/// `models-live.json`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct CachedModelMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
struct LiveModelEntry {
    fetched_at_ms: u64,
    models: Vec<String>,
    #[serde(default, rename = "modelDetails", skip_serializing_if = "HashMap::is_empty")]
    model_details: HashMap<String, CachedModelMetadata>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Whether a `/models` id can be used for chat: image, video, audio,
/// embedding and moderation models (e.g. xAI's `grok-imagine-*`) are not
/// offered as agent models.
pub(super) fn is_chat_model(id: &str) -> bool {
    const NON_CHAT: [&str; 12] = [
        "imagine", "image", "video", "embed", "tts", "whisper", "audio", "dall-e", "moderation", "transcribe",
        "speech", "rerank",
    ];
    let id = id.to_lowercase();
    !NON_CHAT.iter().any(|marker| id.contains(marker))
}

impl PiProviderManager {
    fn live_models_path(&self) -> PathBuf {
        self.primary_dir.join("models-live.json")
    }

    fn live_entry(&self, provider_id: &str) -> Option<LiveModelEntry> {
        let store = Self::read_json(&self.live_models_path());
        let raw = store.get(provider_id)?;
        serde_json::from_value(raw.clone()).ok()
    }

    /// The provider's last fetched model ids, if it has a live list.
    pub(super) fn live_model_ids(&self, provider_id: &str) -> Option<Vec<String>> {
        let entry = self.live_entry(provider_id)?;
        let ids: Vec<String> = entry
            .models
            .into_iter()
            .filter(|id| is_chat_model(id))
            .collect();
        if provider_id == "ollama" || provider_id == APPLE_FM_PROVIDER_ID {
            Some(ids)
        } else {
            (!ids.is_empty()).then_some(ids)
        }
    }

    fn live_list_age(&self, provider_id: &str) -> Option<Duration> {
        let entry = self.live_entry(provider_id)?;
        Some(Duration::from_millis(now_ms().saturating_sub(entry.fetched_at_ms)))
    }

    /// Restricts `models` to the live list (keeping `declared` ids), applies
    /// cached `modelDetails`, and adds live ids the catalog did not know.
    pub(super) fn apply_live_list(&self, provider_id: &str, models: &mut Vec<ModelInfo>, declared: &HashSet<String>) {
        let Some(entry) = self.live_entry(provider_id) else { return };
        let live: Vec<String> = entry
            .models
            .into_iter()
            .filter(|id| is_chat_model(id))
            .collect();
        if live.is_empty() && provider_id != "ollama" && provider_id != APPLE_FM_PROVIDER_ID {
            return;
        }
        let live_set: HashSet<&str> = live.iter().map(String::as_str).collect();
        models.retain(|model| live_set.contains(model.id.as_str()) || declared.contains(&model.id));
        for id in live {
            let details = entry.model_details.get(&id);
            if let Some(existing) = models.iter_mut().find(|model| model.id == id) {
                if !declared.contains(&id) {
                    if let Some(details) = details {
                        if let Some(name) = &details.name {
                            existing.name = name.clone();
                        }
                        if details.context_window.is_some() {
                            existing.context_window = details.context_window;
                        }
                        if details.max_tokens.is_some() {
                            existing.max_tokens = details.max_tokens;
                        }
                        if details.reasoning.is_some() {
                            existing.reasoning = details.reasoning;
                        }
                    }
                }
            } else {
                models.push(ModelInfo {
                    name: details.and_then(|d| d.name.clone()).unwrap_or_else(|| id.clone()),
                    id,
                    api: None,
                    base_url: None,
                    context_window: details.and_then(|d| d.context_window),
                    max_tokens: details.and_then(|d| d.max_tokens),
                    reasoning: details.and_then(|d| d.reasoning),
                    cost: None,
                    reasoning_efforts: None,
                });
            }
        }
    }

    fn store_live_list(&self, provider_id: &str, ids: &[String]) -> Result<()> {
        self.store_live_list_with_details(provider_id, ids, &HashMap::new())
    }

    pub(super) fn store_live_list_with_details(
        &self,
        provider_id: &str,
        ids: &[String],
        details: &HashMap<String, CachedModelMetadata>,
    ) -> Result<()> {
        let path = self.live_models_path();
        let mut store = match Self::read_json(&path) {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        let entry = LiveModelEntry {
            fetched_at_ms: now_ms(),
            models: ids.to_vec(),
            model_details: details.clone(),
        };
        store.insert(provider_id.to_string(), serde_json::to_value(entry)?);
        Self::write_json(&path, &Value::Object(store))
    }

    /// Discovers installed Ollama chat models and updates `models-live.json`.
    /// When `force` is false, reuses cached models younger than
    /// [`OLLAMA_MODEL_LIST_MAX_AGE`]. An empty response updates the cache to
    /// empty so removed models disappear; unreachable errors leave the cache
    /// untouched.
    pub async fn refresh_ollama_models(&self, force: bool) -> Result<usize> {
        if !force && self.live_list_age("ollama").is_some_and(|age| age < OLLAMA_MODEL_LIST_MAX_AGE) {
            return Ok(self.live_model_ids("ollama").map(|ids| ids.len()).unwrap_or(0));
        }
        let base_url = self.resolve_base_url("ollama");
        if base_url.trim().is_empty() {
            anyhow::bail!("No baseUrl configured for ollama");
        }
        let api_key = self.resolve_api_key("ollama", None).await;
        let discovered = self.discover_ollama_models(&base_url, api_key.as_deref()).await?;
        let ids: Vec<String> = discovered.iter().map(|m| m.id.clone()).collect();
        let details: HashMap<String, CachedModelMetadata> = discovered
            .into_iter()
            .map(|m| (m.id, m.metadata))
            .collect();
        self.store_live_list_with_details("ollama", &ids, &details)?;
        Ok(ids.len())
    }

    /// Discovers available Apple Foundation Models and updates
    /// `models-live.json`. When `force` is false, reuses cached models younger
    /// than [`APPLE_FM_MODEL_LIST_MAX_AGE`].
    pub async fn refresh_apple_fm_models(&self, force: bool) -> Result<usize> {
        if !force
            && self
                .live_list_age(APPLE_FM_PROVIDER_ID)
                .is_some_and(|age| age < APPLE_FM_MODEL_LIST_MAX_AGE)
        {
            return Ok(self
                .live_model_ids(APPLE_FM_PROVIDER_ID)
                .map(|ids| ids.len())
                .unwrap_or(0));
        }
        let base_url = self.resolve_base_url(APPLE_FM_PROVIDER_ID);
        if base_url.trim().is_empty() {
            anyhow::bail!("No baseUrl configured for {APPLE_FM_PROVIDER_ID}");
        }
        let discovered = discover_apple_fm_models(&base_url).await?;
        let ids: Vec<String> = discovered.iter().map(|m| m.id.clone()).collect();
        let details: HashMap<String, CachedModelMetadata> = discovered
            .into_iter()
            .map(|m| (m.id, m.metadata))
            .collect();
        self.store_live_list_with_details(APPLE_FM_PROVIDER_ID, &ids, &details)?;
        Ok(ids.len())
    }

    /// Fetches the provider's live model list and caches it. An empty
    /// answer keeps the previous list (it would hide every model).
    pub async fn refresh_model_list(&self, provider_id: &str) -> Result<usize> {
        if provider_id == "ollama" {
            return self.refresh_ollama_models(true).await;
        }
        if provider_id == APPLE_FM_PROVIDER_ID {
            return self.refresh_apple_fm_models(true).await;
        }
        let ids: Vec<String> = self
            .fetch_remote_model_ids(&FetchModelsInput { provider_id: Some(provider_id.to_string()), ..Default::default() })
            .await?
            .into_iter()
            .filter(|id| is_chat_model(id))
            .collect();
        if ids.is_empty() {
            anyhow::bail!("{provider_id} returned no models");
        }
        self.store_live_list(provider_id, &ids)?;
        Ok(ids.len())
    }

    /// Providers whose model list can be fetched with their stored key (or
    /// keyless for local providers like `"ollama"`), with no OAuth catalog of
    /// their own.
    async fn refreshable_providers(&self) -> Vec<String> {
        let mut refreshable = Vec::new();
        for id in self.provider_ids() {
            let base_url = self.resolve_base_url(&id);
            if base_url.trim().is_empty() || self.oauth_catalog(&id).is_some() {
                continue;
            }
            if is_keyless_provider(&id, &base_url)
                || self.resolve_api_key(&id, None).await.is_some_and(|key| !key.is_empty())
            {
                refreshable.push(id);
            }
        }
        if !refreshable.iter().any(|id| id == "ollama")
            && !self.resolve_base_url("ollama").trim().is_empty()
        {
            refreshable.push("ollama".to_string());
        }
        if !refreshable.iter().any(|id| id == APPLE_FM_PROVIDER_ID)
            && !self.resolve_base_url(APPLE_FM_PROVIDER_ID).trim().is_empty()
        {
            refreshable.push(APPLE_FM_PROVIDER_ID.to_string());
        }
        refreshable
    }

    /// Refreshes every provider whose live list is missing or older than
    /// `max_age` (capped at [`OLLAMA_MODEL_LIST_MAX_AGE`] for `"ollama"` and
    /// [`APPLE_FM_MODEL_LIST_MAX_AGE`] for `"apple-fm"`).
    /// Failures keep the old list; returns `(provider, outcome)`.
    pub async fn refresh_stale_model_lists(&self, max_age: Duration) -> Vec<(String, Result<usize, String>)> {
        let mut outcomes = Vec::new();
        for id in self.refreshable_providers().await {
            let effective_max_age = if id == "ollama" {
                max_age.min(OLLAMA_MODEL_LIST_MAX_AGE)
            } else if id == APPLE_FM_PROVIDER_ID {
                max_age.min(APPLE_FM_MODEL_LIST_MAX_AGE)
            } else {
                max_age
            };
            if self.live_list_age(&id).is_some_and(|age| age < effective_max_age) {
                continue;
            }
            let outcome = if id == "ollama" {
                self.refresh_ollama_models(true).await
            } else if id == APPLE_FM_PROVIDER_ID {
                self.refresh_apple_fm_models(true).await
            } else {
                self.refresh_model_list(&id).await
            }
            .map_err(|err| err.to_string());
            if let Err(err) = &outcome {
                tracing::debug!("Model list refresh for {id} failed: {err}");
            }
            outcomes.push((id, outcome));
        }
        outcomes
    }
}

#[cfg(test)]
mod tests {
    use super::is_chat_model;

    #[test]
    fn media_and_embedding_models_are_not_chat_models() {
        for id in ["grok-imagine-video-1.5", "grok-imagine-image", "text-embedding-3-small", "whisper-1", "gpt-4o-mini-tts"] {
            assert!(!is_chat_model(id), "{id}");
        }
        for id in ["grok-4.7", "deepseek-v4.1-flash", "gpt-6-luna", "claude-haiku-5-5"] {
            assert!(is_chat_model(id), "{id}");
        }
    }
}
