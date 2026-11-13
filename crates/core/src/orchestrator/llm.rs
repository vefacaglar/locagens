//! Pure Rust streaming LLM client.
//!
//! Talks directly to the provider endpoints resolved from `pi`'s
//! configuration (`~/.pi/agent/auth.json` for credentials, `models.json` /
//! `models-store.json` for `baseUrl` + API protocol) via `reqwest`:
//!
//! - **OpenAI-compatible** (`{baseUrl}/chat/completions`, `stream: true`):
//!   parses `data: {...}` SSE chunks (`choices[0].delta.content`,
//!   `reasoning_content` / `reasoning`, `tool_calls`, `usage`).
//! - **Anthropic Messages** (`{baseUrl}/messages`, `stream: true`): parses
//!   `content_block_start`, `content_block_delta` (`text_delta`,
//!   `thinking_delta`, `input_json_delta`), `message_delta`, `message_stop`.
//! - **OpenAI Responses** (Codex WireAPI, `{baseUrl}/responses` with a
//!   `/v1`-suffixed baseUrl or `{baseUrl}/v1/responses` otherwise,
//!   `stream: true`): sends `instructions` + `input` items and parses
//!   `response.output_text.delta`, `response.reasoning_summary_text.delta` /
//!   `response.reasoning_text.delta`, `response.output_item.added` /
//!   `.done` (function_call items), `response.function_call_arguments.delta`
//!   and `response.completed` (usage).
//!
//! Deltas are forwarded through a callback so the orchestrator can update the
//! live assistant `RunMessage` and emit throttled `message_updated` SSE events.

use anyhow::Result;
use serde_json::{json, Map, Value};
use std::time::Duration;

/// One chat message in provider-neutral form. Tool calls are kept in the
/// OpenAI shape (`{ id, function: { name, arguments } }`) — the normalized
/// shape used in `RunMessage.rawResponse` — and converted per API protocol
/// when the request body is built.
#[derive(Clone, Debug, Default)]
pub struct LlmMessage {
    pub role: String,
    pub content: String,
    /// `tool_call_id` for `role == "tool"` responses.
    ///
    /// (`call_id` for `function_call_output` items under the Responses API.)
    pub tool_call_id: Option<String>,
    /// Tool calls requested by an assistant message.
    pub tool_calls: Option<Vec<Value>>,
}

/// Token accounting for one completion call.
#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// Provider-reported cost (when the upstream bills it directly); the
    /// orchestrator's per-million pricing is only a fallback.
    pub provider_cost: Option<f64>,
}

impl Usage {
    pub fn is_empty(&self) -> bool {
        self.input_tokens == 0 && self.output_tokens == 0
    }
}

/// Everything a streaming completion produced.
#[derive(Clone, Debug, Default)]
pub struct StreamOutcome {
    pub content: String,
    pub reasoning: String,
    /// Normalized tool calls: `[{ id, function: { name, arguments(str) } }]`.
    pub tool_calls: Vec<Value>,
    pub usage: Option<Usage>,
}

/// Resolved endpoint for one loop iteration.
#[derive(Clone, Debug, Default)]
pub struct Endpoint {
    pub base_url: String,
    /// `"openai-compatible"` (chat/completions), `"openai-responses"` /
    /// `"responses"` (Codex `/v1/responses` WireAPI) or `"anthropic"`.
    pub api: String,
    pub api_key: String,
    /// Per-conversation routing id (OpenCode requires `x-opencode-session`).
    pub session_id: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub max_tokens: Option<i64>,
    /// Whether the model accepts a `reasoning_effort` parameter.
    pub reasoning_supported: bool,
    /// Provider-specific request differences (headers, URLs, body fields).
    pub quirk: ProviderQuirk,
}

/// Providers whose requests differ from the plain protocol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProviderQuirk {
    #[default]
    None,
    /// GitHub Copilot: editor identity headers, bearer auth for every API,
    /// no `/v1` prefix on its chat and Responses URLs.
    GithubCopilot,
    /// "Sign in with ChatGPT" token against `api.openai.com/v1`: the
    /// Responses body must be `store: false` without `max_output_tokens`.
    ChatgptOauth,
}

const OPENAI_API_URL: &str = "https://api.openai.com/v1";

impl ProviderQuirk {
    /// Detects the quirk from the resolved provider, base URL and credential.
    /// ChatGPT sign-in is `openai` on exactly `api.openai.com/v1` with a
    /// bearer that is not an `sk-` API key (pi's detection).
    pub fn detect(provider_id: &str, base_url: &str, api_key: &str) -> Self {
        if provider_id == "github-copilot" {
            Self::GithubCopilot
        } else if provider_id == "openai"
            && base_url.trim_end_matches('/') == OPENAI_API_URL
            && !api_key.starts_with("sk-")
        {
            Self::ChatgptOauth
        } else {
            Self::None
        }
    }
}

impl Endpoint {
    /// `{base}/messages`; Copilot serves Anthropic Messages at `/v1/messages`.
    fn messages_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if self.quirk == ProviderQuirk::GithubCopilot {
            format!("{base}/v1/messages")
        } else {
            format!("{base}/messages")
        }
    }

    /// The Responses URL; Copilot has no `/v1` prefix.
    fn responses_endpoint_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if self.quirk == ProviderQuirk::GithubCopilot {
            format!("{base}/responses")
        } else {
            responses_url(base)
        }
    }

    pub fn is_anthropic(&self) -> bool {
        self.api == "anthropic"
            || self.api == "anthropic-messages"
            || self.base_url.contains("anthropic")
    }

    /// True when the endpoint speaks Codex's native OpenAI Responses WireAPI
    /// (`/v1/responses`) instead of chat completions.
    pub fn is_openai_responses(&self) -> bool {
        self.api == "openai-responses" || self.api == "responses"
    }
}

/// Accumulator fed with parsed SSE/JSON events, protocol-agnostically. Kept as
/// a plain struct so the parsing logic is unit-testable without network I/O.
#[derive(Default)]
pub struct StreamAcc {
    pub content: String,
    pub reasoning: String,
    /// (content_delta, reasoning_delta) pairs, in arrival order — used by
    /// tests and re-played by the orchestrator as streaming deltas.
    pub deltas: Vec<(String, String)>,
    tool_acc: std::collections::BTreeMap<usize, ToolAcc>,
    next_tool_index: usize,
    pub usage: Usage,
    pub finished: bool,
}

#[derive(Default)]
struct ToolAcc {
    id: String,
    name: String,
    args: String,
}

