//! Live model lists: each provider's `GET /models` result, cached in
//! `models-live.json` under the Locagens home with its fetch time. Once a
//! provider has a live list it decides which models are offered (plus any
//! the user declared in `models.json`), so stale catalog entries — the
//! built-in defaults or an old `pi` `models-store.json` — drop out.

use super::*;
use std::collections::HashSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How old a live list may get before a refresh fetches it again.
pub const MODEL_LIST_MAX_AGE: Duration = Duration::from_secs(6 * 60 * 60);

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

    /// The provider's last fetched model ids, if it has a live list.
    pub(super) fn live_model_ids(&self, provider_id: &str) -> Option<Vec<String>> {
        let store = Self::read_json(&self.live_models_path());
        let ids: Vec<String> = store
            .get(provider_id)?
            .get("models")?
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .filter(|id| is_chat_model(id))
            .map(str::to_string)
            .collect();
        (!ids.is_empty()).then_some(ids)
    }

    fn live_list_age(&self, provider_id: &str) -> Option<Duration> {
        let store = Self::read_json(&self.live_models_path());
        let fetched = store.get(provider_id)?.get("fetchedAtMs")?.as_u64()?;
        Some(Duration::from_millis(now_ms().saturating_sub(fetched)))
    }

    /// Restricts `models` to the live list (keeping `declared` ids) and
    /// adds live ids the catalog did not know.
    pub(super) fn apply_live_list(&self, provider_id: &str, models: &mut Vec<ModelInfo>, declared: &HashSet<String>) {
        let Some(live) = self.live_model_ids(provider_id) else { return };
        let live_set: HashSet<&str> = live.iter().map(String::as_str).collect();
        models.retain(|model| live_set.contains(model.id.as_str()) || declared.contains(&model.id));
        for id in live {
            if !models.iter().any(|model| model.id == id) {
                models.push(ModelInfo {
                    name: id.clone(),
                    id,
                    api: None,
                    base_url: None,
                    context_window: None,
                    max_tokens: None,
                    reasoning: None,
                    cost: None,
                    reasoning_efforts: None,
                });
            }
        }
    }

    fn store_live_list(&self, provider_id: &str, ids: &[String]) -> Result<()> {
        let path = self.live_models_path();
        let mut store = match Self::read_json(&path) {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        store.insert(provider_id.to_string(), json!({ "fetchedAtMs": now_ms(), "models": ids }));
        Self::write_json(&path, &Value::Object(store))
    }

    /// Fetches the provider's live model list and caches it. An empty
    /// answer keeps the previous list (it would hide every model).
    pub async fn refresh_model_list(&self, provider_id: &str) -> Result<usize> {
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

    /// Providers whose model list can be fetched with their stored key:
    /// a base URL, a credential, and no OAuth catalog of their own.
    async fn refreshable_providers(&self) -> Vec<String> {
        let mut refreshable = Vec::new();
        for id in self.provider_ids() {
            if self.resolve_base_url(&id).trim().is_empty() || self.oauth_catalog(&id).is_some() {
                continue;
            }
            if self.resolve_api_key(&id, None).await.is_some_and(|key| !key.is_empty()) {
                refreshable.push(id);
            }
        }
        refreshable
    }

    /// Refreshes every provider whose live list is missing or older than
    /// `max_age`. Failures keep the old list; returns `(provider, outcome)`.
    pub async fn refresh_stale_model_lists(&self, max_age: Duration) -> Vec<(String, Result<usize, String>)> {
        let mut outcomes = Vec::new();
        for id in self.refreshable_providers().await {
            if self.live_list_age(&id).is_some_and(|age| age < max_age) {
                continue;
            }
            let outcome = self.refresh_model_list(&id).await.map_err(|err| err.to_string());
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
