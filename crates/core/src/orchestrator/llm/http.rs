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
    }
    // OpenCode Zen requires a per-conversation routing header, on both its
    // OpenAI-compatible and its Anthropic Messages endpoints.
    if base_url.contains("opencode.ai") && !endpoint.session_id.is_empty() {
        request = request.header("x-opencode-session", &endpoint.session_id);
    }
    request
}

/// Headers a provider quirk adds to every request: Copilot's editor identity
/// plus who initiated the call and whether it carries images.
pub(super) fn quirk_headers(endpoint: &Endpoint, messages: &[LlmMessage]) -> Vec<(&'static str, &'static str)> {
    if endpoint.quirk != ProviderQuirk::GithubCopilot {
        return Vec::new();
    }
    let last_is_user = messages.last().is_none_or(|message| message.role == "user");
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

/// Longest server-requested wait honoured before a retry. A 60-second rate
/// limit window is waited out instead of failing the run (FR-070).
pub(super) const MAX_RETRY_AFTER: Duration = Duration::from_secs(90);

/// The server's requested wait: `retry-after-ms` (OpenAI) or `Retry-After`
/// in seconds, capped at [`MAX_RETRY_AFTER`].
pub(super) fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok()).map(str::trim);
    let millis = header("retry-after-ms").and_then(|raw| raw.parse::<f64>().ok()).map(|ms| ms / 1000.0);
    let seconds = millis.or_else(|| header("retry-after").and_then(|raw| raw.parse::<f64>().ok()))?;
    (seconds.is_finite() && seconds >= 0.0).then(|| Duration::from_secs_f64(seconds).min(MAX_RETRY_AFTER))
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
                if status.is_redirection() {
                    let location = response
                        .headers()
                        .get(reqwest::header::LOCATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("?");
                    anyhow::bail!(
                        "Provider redirected ({status}) to {location}. Redirects are not followed so the API key \
                         never leaves the configured host; set the provider's baseUrl to the final address."
                    );
                }
                if status.is_success()
                    || attempt + 1 >= MAX_SEND_ATTEMPTS
                    || !is_retryable_status(status)
                {
                    return Ok(response);
                }
                let retry_after = retry_after(response.headers()).unwrap_or_else(|| backoff_delay(attempt));
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
            // Requests carry `x-api-key` / `Authorization`: never follow a
            // redirect to another host with them (FR-035).
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default()
    });
    CLIENT.clone()
}


/// Short human-readable error summary from an API error body.
/// The error for a non-success response: the status, the provider's own
/// message when it sent one, and a hint for a missing model or endpoint.
pub(super) fn provider_error(status: reqwest::StatusCode, body: &str) -> anyhow::Error {
    let summary = summarize_error(body);
    let summary = summary.trim();
    let mut text = format!("Provider returned {status}");
    if !summary.is_empty() {
        text.push_str(": ");
        text.push_str(summary);
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        text.push_str(" (the model or endpoint may not be available for this provider or account)");
    }
    anyhow::anyhow!(text)
}

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

#[cfg(test)]
mod provider_header_tests {
    use super::apply_provider_headers;
    use crate::orchestrator::llm::types::Endpoint;

    fn session_header(api: &str) -> Option<String> {
        let base_url = "https://opencode.ai/zen/go/v1";
        let endpoint = Endpoint {
            base_url: base_url.into(),
            api: api.into(),
            api_key: "test-key".into(),
            session_id: "run-1".into(),
            ..Default::default()
        };
        let request = apply_provider_headers(reqwest::Client::new().post(base_url), &endpoint, base_url, &[])
            .build()
            .unwrap();
        request.headers().get("x-opencode-session").map(|v| v.to_str().unwrap().to_string())
    }

    #[test]
    fn opencode_gets_the_session_header_on_both_apis() {
        assert_eq!(session_header("openai-compatible").as_deref(), Some("run-1"));
        assert_eq!(session_header("anthropic-messages").as_deref(), Some("run-1"));
    }
}

#[cfg(test)]
mod provider_error_tests {
    use super::provider_error;
    use reqwest::StatusCode;

    #[test]
    fn empty_body_has_no_dangling_colon() {
        let text = provider_error(StatusCode::BAD_GATEWAY, "").to_string();
        assert_eq!(text, "Provider returned 502 Bad Gateway");
    }

    #[test]
    fn not_found_gets_a_hint_and_keeps_the_message() {
        let text = provider_error(StatusCode::NOT_FOUND, r#"{"error":{"message":"no such model"}}"#).to_string();
        assert!(text.starts_with("Provider returned 404 Not Found: no such model"), "{text}");
        assert!(text.contains("may not be available"), "{text}");
    }
}