impl StreamAcc {
    /// Consumes one OpenAI-compatible `chat.completion.chunk`.
    pub fn feed_openai(&mut self, chunk: &Value) {
        if let Some(usage) = chunk.get("usage").filter(|u| !u.is_null()) {
            self.usage = Usage {
                input_tokens: usage.get("prompt_tokens").and_then(Value::as_i64).unwrap_or(0),
                output_tokens: usage.get("completion_tokens").and_then(Value::as_i64).unwrap_or(0),
                cache_read_tokens: usage
                    .pointer("/prompt_tokens_details/cached_tokens")
                    .and_then(Value::as_i64)
                    .or_else(|| usage.get("cache_read_input_tokens").and_then(Value::as_i64))
                    .unwrap_or(0),
                cache_write_tokens: usage
                    .pointer("/prompt_tokens_details/cache_write_tokens")
                    .and_then(Value::as_i64)
                    .or_else(|| usage.get("cache_write_input_tokens").and_then(Value::as_i64))
                    .unwrap_or(0),
                // OpenCode-style providers report the billed cost directly.
                provider_cost: usage
                    .get("cost")
                    .and_then(Value::as_f64)
                    .or_else(|| usage.pointer("/opencode/list_cost").and_then(Value::as_f64)),
            };
        }

        let Some(choice) = chunk.pointer("/choices/0") else {
            return;
        };
        if let Some(delta) = choice.get("delta") {
            if let Some(text) = delta.get("content").and_then(Value::as_str) {
                if !text.is_empty() {
                    self.content.push_str(text);
                    self.deltas.push((text.to_string(), String::new()));
                }
            }
            let reasoning = delta
                .get("reasoning_content")
                .or_else(|| delta.get("reasoning"))
                .or_else(|| delta.get("reasoningContent"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !reasoning.is_empty() {
                self.reasoning.push_str(reasoning);
                self.deltas.push((String::new(), reasoning.to_string()));
            }
            if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for call in tool_calls {
                    let index = call.get("index").and_then(Value::as_u64).map(|i| i as usize)
                        .unwrap_or_else(|| {
                            // Providers without an index treat each entry as new.
                            let idx = self.next_tool_index;
                            self.next_tool_index += 1;
                            idx
                        });
                    let entry = self.tool_acc.entry(index).or_default();
                    if let Some(id) = call.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) {
                        entry.id = id.to_string();
                    }
                    if let Some(function) = call.get("function") {
                        if let Some(name) = function.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()) {
                            entry.name = name.to_string();
                        }
                        if let Some(args) = function.get("arguments").and_then(Value::as_str) {
                            entry.args.push_str(args);
                        }
                    }
                }
            }
        }
        if choice.get("finish_reason").and_then(Value::as_str).is_some() {
            self.finished = true;
        }
    }

    /// Consumes one Anthropic Messages streaming event.
    pub fn feed_anthropic(&mut self, event: &Value) {
        match event.get("type").and_then(Value::as_str).unwrap_or_default() {
            "message_start" => {
                if let Some(usage) = event.pointer("/message/usage") {
                    self.usage.input_tokens = usage.get("input_tokens").and_then(Value::as_i64).unwrap_or(0);
                    self.usage.cache_read_tokens = usage.get("cache_read_input_tokens").and_then(Value::as_i64).unwrap_or(0);
                    self.usage.cache_write_tokens = usage.get("cache_creation_input_tokens").and_then(Value::as_i64).unwrap_or(0);
                }
            }
            "content_block_start" => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                if let Some(block) = event.get("content_block") {
                    if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                        self.tool_acc.insert(index, ToolAcc {
                            id: block.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                            name: block.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                            args: String::new(),
                        });
                    }
                }
            }
            "content_block_delta" => {
                let delta = event.get("delta").cloned().unwrap_or(Value::Null);
                match delta.get("type").and_then(Value::as_str).unwrap_or_default() {
                    "text_delta" => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str) {
                            if !text.is_empty() {
                                self.content.push_str(text);
                                self.deltas.push((text.to_string(), String::new()));
                            }
                        }
                    }
                    "thinking_delta" => {
                        if let Some(text) = delta.get("thinking").and_then(Value::as_str) {
                            if !text.is_empty() {
                                self.reasoning.push_str(text);
                                self.deltas.push((String::new(), text.to_string()));
                            }
                        }
                    }
                    "input_json_delta" => {
                        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                        if let Some(partial) = delta.get("partial_json").and_then(Value::as_str) {
                            if let Some(entry) = self.tool_acc.get_mut(&index) {
                                entry.args.push_str(partial);
                            }
                        }
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(usage) = event.get("usage") {
                    self.usage.output_tokens = usage.get("output_tokens").and_then(Value::as_i64).unwrap_or(0);
                }
            }
            "message_stop" => {
                self.finished = true;
            }
            _ => {}
        }
    }

    /// Consumes one OpenAI `/v1/responses` (Codex WireAPI) SSE event.
    pub fn feed_responses(&mut self, event: &Value) {
        match event.get("type").and_then(Value::as_str).unwrap_or_default() {
            "response.output_text.delta" => {
                if let Some(text) = event.get("delta").and_then(Value::as_str) {
                    if !text.is_empty() {
                        self.content.push_str(text);
                        self.deltas.push((text.to_string(), String::new()));
                    }
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(text) = event.get("delta").and_then(Value::as_str) {
                    if !text.is_empty() {
                        self.reasoning.push_str(text);
                        self.deltas.push((String::new(), text.to_string()));
                    }
                }
            }
            "response.output_item.added" => {
                let item = event.get("item").cloned().unwrap_or(Value::Null);
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    let index = match event.get("output_index").and_then(Value::as_u64) {
                        Some(i) => i as usize,
                        None => {
                            let idx = self.next_tool_index;
                            self.next_tool_index += 1;
                            idx
                        }
                    };
                    let entry = self.tool_acc.entry(index).or_default();
                    if let Some(id) = item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                    {
                        entry.id = id.to_string();
                    }
                    if let Some(name) = item.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()) {
                        entry.name = name.to_string();
                    }
                    if let Some(args) = item.get("arguments").and_then(Value::as_str) {
                        entry.args.push_str(args);
                    }
                }
            }
            "response.function_call_arguments.delta" => {
                let index = self.responses_tool_index(event);
                if let Some(entry) = self.tool_acc.get_mut(&index) {
                    if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                        entry.args.push_str(delta);
                    }
                }
            }
            "response.output_item.done" => {
                let item = event.get("item").cloned().unwrap_or(Value::Null);
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    let index = self.responses_tool_index(event);
                    let entry = self.tool_acc.entry(index).or_default();
                    if let Some(id) = item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                    {
                        entry.id = id.to_string();
                    }
                    if let Some(name) = item.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()) {
                        entry.name = name.to_string();
                    }
                    // The done event carries the full final arguments.
                    if let Some(args) = item.get("arguments").and_then(Value::as_str) {
                        entry.args = args.to_string();
                    }
                }
            }
            "response.completed" => {
                if let Some(usage) = event.pointer("/response/usage").filter(|u| !u.is_null()) {
                    self.usage = Usage {
                        input_tokens: usage.get("input_tokens").and_then(Value::as_i64).unwrap_or(0),
                        output_tokens: usage.get("output_tokens").and_then(Value::as_i64).unwrap_or(0),
                        cache_read_tokens: usage
                            .pointer("/input_tokens_details/cached_tokens")
                            .and_then(Value::as_i64)
                            .unwrap_or(0),
                        cache_write_tokens: 0,
                        provider_cost: usage.get("cost").and_then(Value::as_f64),
                    };
                }
                self.finished = true;
            }
            _ => {}
        }
    }

    /// Tool-accumulator key for a Responses event: the `output_index` field
    /// when present, otherwise the sole tracked call (or a fresh slot).
    fn responses_tool_index(&self, event: &Value) -> usize {
        match event.get("output_index").and_then(Value::as_u64) {
            Some(i) => i as usize,
            None => self.tool_acc.keys().next().copied().unwrap_or(self.next_tool_index),
        }
    }

    /// Normalizes accumulated tool calls into the OpenAI shape used in
    /// `RunMessage.rawResponse`.
    pub fn tool_calls(&self) -> Vec<Value> {
        self.tool_acc
            .values()
            .filter(|acc| !acc.name.is_empty())
            .map(|acc| {
                json!({
                    "id": if acc.id.is_empty() { format!("call_{}", uuid::Uuid::new_v4().simple()) } else { acc.id.clone() },
                    "function": {
                        "name": acc.name,
                        "arguments": if acc.args.trim().is_empty() { "{}".to_string() } else { acc.args.clone() },
                    }
                })
            })
            .collect()
    }
}

// ----------------------------------------------------------------------
// Request transport: shared headers + transient-error retry
// ----------------------------------------------------------------------

