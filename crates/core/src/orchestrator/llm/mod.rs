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
    stream_chat_observed(http, endpoint, system, messages, tools, on_delta, &mut |_| {}, should_stop).await
}

/// [`stream_chat`] that also reports tool-call argument progress: after each
/// stream event, `on_tool_args` receives the arguments accumulated so far for
/// every `apply_patch` call that grew (see [`StreamAcc::take_tool_progress`]).
#[allow(clippy::too_many_arguments)]
pub async fn stream_chat_observed(
    http: &reqwest::Client,
    endpoint: &Endpoint,
    system: &str,
    messages: &[LlmMessage],
    tools: &[Value],
    on_delta: &mut (dyn FnMut(&str, &str) + Send),
    on_tool_args:&mut (dyn FnMut(&ToolArgsProgress) + Send),
    should_stop: &(dyn Fn() -> bool + Send + Sync),
) -> Result<StreamOutcome> {
    let base_url = endpoint.base_url.trim_end_matches('/').to_string();
    if base_url.is_empty() {
        anyhow::bail!("No baseUrl configured for provider endpoint");
    }

    // Codex Responses WireAPI has its own request/streaming path.
    if endpoint.is_openai_responses() {
        return stream_openai_responses(http, endpoint, system, messages, tools, on_delta, on_tool_args, should_stop)
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
            return consume_response(retry, endpoint, on_delta, on_tool_args, should_stop).await;
        }
        anyhow::bail!("Provider returned {status}: {}", summarize_error(&body));
    }

    consume_response(response, endpoint, on_delta, on_tool_args, should_stop).await
}

/// Drains a successful completion response into a `StreamOutcome` — either
/// the SSE event stream or, when the endpoint ignored `stream: true`, the
/// plain JSON body.
async fn consume_response(
    response: reqwest::Response,
    endpoint: &Endpoint,
    on_delta: &mut (dyn FnMut(&str, &str) + Send),
    on_tool_args: &mut (dyn FnMut(&ToolArgsProgress) + Send),
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
        let body: Value = match tokio::time::timeout(STREAM_IDLE_TIMEOUT, response.json()).await {
            Ok(body) => body.unwrap_or(Value::Null),
            Err(_) => anyhow::bail!("Stream stalled: no data from the provider for {} s", STREAM_IDLE_TIMEOUT.as_secs()),
        };
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

    let mut stream = Box::pin(response.bytes_stream());
    let mut lines = SseLineBuffer::default();
    let mut acc = StreamAcc::default();

    while let Some(bytes) = next_stream_chunk(&mut stream, should_stop).await? {
        lines.push(&bytes);
        while let Some(line) = lines.next_line() {
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
                for progress in acc.take_tool_progress(&|name| name == "apply_patch") {
                    on_tool_args(&progress);
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
            for progress in acc.take_tool_progress(&|name| name == "apply_patch") {
                on_tool_args(&progress);
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


mod anthropic;
mod http;
mod openai_chat;
mod openai_responses;
mod sse;
mod stream_acc;
#[cfg(test)]
mod tests;
mod types;

pub use self::http::shared_http_client;
pub use self::openai_responses::stream_openai_responses;
pub use self::sse::{SseLineBuffer, STREAM_IDLE_TIMEOUT};
pub use self::stream_acc::{StreamAcc, ToolArgsProgress};
pub use self::types::*;

use self::anthropic::*;
use self::http::*;
use self::openai_chat::*;
use self::openai_responses::*;
use self::sse::*;
