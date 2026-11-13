//! Request transport: provider headers, retry with backoff, shared client.

use super::*;

// ----------------------------------------------------------------------
// Request transport: shared headers + transient-error retry
// ----------------------------------------------------------------------

/// Attaches the provider auth/routing headers every attempt needs: Anthropic
/// gets `x-api-key` + `anthropic-version`, OpenAI-compatible endpoints get a
/// bearer token, OpenRouter gets `X-Title` and OpenCode Zen a
/// `x-opencode-session` routing header.
pub(super) fn apply_provider_headers(
    mut request: reqwest::RequestBuilder,
    endpoint: &Endpoint,
    base_url: &str,
    quirk_headers: &[(&'static str, &'static str)],
) -> reqwest::RequestBuilder {
    for (name, value) in quirk_headers {
        request = request.header(*name, *value);
    }
    if endpoint.is_anthropic() {
        // Copilot authenticates Anthropic requests with a bearer token too.
        request = if endpoint.quirk == ProviderQuirk::GithubCopilot {
            request.bearer_auth(&endpoint.api_key)
        } else {
            request.header("x-api-key", &endpoint.api_key)
        };
        request = request.header("anthropic-version", "2023-06-01");
    } else {
        if !endpoint.api_key.is_empty() {
            request = request.bearer_auth(&endpoint.api_key);
        }
        if base_url.contains("openrouter") {
            request = request.header("X-Title", "Locagens");
        }
        // OpenCode Zen requires a per-conversation routing header.
        if base_url.contains("opencode.ai") && !endpoint.session_id.is_empty() {
            request = request.header("x-opencode-session", &endpoint.session_id);
        }
    }
    request
}

/// Headers a provider quirk adds to every request: Copilot's editor identity
/// plus who initiated the call and whether it carries images.
pub(super) fn quirk_headers(endpoint: &Endpoint, messages: &[LlmMessage]) -> Vec<(&'static str, &'static str)> {
    if endpoint.quirk != ProviderQuirk::GithubCopilot {
        return Vec::new();
    }
    let last_is_user = messages.last().map_or(true, |message| message.role == "user");
    let has_images = messages
        .iter()
        .any(|message| message.role == "tool" && extract_tool_image(&message.content).is_some());
    let mut headers = locagens_config::oauth::github_copilot::COPILOT_HEADERS.to_vec();
    headers.extend(locagens_config::oauth::github_copilot::dynamic_headers(last_is_user, has_images));
    headers
}

/// Total send attempts per request (1 initial call + 3 retries).
pub(super) const MAX_SEND_ATTEMPTS: usize = 4;

/// Exponential backoff between retries: `500ms * 2^attempt` with a little
/// jitter, capped at 10 s.
pub(super) fn backoff_delay(attempt: usize) -> Duration {
    let base = Duration::from_millis(500).saturating_mul(1 << attempt.min(8) as u32);
    let jitter = Duration::from_millis((std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_millis() as u64)
        .unwrap_or(0))
        % 100);
    base.saturating_add(jitter).min(Duration::from_secs(10))
}

/// Whether an HTTP status is transient and worth retrying.
pub(super) fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504)
}

/// Whether a transport error is transient (timeout / connect / request).
pub(super) fn is_retryable_error(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect() || err.is_request()
}

/// Sends a provider request with exponential backoff on transient failures
/// (HTTP 429/5xx or timeouts/connect errors): up to 3 retries after the
/// initial attempt. Headers & auth are re-attached on every attempt because
/// `make_request` rebuilds the request each time. Returns the final response
/// (success or non-retryable error status) so the caller can inspect it.
pub(super) async fn send_with_retry(
    mut make_request: impl FnMut() -> reqwest::RequestBuilder,
) -> anyhow::Result<reqwest::Response> {
    let mut attempt = 0usize;
    loop {
        // The shared client has no total timeout (long streams are fine);
        // waiting for the response headers is bounded here instead.
        let Ok(sent) = tokio::time::timeout(STREAM_IDLE_TIMEOUT, make_request().send()).await else {
            anyhow::bail!("Provider did not respond within {} s", STREAM_IDLE_TIMEOUT.as_secs());
        };
        match sent {
            Ok(response) => {
                let status = response.status();
                if status.is_success()
                    || attempt + 1 >= MAX_SEND_ATTEMPTS
                    || !is_retryable_status(status)
                {
                    return Ok(response);
                }
                let retry_after = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|raw| raw.trim().parse::<u64>().ok())
                    .map(Duration::from_secs)
                    .unwrap_or_else(|| backoff_delay(attempt))
                    .min(Duration::from_secs(10));
                tracing::warn!("Provider returned {status}; retrying in {retry_after:?}");
                tokio::time::sleep(retry_after).await;
                attempt += 1;
            }
            Err(err) => {
                if attempt + 1 >= MAX_SEND_ATTEMPTS || !is_retryable_error(&err) {
                    return Err(anyhow::anyhow!("Provider request failed: {err}"));
                }
                let delay = backoff_delay(attempt);
                tracing::warn!("Provider request failed ({err}); retrying in {delay:?}");
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}


/// The one HTTP client shared by every model call (connection reuse). It has
/// a connect timeout but no total timeout: streams may legitimately run for a
/// long time, and stalls are caught by [`STREAM_IDLE_TIMEOUT`] instead.
pub fn shared_http_client() -> reqwest::Client {
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default()
    });
    CLIENT.clone()
}


/// Short human-readable error summary from an API error body.
pub(super) fn summarize_error(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("error"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.chars().take(400).collect())
}