/// Attaches the provider auth/routing headers every attempt needs: Anthropic
/// gets `x-api-key` + `anthropic-version`, OpenAI-compatible endpoints get a
/// bearer token, OpenRouter gets `X-Title` and OpenCode Zen a
/// `x-opencode-session` routing header.
fn apply_provider_headers(
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
fn quirk_headers(endpoint: &Endpoint, messages: &[LlmMessage]) -> Vec<(&'static str, &'static str)> {
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
const MAX_SEND_ATTEMPTS: usize = 4;

/// Exponential backoff between retries: `500ms * 2^attempt` with a little
/// jitter, capped at 10 s.
fn backoff_delay(attempt: usize) -> Duration {
    let base = Duration::from_millis(500).saturating_mul(1 << attempt.min(8) as u32);
    let jitter = Duration::from_millis((std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_millis() as u64)
        .unwrap_or(0))
        % 100);
    base.saturating_add(jitter).min(Duration::from_secs(10))
}

/// Whether an HTTP status is transient and worth retrying.
fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504)
}

/// Whether a transport error is transient (timeout / connect / request).
fn is_retryable_error(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect() || err.is_request()
}

/// Sends a provider request with exponential backoff on transient failures
/// (HTTP 429/5xx or timeouts/connect errors): up to 3 retries after the
/// initial attempt. Headers & auth are re-attached on every attempt because
/// `make_request` rebuilds the request each time. Returns the final response
/// (success or non-retryable error status) so the caller can inspect it.
async fn send_with_retry(
    mut make_request: impl FnMut() -> reqwest::RequestBuilder,
) -> anyhow::Result<reqwest::Response> {
    let mut attempt = 0usize;
    loop {
        match make_request().send().await {
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

/// Streams one completion, forwarding `(content_delta, reasoning_delta)`
/// through `on_delta`. `should_stop` is polled between events so the caller
/// can abort mid-stream (cancellation); the returned error is `"cancelled"`
/// in that case.
pub async fn stream_chat(
    http: &reqwest::Client,
    endpoint: &Endpoint,
    system: &str,
    messages: &[LlmMessage],
    tools: &[Value],
    on_delta: &mut (dyn FnMut(&str, &str) + Send),
    should_stop: &(dyn Fn() -> bool + Send + Sync),
) -> Result<StreamOutcome> {
    let base_url = endpoint.base_url.trim_end_matches('/').to_string();
    if base_url.is_empty() {
        anyhow::bail!("No baseUrl configured for provider endpoint");
    }

    // Codex Responses WireAPI has its own request/streaming path.
    if endpoint.is_openai_responses() {
        return stream_openai_responses(http, endpoint, system, messages, tools, on_delta, should_stop)
            .await;
    }

    let (url, mut request_body) = build_request(endpoint, system, messages, tools);
    // Ask for token accounting on OpenAI-compatible streams; providers that
    // reject the field get an automatic retry without it. The Responses API
    // reports usage in `response.completed` and rejects the field.
    let with_stream_options = !endpoint.is_anthropic() && !endpoint.is_openai_responses();
    if with_stream_options {
        request_body["stream_options"] = json!({ "include_usage": true });
    }

    let extra_headers = quirk_headers(endpoint, messages);
    let response = send_with_retry(|| {
        let request = http.post(&url);
        apply_provider_headers(request, endpoint, &base_url, &extra_headers).json(&request_body)
    })
    .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        // Retry once without `stream_options` when the provider rejects it.
        if with_stream_options && body.contains("stream_options") {
            request_body.as_object_mut().unwrap().remove("stream_options");
            let retry = send_with_retry(|| {
                let request = http.post(&url);
                apply_provider_headers(request, endpoint, &base_url, &extra_headers).json(&request_body)
            })
            .await?;
            let retry_status = retry.status();
            if !retry_status.is_success() {
                let retry_body = retry.text().await.unwrap_or_default();
                anyhow::bail!("Provider returned {retry_status}: {}", summarize_error(&retry_body));
            }
            return consume_response(retry, endpoint, on_delta, should_stop).await;
        }
        anyhow::bail!("Provider returned {status}: {}", summarize_error(&body));
    }

    consume_response(response, endpoint, on_delta, should_stop).await
}

/// Drains a successful completion response into a `StreamOutcome` — either
/// the SSE event stream or, when the endpoint ignored `stream: true`, the
/// plain JSON body.
async fn consume_response(
    response: reqwest::Response,
    endpoint: &Endpoint,
    on_delta: &mut (dyn FnMut(&str, &str) + Send),
    should_stop: &(dyn Fn() -> bool + Send + Sync),
) -> Result<StreamOutcome> {
    let is_anthropic = endpoint.is_anthropic();
    let is_responses = endpoint.is_openai_responses();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();

    if !content_type.contains("text/event-stream") {
        // Non-streaming fallback: the endpoint ignored `stream: true`.
        let body: Value = response.json().await.unwrap_or(Value::Null);
        let mut acc = StreamAcc::default();
        if is_responses {
            // Replay the full response object as the equivalent events.
            let output = body.get("output").and_then(Value::as_array).cloned().unwrap_or_default();
            for item in &output {
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    acc.feed_responses(&json!({
                        "type": "response.output_item.done", "output_index": 0, "item": item
                    }));
                } else if item.get("type").and_then(Value::as_str) == Some("message") {
                    if let Some(blocks) = item.get("content").and_then(Value::as_array) {
                        for block in blocks {
                            if block.get("type").and_then(Value::as_str) == Some("output_text") {
                                if let Some(text) = block.get("text").and_then(Value::as_str) {
                                    acc.content.push_str(text);
                                    acc.deltas.push((text.to_string(), String::new()));
                                }
                            }
                        }
                    }
                }
            }
            acc.feed_responses(&json!({ "type": "response.completed", "response": body }));
            return finish(acc);
        }
        acc.feed_openai(&non_streaming_openai(&body));
        let blocks = body.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
        for (i, block) in blocks.iter().enumerate() {
            acc.feed_anthropic(&json!({
                "type": "content_block_start", "index": i as u64,
                "content_block": normalize_anthropic_block(block)
            }));
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        acc.feed_anthropic(&json!({ "type": "content_block_delta", "index": i as u64, "delta": { "type": "text_delta", "text": text } }));
                    }
                }
                Some("thinking") => {
                    if let Some(text) = block.get("thinking").and_then(Value::as_str) {
                        acc.feed_anthropic(&json!({ "type": "content_block_delta", "index": i as u64, "delta": { "type": "thinking_delta", "thinking": text } }));
                    }
                }
                _ => {}
            }
        }
        if let Some(usage) = body.get("usage") {
            acc.feed_anthropic(&json!({ "type": "message_delta", "usage": usage }));
        }
        return finish(acc);
    }

    use futures::StreamExt;
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut acc = StreamAcc::default();

    while let Some(chunk) = stream.next().await {
        if should_stop() {
            anyhow::bail!("cancelled");
        }
        let bytes = chunk.map_err(|err| anyhow::anyhow!("Stream interrupted: {err}"))?;
        buffer.push_str(&String::from_utf8_lossy(&bytes));

        while let Some(newline) = buffer.find('\n') {
            let mut line = buffer[..newline].to_string();
            buffer.drain(..=newline);
            if line.ends_with('\r') {
                line.pop();
            }
            if is_responses {
                let Some(event) = parse_responses_sse_line(&line) else {
                    continue;
                };
                let event_type = event.get("type").and_then(Value::as_str).unwrap_or_default();
                if event_type == "error" || event_type == "response.failed" {
                    let message = event
                        .pointer("/error/message")
                        .or_else(|| event.pointer("/response/error/message"))
                        .or_else(|| event.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("stream error");
                    anyhow::bail!("Provider stream error: {message}");
                }
                acc.feed_responses(&event);
                for (content_delta, reasoning_delta) in acc.deltas.drain(..) {
                    on_delta(&content_delta, &reasoning_delta);
                }
                continue;
            }
            let data = match line.strip_prefix("data:") {
                Some(data) => data.trim(),
                None => continue,
            };
            if data.is_empty() {
                continue;
            }
            if data == "[DONE]" {
                return finish(acc);
            }
            let Ok(event) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            if is_anthropic {
                acc.feed_anthropic(&event);
            } else {
                if event.get("type").and_then(Value::as_str) == Some("error") {
                    let message = event
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("stream error");
                    anyhow::bail!("Provider stream error: {message}");
                }
                acc.feed_openai(&event);
            }
            for (content_delta, reasoning_delta) in acc.deltas.drain(..) {
                on_delta(&content_delta, &reasoning_delta);
            }
            // NOTE: do not return on `acc.finished` — OpenAI-compatible
            // providers send a final usage-only chunk after the chunk that
            // carries `finish_reason`.
        }
    }

    finish(acc)
}

