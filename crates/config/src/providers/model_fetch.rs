use super::*;
use super::apple_fm::{discover_apple_fm_models, ensure_apple_fm_server, APPLE_FM_PROVIDER_ID};
use super::auth_store::literal_ui_key;
use super::model_catalog::is_anthropic;
use super::model_refresh::CachedModelMetadata;
use super::ollama::is_keyless_provider;

/// Short human-readable error summary from an API error body.
fn summarize_error(body: &Value) -> String {
    body.pointer("/error/message")
        .or_else(|| body.pointer("/error"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| body.to_string())
}

impl PiProviderManager {
    /// Queries the provider's model
    /// list endpoint (openai-compatible `GET {baseUrl}/models`, Anthropic
    /// `/v1/models`) and returns plain model ids. When `providerId` is
    /// missing, the `baseUrl`/`apiKey`/`type` from the request body are used
    /// directly; when present, fetched ids are merged with the provider's
    /// cached `models-store.json` models.
    pub async fn fetch_models(&self, input: &FetchModelsInput) -> Result<Vec<String>> {
        let mut models: Vec<String> = input
            .provider_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(|id| self.models_store_models(id).into_iter().map(|model| model.id).collect())
            .unwrap_or_default();
        for id in self.fetch_remote_model_ids(input).await? {
            if !models.contains(&id) {
                models.push(id);
            }
        }
        Ok(models)
    }

    /// The model ids the provider's list endpoint reports (nothing cached).
    pub(super) async fn fetch_remote_model_ids(&self, input: &FetchModelsInput) -> Result<Vec<String>> {
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

        let supplied_key = literal_ui_key(input.api_key.as_deref());
        let api_key = match supplied_key {
            Some(key) => Some(key),
            // The stored credential only goes to the provider's own endpoint.
            None => match provider_id.as_ref() {
                Some(id) if self.is_configured_base_url(id, &base_url) => self.resolve_api_key(id, None).await,
                _ => None,
            },
        };

        if provider_id.as_deref() == Some("ollama") {
            let discovered = self
                .discover_ollama_models(&base_url, api_key.as_deref())
                .await?;
            let ids: Vec<String> = discovered.iter().map(|m| m.id.clone()).collect();
            if self.is_configured_base_url("ollama", &base_url) {
                let details: HashMap<String, CachedModelMetadata> = discovered
                    .into_iter()
                    .map(|m| (m.id, m.metadata))
                    .collect();
                self.store_live_list_with_details("ollama", &ids, &details)?;
            }
            return Ok(ids);
        }

        if provider_id.as_deref() == Some(APPLE_FM_PROVIDER_ID) {
            let discovered = discover_apple_fm_models(&base_url).await?;
            let ids: Vec<String> = discovered.iter().map(|m| m.id.clone()).collect();
            if self.is_configured_base_url(APPLE_FM_PROVIDER_ID, &base_url) {
                let details: HashMap<String, CachedModelMetadata> = discovered
                    .into_iter()
                    .map(|m| (m.id, m.metadata))
                    .collect();
                self.store_live_list_with_details(APPLE_FM_PROVIDER_ID, &ids, &details)?;
            }
            return Ok(ids);
        }

        let mut models: Vec<String> = Vec::new();

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

    /// Sends a tiny completion request and
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
        // A key typed in the UI is used literally (never `!command` / `$VAR`),
        // and the stored credential is never sent to a foreign `baseUrl`.
        let api_key = match literal_ui_key(input.api_key.as_deref()) {
            Some(key) => Some(key),
            None if self.is_configured_base_url(&input.provider_id, base_url) => {
                self.resolve_api_key(&input.provider_id, None).await
            }
            None => None,
        }
        .filter(|k| !k.is_empty());
        if api_key.is_none() && !is_keyless_provider(&input.provider_id, base_url) {
            anyhow::bail!(
                "No API key available for provider \"{}\"",
                input.provider_id
            );
        }
        let api_key = api_key.unwrap_or_default();

        if input.provider_id == "ollama"
            && input.model.as_deref().map(str::trim).is_none_or(str::is_empty)
            && self.merged_models("ollama").is_empty()
        {
            let _ = self.refresh_ollama_models(false).await;
        }
        if input.provider_id == APPLE_FM_PROVIDER_ID {
            ensure_apple_fm_server(base_url).await?;
            if input.model.as_deref().map(str::trim).is_none_or(str::is_empty)
                && self.merged_models(APPLE_FM_PROVIDER_ID).is_empty()
            {
                let _ = self.refresh_apple_fm_models(false).await;
            }
        }

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

        let (content, usage) = if is_anthropic(&input.provider_id, &api, base_url) {
                let mut request = self
                    .http
                    .post(format!("{base_url}/messages"))
                    .header("anthropic-version", "2023-06-01")
                    .json(&json!({
                        "model": model,
                        "max_tokens": 32,
                        "messages": [{ "role": "user", "content": "Reply with the single word: pong" }]
                    }));
                if !api_key.is_empty() {
                    request = request.header("x-api-key", &api_key);
                }
                let response = request.send().await?;
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
                    .json(&json!({
                        "model": model,
                        "max_tokens": 32,
                        "messages": [{ "role": "user", "content": "Reply with the single word: pong" }]
                    }));
                if !api_key.is_empty() {
                    request = request.bearer_auth(&api_key);
                }
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

    /// Whether `base_url` is the endpoint this provider is configured with
    /// (only then may its stored credential be sent there).
    pub(super) fn is_configured_base_url(&self, provider_id: &str, base_url: &str) -> bool {
        let normalize = |url: &str| url.trim().trim_end_matches('/').to_ascii_lowercase();
        normalize(base_url) == normalize(&self.resolve_base_url(provider_id))
    }
}
