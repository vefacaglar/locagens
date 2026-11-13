use super::*;
use super::auth_store::literal_ui_key;
use super::model_catalog::is_anthropic;

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

        let (content, usage) = if is_anthropic(&input.provider_id, &api, base_url) {
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

    /// Whether `base_url` is the endpoint this provider is configured with
    /// (only then may its stored credential be sent there).
    pub(super) fn is_configured_base_url(&self, provider_id: &str, base_url: &str) -> bool {
        let normalize = |url: &str| url.trim().trim_end_matches('/').to_ascii_lowercase();
        normalize(base_url) == normalize(&self.resolve_base_url(provider_id))
    }
}