fn finish(acc: StreamAcc) -> Result<StreamOutcome> {
    Ok(StreamOutcome {
        content: acc.content.clone(),
        reasoning: acc.reasoning.clone(),
        tool_calls: acc.tool_calls(),
        usage: if acc.usage.is_empty() { None } else { Some(acc.usage) },
    })
}

/// Builds the request URL and body for the endpoint's API protocol.
fn build_request(
    endpoint: &Endpoint,
    system: &str,
    messages: &[LlmMessage],
    tools: &[Value],
) -> (String, Value) {
    let base_url = endpoint.base_url.trim_end_matches('/');
    if endpoint.is_anthropic() {
        (
            endpoint.messages_url(),
            anthropic_body(endpoint, system, messages, tools),
        )
    } else if endpoint.is_openai_responses() {
        (
            endpoint.responses_endpoint_url(),
            build_responses_body(endpoint, system, messages, tools),
        )
    } else {
        (
            format!("{base_url}/chat/completions"),
            openai_body(endpoint, system, messages, tools),
        )
    }
}

/// Resolves the `/v1/responses` endpoint from a provider `baseUrl`:
/// `/v1`-suffixed base URLs append `/responses`, bare hosts append
/// `/v1/responses`.
fn responses_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.rsplit('/').next() == Some("v1") {
        format!("{base}/responses")
    } else {
        format!("{base}/v1/responses")
    }
}

/// Extracts a base64 image attached by the `view_image` tool from a tool
/// response: `(compact_json, path, mime_type, data_base64)` where
/// `compact_json` is the response with `data_base64`/`data_url` stripped
/// (the raw base64 blob is delivered as a real image part instead of text).
fn extract_tool_image(content: &str) -> Option<(String, String, String, String)> {
    let value: Value = serde_json::from_str(content).ok()?;
    let object = value.as_object()?;
    let b64 = object.get("data_base64").and_then(Value::as_str)?;
    let mime = object.get("mime_type").and_then(Value::as_str)?;
    let path = object
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut compact = object.clone();
    compact.remove("data_base64");
    compact.remove("data_url");
    Some((
        Value::Object(compact).to_string(),
        path,
        mime.to_string(),
        b64.to_string(),
    ))
}

/// Builds a Codex OpenAI `/v1/responses` (Responses WireAPI) request body:
/// system prompt becomes `instructions`, the conversation becomes `input`
/// items (`message`, `function_call`, `function_call_output`) and tools are
/// flattened to the `{ type: "function", name, description, parameters }`
/// shape the Responses API expects.
fn build_responses_body(
    endpoint: &Endpoint,
    system: &str,
    messages: &[LlmMessage],
    tools: &[Value],
) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), json!(endpoint.model));
    body.insert("stream".into(), json!(true));
    if endpoint.quirk == ProviderQuirk::ChatgptOauth {
        // ChatGPT sign-in tokens: nothing is stored server side.
        body.insert("store".into(), json!(false));
    }
    if !system.trim().is_empty() {
        body.insert("instructions".into(), json!(system));
    }

    let mut input: Vec<Value> = Vec::new();
    for message in messages {
        match message.role.as_str() {
            "assistant" => {
                if !message.content.is_empty() {
                    input.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": message.content }],
                    }));
                }
                if let Some(tool_calls) = message.tool_calls.as_ref() {
                    for call in tool_calls {
                        input.push(json!({
                            "type": "function_call",
                            "call_id": call.get("id").cloned().unwrap_or(Value::Null),
                            "name": call.pointer("/function/name").cloned().unwrap_or(Value::Null),
                            "arguments": call.pointer("/function/arguments").cloned().unwrap_or(json!("{}")),
                        }));
                    }
                }
            }
            "tool" => {
                match extract_tool_image(&message.content) {
                    Some((compact_json, _path, mime, b64)) => {
                        // Real multimodal tool result: compact JSON text plus
                        // the image as a native `input_image` part.
                        input.push(json!({
                            "type": "function_call_output",
                            "call_id": message.tool_call_id.clone().unwrap_or_default(),
                            "output": [
                                { "type": "input_text", "text": compact_json },
                                { "type": "input_image", "image_url": format!("data:{mime};base64,{b64}") },
                            ],
                        }));
                    }
                    None => {
                        input.push(json!({
                            "type": "function_call_output",
                            "call_id": message.tool_call_id.clone().unwrap_or_default(),
                            "output": message.content,
                        }));
                    }
                }
            }
            role => {
                input.push(json!({
                    "type": "message",
                    "role": role,
                    "content": message.content,
                }));
            }
        }
    }
    body.insert("input".into(), json!(input));

    if !tools.is_empty() {
        let converted: Vec<Value> = tools
            .iter()
            .filter_map(|tool| {
                // Accept both the chat-completions nested form and the
                // already-flat Responses form.
                let function = tool.get("function").unwrap_or(tool);
                Some(json!({
                    "type": "function",
                    "name": function.get("name").cloned().unwrap_or(Value::Null),
                    "description": function.get("description").cloned().unwrap_or(Value::Null),
                    "parameters": function.get("parameters").cloned().unwrap_or(json!({ "type": "object", "properties": {} })),
                }))
            })
            .collect();
        body.insert("tools".into(), json!(converted));
    }

    if let Some(effort) = endpoint.reasoning_effort.as_deref().filter(|e| !e.is_empty()) {
        if endpoint.reasoning_supported {
            body.insert("reasoning".into(), json!({ "effort": effort }));
        }
    }
    // ChatGPT sign-in tokens reject `max_output_tokens`; the prompt-cache
    // fields and `configuration_update` items pi omits are never built here.
    if let Some(max_tokens) = endpoint.max_tokens.filter(|t| *t > 0 && endpoint.quirk != ProviderQuirk::ChatgptOauth) {
        body.insert("max_output_tokens".into(), json!(max_tokens));
    }
    Value::Object(body)
}

/// Parses one SSE line from an OpenAI `/v1/responses` stream into its JSON
/// event. Returns `None` for `event:`/comment lines, `[DONE]` sentinels and
/// non-JSON payloads.
fn parse_responses_sse_line(line: &str) -> Option<Value> {
    let data = line.trim().strip_prefix("data:")?.trim();
    if data.is_empty() || data == "[DONE]" {
        return None;
    }
    serde_json::from_str::<Value>(data).ok()
}

/// Streams one OpenAI `/v1/responses` completion, forwarding
/// `(content_delta, reasoning_delta)` through `on_delta` (same contract as
/// [`stream_chat`], which dispatches here for Responses endpoints).
#[allow(clippy::too_many_arguments)]
pub async fn stream_openai_responses(
    http: &reqwest::Client,
    endpoint: &Endpoint,
    system: &str,
    messages: &[LlmMessage],
    tools: &[Value],
    on_delta: &mut (dyn FnMut(&str, &str) + Send),
    should_stop: &(dyn Fn() -> bool + Send + Sync),
) -> Result<StreamOutcome> {
    let base_url = endpoint.base_url.trim_end_matches('/').to_string();
    if base_url.is_empty() {
        anyhow::bail!("No baseUrl configured for provider endpoint");
    }

    let url = endpoint.responses_endpoint_url();
    let request_body = build_responses_body(endpoint, system, messages, tools);
    let extra_headers = quirk_headers(endpoint, messages);
    let response = send_with_retry(|| {
        let request = http.post(&url);
        apply_provider_headers(request, endpoint, &base_url, &extra_headers).json(&request_body)
    })
    .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Provider returned {status}: {}", summarize_error(&body));
    }

    consume_response(response, endpoint, on_delta, should_stop).await
}

