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
//!
//! Deltas are forwarded through a callback so the orchestrator can update the
//! live assistant `RunMessage` and emit throttled `message_updated` SSE events.

use anyhow::Result;
use serde_json::{json, Map, Value};

/// One chat message in provider-neutral form. Tool calls are kept in the
/// OpenAI shape (`{ id, function: { name, arguments } }`) — the normalized
/// shape used in `RunMessage.rawResponse` — and converted per API protocol
/// when the request body is built.
#[derive(Clone, Debug, Default)]
pub struct LlmMessage {
    pub role: String,
    pub content: String,
    /// `tool_call_id` for `role == "tool"` responses.
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
    /// `"openai-compatible"` or `"anthropic"`.
    pub api: String,
    pub api_key: String,
    /// Per-conversation routing id (OpenCode requires `x-opencode-session`).
    pub session_id: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub max_tokens: Option<i64>,
    /// Whether the model accepts a `reasoning_effort` parameter.
    pub reasoning_supported: bool,
}

impl Endpoint {
    pub fn is_anthropic(&self) -> bool {
        self.api == "anthropic"
            || self.api == "anthropic-messages"
            || self.base_url.contains("anthropic")
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

    let (url, mut request_body) = build_request(endpoint, system, messages, tools);
    // Ask for token accounting on OpenAI-compatible streams; providers that
    // reject the field get an automatic retry without it.
    let with_stream_options = !endpoint.is_anthropic();
    if with_stream_options {
        request_body["stream_options"] = json!({ "include_usage": true });
    }

    let mut request = http.post(&url);
    if endpoint.is_anthropic() {
        request = request
            .header("x-api-key", &endpoint.api_key)
            .header("anthropic-version", "2023-06-01");
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
    let response = request
        .json(&request_body)
        .send()
        .await
        .map_err(|err| anyhow::anyhow!("Provider request failed: {err}"))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        // Retry once without `stream_options` when the provider rejects it.
        if with_stream_options && body.contains("stream_options") {
            request_body.as_object_mut().unwrap().remove("stream_options");
            let retry = http
                .post(&url)
                .json(&request_body)
                .send()
                .await
                .map_err(|err| anyhow::anyhow!("Provider request failed: {err}"))?;
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
            format!("{base_url}/messages"),
            anthropic_body(endpoint, system, messages, tools),
        )
    } else {
        (
            format!("{base_url}/chat/completions"),
            openai_body(endpoint, system, messages, tools),
        )
    }
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
                chat.push(json!({
                    "role": "tool",
                    "tool_call_id": message.tool_call_id.clone().unwrap_or_default(),
                    "content": message.content,
                }));
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
                let tool_result = json!({
                    "type": "tool_result",
                    "tool_use_id": message.tool_call_id.clone().unwrap_or_default(),
                    "content": message.content,
                });
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
}