fn openai_body(endpoint: &Endpoint, system: &str, messages: &[LlmMessage], tools: &[Value]) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), json!(endpoint.model));
    body.insert("stream".into(), json!(true));

    let mut chat: Vec<Value> = Vec::new();
    if !system.trim().is_empty() {
        chat.push(json!({ "role": "system", "content": system }));
    }
    for message in messages {
        match message.role.as_str() {
            "assistant" => {
                let mut entry = Map::new();
                entry.insert("role".into(), json!("assistant"));
                // Only send non-empty content: some upstreams reject a null or
                // empty `content` alongside tool_calls.
                if !message.content.is_empty() {
                    entry.insert("content".into(), json!(message.content));
                }
                if let Some(tool_calls) = message.tool_calls.as_ref().filter(|calls| !calls.is_empty()) {
                    // Ensure every call carries the `type: "function"` marker.
                    let typed: Vec<Value> = tool_calls
                        .iter()
                        .map(|call| {
                            let mut call = call.clone();
                            if call.is_object() && call.get("type").is_none() {
                                call["type"] = json!("function");
                            }
                            call
                        })
                        .collect();
                    entry.insert("tool_calls".into(), json!(typed));
                }
                chat.push(Value::Object(entry));
            }
            "tool" => {
                match extract_tool_image(&message.content) {
                    Some((compact_json, path, mime, b64)) => {
                        // Chat Completions has no native tool-image channel:
                        // the compact JSON goes into the tool message and the
                        // image rides along in a synthetic user message.
                        chat.push(json!({
                            "role": "tool",
                            "tool_call_id": message.tool_call_id.clone().unwrap_or_default(),
                            "content": compact_json,
                        }));
                        chat.push(json!({
                            "role": "user",
                            "content": [
                                { "type": "text", "text": format!("Attached image from view_image ({path}):") },
                                { "type": "image_url", "image_url": { "url": format!("data:{mime};base64,{b64}") } },
                            ],
                        }));
                    }
                    None => {
                        chat.push(json!({
                            "role": "tool",
                            "tool_call_id": message.tool_call_id.clone().unwrap_or_default(),
                            "content": message.content,
                        }));
                    }
                }
            }
            role => {
                chat.push(json!({ "role": role, "content": message.content }));
            }
        }
    }
    body.insert("messages".into(), json!(chat));

    if !tools.is_empty() {
        body.insert("tools".into(), json!(tools));
    }
    if let Some(effort) = endpoint.reasoning_effort.as_deref().filter(|e| !e.is_empty()) {
        if endpoint.reasoning_supported {
            body.insert("reasoning_effort".into(), json!(effort));
        }
    }
    if let Some(max_tokens) = endpoint.max_tokens.filter(|t| *t > 0) {
        body.insert("max_tokens".into(), json!(max_tokens));
    }
    Value::Object(body)
}

fn anthropic_body(endpoint: &Endpoint, system: &str, messages: &[LlmMessage], tools: &[Value]) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), json!(endpoint.model));
    body.insert("stream".into(), json!(true));
    if !system.trim().is_empty() {
        body.insert("system".into(), json!(system));
    }

    let mut chat: Vec<Value> = Vec::new();
    for message in messages {
        match message.role.as_str() {
            "assistant" => {
                let mut blocks: Vec<Value> = Vec::new();
                if !message.content.is_empty() {
                    blocks.push(json!({ "type": "text", "text": message.content }));
                }
                if let Some(tool_calls) = message.tool_calls.as_ref() {
                    for call in tool_calls {
                        let input = serde_json::from_str::<Value>(
                            call.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}"),
                        )
                        .unwrap_or_else(|_| json!({}));
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": call.get("id").cloned().unwrap_or(Value::Null),
                            "name": call.pointer("/function/name").cloned().unwrap_or(Value::Null),
                            "input": input,
                        }));
                    }
                }
                if blocks.is_empty() {
                    blocks.push(json!({ "type": "text", "text": "" }));
                }
                chat.push(json!({ "role": "assistant", "content": blocks }));
            }
            "tool" => {
                let tool_result = match extract_tool_image(&message.content) {
                    Some((compact_json, _path, mime, b64)) => {
                        // Native Anthropic multimodal tool_result: a text
                        // block plus a base64 image block.
                        json!({
                            "type": "tool_result",
                            "tool_use_id": message.tool_call_id.clone().unwrap_or_default(),
                            "content": [
                                { "type": "text", "text": compact_json },
                                { "type": "image", "source": { "type": "base64", "media_type": mime, "data": b64 } },
                            ],
                        })
                    }
                    None => json!({
                        "type": "tool_result",
                        "tool_use_id": message.tool_call_id.clone().unwrap_or_default(),
                        "content": message.content,
                    }),
                };
                // Tool results must live in the following user message;
                // consecutive tool messages merge into one.
                let merge = match chat.last_mut() {
                    Some(Value::Object(last))
                        if last.get("role").and_then(Value::as_str) == Some("user")
                            && last.get("content").map(Value::is_array).unwrap_or(false) =>
                    {
                        true
                    }
                    _ => false,
                };
                if merge {
                    if let Some(Value::Array(blocks)) = chat.last_mut().unwrap().get_mut("content") {
                        blocks.push(tool_result);
                    }
                } else {
                    chat.push(json!({ "role": "user", "content": [tool_result] }));
                }
            }
            role => {
                chat.push(json!({ "role": role, "content": message.content }));
            }
        }
    }
    body.insert("messages".into(), json!(chat));

    if !tools.is_empty() {
        let converted: Vec<Value> = tools
            .iter()
            .filter_map(|tool| {
                let function = tool.get("function")?;
                Some(json!({
                    "name": function.get("name").cloned().unwrap_or(Value::Null),
                    "description": function.get("description").cloned().unwrap_or(Value::Null),
                    "input_schema": function.get("parameters").cloned().unwrap_or(json!({ "type": "object", "properties": {} })),
                }))
            })
            .collect();
        body.insert("tools".into(), json!(converted));
    }

    let mut max_tokens = endpoint.max_tokens.filter(|t| *t > 0).unwrap_or(8192);
    if let Some(effort) = endpoint.reasoning_effort.as_deref().filter(|e| !e.is_empty()) {
        let budget = match effort {
            "low" | "minimal" => 2048,
            "high" | "max" => 8192,
            _ => 4096,
        };
        max_tokens = max_tokens.max(budget + 1024);
        body.insert("thinking".into(), json!({ "type": "enabled", "budget_tokens": budget }));
    }
    body.insert("max_tokens".into(), json!(max_tokens));
    Value::Object(body)
}

/// Wraps a non-streaming OpenAI response so `feed_openai` can normalize it.
fn non_streaming_openai(body: &Value) -> Value {
    let mut chunk = json!({ "usage": body.get("usage").cloned().unwrap_or(Value::Null) });
    if let Some(message) = body.pointer("/choices/0/message") {
        let mut delta = Map::new();
        delta.insert("content".into(), message.get("content").cloned().unwrap_or(Value::Null));
        delta.insert(
            "reasoning_content".into(),
            message
                .get("reasoning_content")
                .or_else(|| message.get("reasoning"))
                .cloned()
                .unwrap_or(Value::Null),
        );
        delta.insert("tool_calls".into(), message.get("tool_calls").cloned().unwrap_or(Value::Null));
        chunk["choices"] = json!([{ "delta": delta }]);
    }
    chunk
}

/// Wraps a non-streaming Anthropic content block for replay through
/// `feed_anthropic`'s `content_block_start` handler.
fn normalize_anthropic_block(block: &Value) -> Value {
    if block.get("type").and_then(Value::as_str) == Some("tool_use") {
        block.clone()
    } else {
        json!({ "type": "text" })
    }
}

/// Wraps a non-streaming Anthropic usage object for replay.

/// Short human-readable error summary from an API error body.
fn summarize_error(body: &str) -> String {
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
mod tests {
    use super::*;

    #[test]
    fn openai_stream_accumulates_content_reasoning_and_tool_calls() {
        let mut acc = StreamAcc::default();
        acc.feed_openai(&json!({
            "choices": [{ "delta": { "content": "Hello" } }]
        }));
        acc.feed_openai(&json!({
            "choices": [{ "delta": { "reasoning_content": "thinking..." } }]
        }));
        acc.feed_openai(&json!({
            "choices": [{ "delta": { "tool_calls": [
                { "index": 0, "id": "call-1", "function": { "name": "read_file", "arguments": "{\"pa" } },
                { "index": 1, "id": "call-2", "function": { "name": "list_directory", "arguments": "{}" } }
            ] } }]
        }));
        acc.feed_openai(&json!({
            "choices": [{ "delta": { "tool_calls": [
                { "index": 0, "function": { "arguments": "th\": \"src\"}" } }
            ] }, "finish_reason": "tool_calls" }]
        }));
        acc.feed_openai(&json!({
            "choices": [],
            "usage": { "prompt_tokens": 12, "completion_tokens": 8,
                       "prompt_tokens_details": { "cached_tokens": 4 } }
        }));

        let outcome = acc.tool_calls();
        assert_eq!(outcome.len(), 2);
        assert_eq!(outcome[0]["id"], "call-1");
        assert_eq!(outcome[0]["function"]["name"], "read_file");
        assert_eq!(outcome[0]["function"]["arguments"], r#"{"path": "src"}"#);
        assert_eq!(acc.content, "Hello");
        assert_eq!(acc.reasoning, "thinking...");
        assert_eq!(acc.usage.input_tokens, 12);
        assert_eq!(acc.usage.output_tokens, 8);
        assert_eq!(acc.usage.cache_read_tokens, 4);
        // Deltas arrive in arrival order with the right splits.
        assert_eq!(acc.deltas.len(), 2);
        assert_eq!(acc.deltas[0], ("Hello".to_string(), String::new()));
        assert_eq!(acc.deltas[1], (String::new(), "thinking...".to_string()));
    }

    #[test]
    fn anthropic_stream_accumulates_text_thinking_and_tool_input() {
        let mut acc = StreamAcc::default();
        acc.feed_anthropic(&json!({
            "type": "message_start",
            "message": { "usage": { "input_tokens": 20, "cache_read_input_tokens": 5 } }
        }));
        acc.feed_anthropic(&json!({
            "type": "content_block_start", "index": 0,
            "content_block": { "type": "text" }
        }));
        acc.feed_anthropic(&json!({
            "type": "content_block_delta", "index": 0,
            "delta": { "type": "text_delta", "text": "Hi " }
        }));
        acc.feed_anthropic(&json!({
            "type": "content_block_start", "index": 1,
            "content_block": { "type": "tool_use", "id": "toolu-1", "name": "write_file" }
        }));
        acc.feed_anthropic(&json!({
            "type": "content_block_delta", "index": 1,
            "delta": { "type": "input_json_delta", "partial_json": "{\"path\":\"a" }
        }));
        acc.feed_anthropic(&json!({
            "type": "content_block_delta", "index": 1,
            "delta": { "type": "input_json_delta", "partial_json": ".txt\"}" }
        }));
        acc.feed_anthropic(&json!({
            "type": "message_delta", "usage": { "output_tokens": 9 }
        }));
        acc.feed_anthropic(&json!({ "type": "message_stop" }));

        assert!(acc.finished);
        assert_eq!(acc.content, "Hi ");
        assert_eq!(acc.usage.input_tokens, 20);
        assert_eq!(acc.usage.cache_read_tokens, 5);
        assert_eq!(acc.usage.output_tokens, 9);
        let tool_calls = acc.tool_calls();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["id"], "toolu-1");
        assert_eq!(tool_calls[0]["function"]["name"], "write_file");
        assert_eq!(tool_calls[0]["function"]["arguments"], r#"{"path":"a.txt"}"#);
    }

    #[test]
    fn request_bodies_match_protocols() {
        let endpoint = Endpoint {
            base_url: "https://example.com/v1".into(),
            api: "openai-compatible".into(),
            api_key: "k".into(),
            session_id: String::new(),
            model: "m".into(),
            reasoning_effort: Some("high".into()),
            max_tokens: None,
            reasoning_supported: true,
            quirk: ProviderQuirk::None,
        };
        let messages = vec![
            LlmMessage { role: "user".into(), content: "do it".into(), ..Default::default() },
            LlmMessage {
                role: "assistant".into(),
                content: String::new(),
                tool_calls: Some(vec![json!({ "id": "c1", "function": { "name": "t", "arguments": "{\"a\":1}" } })]),
                ..Default::default()
            },
            LlmMessage { role: "tool".into(), content: "done".into(), tool_call_id: Some("c1".into()), ..Default::default() },
        ];
        let body = openai_body(&endpoint, "sys", &messages, &[]);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][2]["tool_calls"][0]["id"], "c1");
        assert_eq!(body["messages"][3]["tool_call_id"], "c1");
        assert_eq!(body["reasoning_effort"], "high");
        assert!(body["stream"].as_bool().unwrap());

        let anthropic = Endpoint { api: "anthropic".into(), ..endpoint };
        let body = anthropic_body(&anthropic, "sys", &messages, &[]);
        assert_eq!(body["system"], "sys");
        assert_eq!(body["max_tokens"], 9216);
        assert_eq!(body["thinking"]["budget_tokens"], 8192);
        // Assistant message: text block (skipped when empty) + tool_use block.
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(body["messages"][1]["content"][0]["input"], json!({ "a": 1 }));
        // Tool result merged into a user message.
        assert_eq!(body["messages"][2]["role"], "user");
        assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "c1");

        // Tool definitions convert to Anthropic input_schema form.
        let tools = vec![json!({ "type": "function", "function": {
            "name": "read_file", "description": "Read", "parameters": { "type": "object" } } })];
        let body = anthropic_body(&anthropic, "", &[], &tools);
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(body["tools"][0]["name"], "read_file");
    }

    #[test]
    fn responses_request_body_matches_wire_api() {
        let endpoint = Endpoint {
            base_url: "https://api.openai.com/v1".into(),
            api: "openai-responses".into(),
            api_key: "k".into(),
            session_id: String::new(),
            model: "gpt-5".into(),
            reasoning_effort: Some("high".into()),
            max_tokens: Some(1024),
            reasoning_supported: true,
            quirk: ProviderQuirk::None,
        };
        let messages = vec![
            LlmMessage { role: "user".into(), content: "do it".into(), ..Default::default() },
            LlmMessage {
                role: "assistant".into(),
                content: "calling".into(),
                tool_calls: Some(vec![json!({ "id": "c1", "function": { "name": "t", "arguments": "{\"a\":1}" } })]),
                ..Default::default()
            },
            LlmMessage { role: "tool".into(), content: "done".into(), tool_call_id: Some("c1".into()), ..Default::default() },
        ];
        let tools = vec![json!({ "type": "function", "function": {
            "name": "read_file", "description": "Read", "parameters": { "type": "object" } } })];
        let body = build_responses_body(&endpoint, "sys", &messages, &tools);

        assert_eq!(body["model"], "gpt-5");
        assert_eq!(body["stream"], true);
        assert_eq!(body["instructions"], "sys");
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["max_output_tokens"], 1024);
        assert!(body.get("messages").is_none());

        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 4);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"], "do it");
        // Assistant message: text item first, then its function_call item.
        assert_eq!(input[1]["type"], "message");
        assert_eq!(input[1]["content"][0]["type"], "output_text");
        assert_eq!(input[1]["content"][0]["text"], "calling");
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "c1");
        assert_eq!(input[2]["name"], "t");
        assert_eq!(input[2]["arguments"], r#"{"a":1}"#);
        assert_eq!(input[3]["type"], "function_call_output");
        assert_eq!(input[3]["call_id"], "c1");
        assert_eq!(input[3]["output"], "done");

        // Tools flatten to the Responses `{ type, name, description,
        // parameters }` form.
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "read_file");
        assert_eq!(body["tools"][0]["description"], "Read");
        assert_eq!(body["tools"][0]["parameters"]["type"], "object");
        // No chat-completions `function` nesting remains.
        assert!(body["tools"][0].get("function").is_none());

        // `reasoning.effort` is omitted when unsupported, and
        // `max_output_tokens` follows `max_tokens`.
        let plain = Endpoint { reasoning_supported: false, ..endpoint };
        let body = build_responses_body(&plain, "", &[], &[]);
        assert!(body.get("reasoning").is_none());
        assert!(body.get("instructions").is_none());

        // URL resolution: `/v1`-suffixed baseUrls append `/responses`, bare
        // hosts append `/v1/responses`.
        assert_eq!(responses_url("https://api.openai.com/v1"), "https://api.openai.com/v1/responses");
        assert_eq!(responses_url("https://api.openai.com/v1/"), "https://api.openai.com/v1/responses");
        assert_eq!(responses_url("https://codex.example.com"), "https://codex.example.com/v1/responses");
    }

    #[test]
    fn openai_responses_stream_accumulates_text_reasoning_and_function_calls() {
        let mut acc = StreamAcc::default();
        acc.feed_responses(&json!({
            "type": "response.output_text.delta", "delta": "Hello"
        }));
        acc.feed_responses(&json!({
            "type": "response.reasoning_summary_text.delta", "delta": "thinking..."
        }));
        acc.feed_responses(&json!({
            "type": "response.reasoning_text.delta", "delta": " more"
        }));
        acc.feed_responses(&json!({
            "type": "response.output_item.added", "output_index": 1,
            "item": { "type": "function_call", "call_id": "call-1", "name": "read_file", "arguments": "" }
        }));
        acc.feed_responses(&json!({
            "type": "response.function_call_arguments.delta", "output_index": 1,
            "delta": "{\"pa"
        }));
        acc.feed_responses(&json!({
            "type": "response.function_call_arguments.delta", "output_index": 1,
            "delta": "th\": \"src\"}"
        }));
        // The done event carries the full final arguments (must not append).
        acc.feed_responses(&json!({
            "type": "response.output_item.done", "output_index": 1,
            "item": { "type": "function_call", "call_id": "call-1", "name": "read_file",
                      "arguments": "{\"path\": \"src\"}" }
        }));
        acc.feed_responses(&json!({
            "type": "response.completed",
            "response": { "usage": { "input_tokens": 12, "output_tokens": 8,
                                     "input_tokens_details": { "cached_tokens": 4 } } }
        }));

        assert!(acc.finished);
        assert_eq!(acc.content, "Hello");
        assert_eq!(acc.reasoning, "thinking... more");
        let tool_calls = acc.tool_calls();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["id"], "call-1");
        assert_eq!(tool_calls[0]["function"]["name"], "read_file");
        assert_eq!(tool_calls[0]["function"]["arguments"], r#"{"path": "src"}"#);
        assert_eq!(acc.usage.input_tokens, 12);
        assert_eq!(acc.usage.output_tokens, 8);
        assert_eq!(acc.usage.cache_read_tokens, 4);
        // Deltas arrive in arrival order with the right splits.
        assert_eq!(acc.deltas.len(), 3);
        assert_eq!(acc.deltas[0], ("Hello".to_string(), String::new()));
        assert_eq!(acc.deltas[1], (String::new(), "thinking...".to_string()));
        assert_eq!(acc.deltas[2], (String::new(), " more".to_string()));
    }

    /// A `view_image` tool output as produced by the tool harness: metadata
    /// plus base64 image data.
    fn view_image_message() -> LlmMessage {
        LlmMessage {
            role: "tool".into(),
            content: json!({
                "ok": true,
                "path": "assets/logo.png",
                "mime_type": "image/png",
                "bytes": 68,
                "width": 1,
                "height": 1,
                "data_base64": "aVZFV19CQVNFNjQ=",
                "data_url": "data:image/png;base64,aVZFV19CQVNFNjQ=",
            })
            .to_string(),
            tool_call_id: Some("c1".into()),
            ..Default::default()
        }
    }

    #[test]
    fn extract_tool_image_strips_base64_fields() {
        let (compact, path, mime, b64) = extract_tool_image(&view_image_message().content).unwrap();
        assert_eq!(path, "assets/logo.png");
        assert_eq!(mime, "image/png");
        assert_eq!(b64, "aVZFV19CQVNFNjQ=");
        let parsed: Value = serde_json::from_str(&compact).unwrap();
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["mime_type"], "image/png");
        assert!(parsed.get("data_base64").is_none());
        assert!(parsed.get("data_url").is_none());
        // Plain tool outputs (no JSON object with data_base64) stay untouched.
        assert!(extract_tool_image("plain text").is_none());
        assert!(extract_tool_image(r#"{"ok":true}"#).is_none());
    }

    #[test]
    fn responses_body_carries_tool_image_as_input_image() {
        let endpoint = Endpoint {
            base_url: "https://api.openai.com/v1".into(),
            api: "openai-responses".into(),
            api_key: "k".into(),
            model: "gpt-5".into(),
            ..Default::default()
        };
        let messages = vec![view_image_message()];
        let body = build_responses_body(&endpoint, "sys", &messages, &[]);
        let item = &body["input"][0];
        assert_eq!(item["type"], "function_call_output");
        assert_eq!(item["call_id"], "c1");
        let output = item["output"].as_array().unwrap();
        assert_eq!(output[0]["type"], "input_text");
        let compact: Value = serde_json::from_str(output[0]["text"].as_str().unwrap()).unwrap();
        assert!(compact.get("data_base64").is_none());
        assert_eq!(compact["mime_type"], "image/png");
        assert_eq!(output[1]["type"], "input_image");
        assert_eq!(
            output[1]["image_url"],
            "data:image/png;base64,aVZFV19CQVNFNjQ="
        );
    }

    #[test]
    fn anthropic_body_carries_tool_image_as_image_block() {
        let endpoint = Endpoint {
            base_url: "https://api.anthropic.com".into(),
            api: "anthropic".into(),
            api_key: "k".into(),
            model: "claude".into(),
            ..Default::default()
        };
        let messages = vec![view_image_message()];
        let body = anthropic_body(&endpoint, "sys", &messages, &[]);
        let result = &body["messages"][0]["content"][0];
        assert_eq!(result["type"], "tool_result");
        assert_eq!(result["tool_use_id"], "c1");
        let content = result["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        let compact: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert!(compact.get("data_base64").is_none());
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["type"], "base64");
        assert_eq!(content[1]["source"]["media_type"], "image/png");
        assert_eq!(content[1]["source"]["data"], "aVZFV19CQVNFNjQ=");
    }

    #[test]
    fn openai_body_carries_tool_image_in_synthetic_user_message() {
        let endpoint = Endpoint {
            base_url: "https://example.com/v1".into(),
            api: "openai-compatible".into(),
            api_key: "k".into(),
            model: "m".into(),
            ..Default::default()
        };
        let messages = vec![view_image_message()];
        let body = openai_body(&endpoint, "sys", &messages, &[]);
        let chat = body["messages"].as_array().unwrap();
        // system message + tool message + synthetic user message.
        assert_eq!(chat.len(), 3);
        // The tool message carries only the compact JSON.
        assert_eq!(chat[1]["role"], "tool");
        assert_eq!(chat[1]["tool_call_id"], "c1");
        let compact: Value = serde_json::from_str(chat[1]["content"].as_str().unwrap()).unwrap();
        assert!(compact.get("data_base64").is_none());
        assert_eq!(compact["path"], "assets/logo.png");
        // Followed by a synthetic user message with the image part.
        assert_eq!(chat[2]["role"], "user");
        let content = chat[2]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert!(content[0]["text"]
            .as_str()
            .unwrap()
            .contains("view_image (assets/logo.png)"));
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(
            content[1]["image_url"]["url"],
            "data:image/png;base64,aVZFV19CQVNFNjQ="
        );
    }

    #[test]
    fn parse_responses_sse_line_skips_sentinels_and_parses_events() {
        assert!(parse_responses_sse_line("event: response.output_text.delta").is_none());
        assert!(parse_responses_sse_line(": keep-alive").is_none());
        assert!(parse_responses_sse_line("data: ").is_none());
        assert!(parse_responses_sse_line("data: [DONE]").is_none());
        assert!(parse_responses_sse_line("data: not json").is_none());
        let event = parse_responses_sse_line(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hi\"}",
        )
        .unwrap();
        assert_eq!(event["type"], "response.output_text.delta");
        assert_eq!(event["delta"], "Hi");
    }

    fn quirk_endpoint(quirk: ProviderQuirk, api: &str, base_url: &str) -> Endpoint {
        Endpoint {
            base_url: base_url.into(),
            api: api.into(),
            api_key: "token-secret".into(),
            model: "m".into(),
            max_tokens: Some(4096),
            quirk,
            ..Default::default()
        }
    }

    fn built_headers(endpoint: &Endpoint, messages: &[LlmMessage]) -> reqwest::header::HeaderMap {
        let request = reqwest::Client::new().post("http://localhost/x");
        let extra = quirk_headers(endpoint, messages);
        apply_provider_headers(request, endpoint, endpoint.base_url.trim_end_matches('/'), &extra)
            .build()
            .unwrap()
            .headers()
            .clone()
    }

    #[test]
    fn quirk_detection_follows_provider_base_url_and_key() {
        let openai = "https://api.openai.com/v1";
        assert_eq!(ProviderQuirk::detect("github-copilot", "https://x", "t"), ProviderQuirk::GithubCopilot);
        assert_eq!(ProviderQuirk::detect("openai", openai, "eyJhbGciOi"), ProviderQuirk::ChatgptOauth);
        assert_eq!(ProviderQuirk::detect("openai", "https://api.openai.com/v1/", "eyJ"), ProviderQuirk::ChatgptOauth);
        assert_eq!(ProviderQuirk::detect("openai", openai, "sk-proj-123"), ProviderQuirk::None);
        assert_eq!(ProviderQuirk::detect("openai", "https://proxy.example/v1", "eyJ"), ProviderQuirk::None);
        assert_eq!(ProviderQuirk::detect("xai", openai, "eyJ"), ProviderQuirk::None);
    }

    #[test]
    fn chatgpt_oauth_body_is_stateless_without_max_output_tokens() {
        let plain = quirk_endpoint(ProviderQuirk::None, "openai-responses", "https://api.openai.com/v1");
        let body = build_responses_body(&plain, "sys", &[], &[]);
        assert_eq!(body["max_output_tokens"], 4096);
        assert!(body.get("store").is_none(), "API-key behavior is unchanged");

        let chatgpt = quirk_endpoint(ProviderQuirk::ChatgptOauth, "openai-responses", "https://api.openai.com/v1");
        let body = build_responses_body(&chatgpt, "sys", &[], &[]);
        assert_eq!(body["store"], false);
        for omitted in ["max_output_tokens", "prompt_cache_retention", "prompt_cache_options", "configuration_update"] {
            assert!(body.get(omitted).is_none(), "{omitted}");
        }
        assert!(body["input"].as_array().unwrap().iter().all(|item| item["type"] != "configuration_update"));
        assert_eq!(body["instructions"], "sys");
        assert_eq!(chatgpt.responses_endpoint_url(), "https://api.openai.com/v1/responses");
        assert_eq!(built_headers(&chatgpt, &[])["authorization"], "Bearer token-secret");
    }

    #[test]
    fn copilot_urls_have_no_version_prefix_except_messages() {
        let base = "https://api.individual.githubcopilot.com";
        let completions = quirk_endpoint(ProviderQuirk::GithubCopilot, "openai-completions", base);
        assert_eq!(build_request(&completions, "", &[], &[]).0, format!("{base}/chat/completions"));
        let responses = quirk_endpoint(ProviderQuirk::GithubCopilot, "openai-responses", base);
        assert_eq!(build_request(&responses, "", &[], &[]).0, format!("{base}/responses"));
        let messages = quirk_endpoint(ProviderQuirk::GithubCopilot, "anthropic-messages", base);
        assert_eq!(build_request(&messages, "", &[], &[]).0, format!("{base}/v1/messages"));
        // Other providers keep their URLs.
        let anthropic = quirk_endpoint(ProviderQuirk::None, "anthropic", "https://api.anthropic.com/v1");
        assert_eq!(build_request(&anthropic, "", &[], &[]).0, "https://api.anthropic.com/v1/messages");
        let openai = quirk_endpoint(ProviderQuirk::None, "openai-responses", "https://codex.example.com");
        assert_eq!(build_request(&openai, "", &[], &[]).0, "https://codex.example.com/v1/responses");
    }

    #[test]
    fn copilot_headers_identify_the_editor_and_the_initiator() {
        let endpoint = quirk_endpoint(ProviderQuirk::GithubCopilot, "openai-completions", "https://api.individual.githubcopilot.com");
        let user_turn = vec![LlmMessage { role: "user".into(), content: "hi".into(), ..Default::default() }];
        let headers = built_headers(&endpoint, &user_turn);
        assert_eq!(headers["authorization"], "Bearer token-secret");
        assert_eq!(headers["user-agent"], "GitHubCopilotChat/0.35.0");
        assert_eq!(headers["editor-version"], "vscode/1.107.0");
        assert_eq!(headers["editor-plugin-version"], "copilot-chat/0.35.0");
        assert_eq!(headers["copilot-integration-id"], "vscode-chat");
        assert_eq!(headers["x-initiator"], "user");
        assert_eq!(headers["openai-intent"], "conversation-edits");
        assert!(headers.get("copilot-vision-request").is_none());

        let agent_turn = vec![
            user_turn[0].clone(),
            LlmMessage { role: "tool".into(), content: "done".into(), tool_call_id: Some("c1".into()), ..Default::default() },
        ];
        assert_eq!(built_headers(&endpoint, &agent_turn)["x-initiator"], "agent");
        let with_image = vec![view_image_message()];
        assert_eq!(built_headers(&endpoint, &with_image)["copilot-vision-request"], "true");
    }

    #[test]
    fn copilot_anthropic_requests_use_a_bearer_not_an_api_key() {
        let endpoint = quirk_endpoint(ProviderQuirk::GithubCopilot, "anthropic-messages", "https://api.individual.githubcopilot.com");
        let headers = built_headers(&endpoint, &[]);
        assert_eq!(headers["authorization"], "Bearer token-secret");
        assert_eq!(headers["anthropic-version"], "2023-06-01");
        assert!(headers.get("x-api-key").is_none());

        let anthropic = quirk_endpoint(ProviderQuirk::None, "anthropic", "https://api.anthropic.com/v1");
        let headers = built_headers(&anthropic, &[]);
        assert_eq!(headers["x-api-key"], "token-secret");
        assert!(headers.get("authorization").is_none() && headers.get("user-agent").is_none());
    }
}
