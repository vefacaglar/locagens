//! Protocol-agnostic accumulator fed with parsed stream events.

use super::*;

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
    /// Argument length already reported per tool slot (`take_tool_progress`).
    reported_tool_args: std::collections::BTreeMap<usize, usize>,
    pub usage: Usage,
    /// A terminal event arrived (`finish_reason`, `message_stop`,
    /// `response.completed` / `.incomplete`): the answer is complete.
    pub finished: bool,
    /// Why generation stopped (`stop`, `length`, `max_tokens`, ...).
    pub stop_reason: Option<String>,
    /// Anthropic `thinking` / `redacted_thinking` blocks by content index.
    thinking: std::collections::BTreeMap<usize, Value>,
}

#[derive(Default)]
pub(super) struct ToolAcc {
    id: String,
    name: String,
    args: String,
}

/// Arguments accumulated so far for one streaming tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolArgsProgress {
    /// Stable per-response slot of the call (matches the provider index).
    pub index: usize,
    pub call_id: String,
    pub name: String,
    /// Raw (partial) JSON arguments received so far.
    pub arguments: String,
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
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.stop_reason = Some(reason.to_string());
            self.finished = true;
        }
    }

    /// Takes the prompt-side counts of an Anthropic `usage` object, never
    /// lowering one already seen (start and delta counts are cumulative).
    /// Anthropic's `input_tokens` leaves the cached part out;
    /// `Usage.input_tokens` is the whole prompt everywhere.
    fn merge_anthropic_input_usage(&mut self, usage: &Value) {
        let count = |key: &str| usage.get(key).and_then(Value::as_i64).unwrap_or(0);
        let uncached = self.usage.input_tokens - self.usage.cache_read_tokens - self.usage.cache_write_tokens;
        self.usage.cache_read_tokens = self.usage.cache_read_tokens.max(count("cache_read_input_tokens"));
        self.usage.cache_write_tokens = self.usage.cache_write_tokens.max(count("cache_creation_input_tokens"));
        self.usage.input_tokens =
            uncached.max(count("input_tokens")) + self.usage.cache_read_tokens + self.usage.cache_write_tokens;
    }

    /// Consumes one Anthropic Messages streaming event.
    pub fn feed_anthropic(&mut self, event: &Value) {
        match event.get("type").and_then(Value::as_str).unwrap_or_default() {
            "message_start" => {
                if let Some(usage) = event.pointer("/message/usage") {
                    self.merge_anthropic_input_usage(usage);
                }
            }
            "content_block_start" => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                if let Some(block) = event.get("content_block") {
                    self.start_anthropic_block(index, block);
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
                                self.extend_thinking(event, "thinking", text);
                            }
                        }
                    }
                    "signature_delta" => {
                        if let Some(signature) = delta.get("signature").and_then(Value::as_str) {
                            self.extend_thinking(event, "signature", signature);
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
                if let Some(output) = event.pointer("/usage/output_tokens").and_then(Value::as_i64) {
                    self.usage.output_tokens = output;
                }
                // The delta's counts are cumulative; some servers (OpenCode's
                // Messages endpoint) report the prompt only here.
                if let Some(usage) = event.get("usage") {
                    self.merge_anthropic_input_usage(usage);
                }
                if let Some(reason) = event.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop_reason = Some(reason.to_string());
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
            "response.completed" | "response.incomplete" => {
                if let Some(reason) = event.pointer("/response/incomplete_details/reason").and_then(Value::as_str) {
                    self.stop_reason = Some(reason.to_string());
                }
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

    /// `content_block_start`: tool calls and thinking blocks get a slot.
    fn start_anthropic_block(&mut self, index: usize, block: &Value) {
        match block.get("type").and_then(Value::as_str).unwrap_or_default() {
            "tool_use" => {
                self.tool_acc.insert(index, ToolAcc {
                    id: block.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                    name: block.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                    args: String::new(),
                });
            }
            "thinking" => {
                let text = block.get("thinking").and_then(Value::as_str).unwrap_or_default();
                if !text.is_empty() {
                    self.reasoning.push_str(text);
                    self.deltas.push((String::new(), text.to_string()));
                }
                let signature = block.get("signature").and_then(Value::as_str).unwrap_or_default();
                self.thinking.insert(index, json!({ "type": "thinking", "thinking": text, "signature": signature }));
            }
            "redacted_thinking" => {
                self.thinking.insert(index, block.clone());
            }
            _ => {}
        }
    }

    /// Appends `text` to field `key` of the thinking block an event targets.
    fn extend_thinking(&mut self, event: &Value, key: &str, text: &str) {
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        if let Some(Value::Object(block)) = self.thinking.get_mut(&index) {
            let current = block.get(key).and_then(Value::as_str).unwrap_or_default();
            let joined = format!("{current}{text}");
            block.insert(key.to_string(), json!(joined));
        }
    }

    /// Thinking blocks to replay with the assistant message. A `thinking`
    /// block without a signature cannot be replayed and is left out.
    pub fn thinking_blocks(&self) -> Vec<Value> {
        self.thinking
            .values()
            .filter(|block| {
                block.get("type").and_then(Value::as_str) == Some("redacted_thinking")
                    || block.get("signature").and_then(Value::as_str).is_some_and(|s| !s.is_empty())
            })
            .cloned()
            .collect()
    }

    /// Whether the output hit the model's output-token limit.
    pub fn truncated(&self) -> bool {
        matches!(self.stop_reason.as_deref(), Some("length" | "max_tokens" | "max_output_tokens"))
    }

    /// Tool-accumulator key for a Responses event: the `output_index` field
    /// when present, otherwise the sole tracked call (or a fresh slot).
    fn responses_tool_index(&self, event: &Value) -> usize {
        match event.get("output_index").and_then(Value::as_u64) {
            Some(i) => i as usize,
            None => self.tool_acc.keys().next().copied().unwrap_or(self.next_tool_index),
        }
    }

    /// Tool calls whose arguments grew since the last call, restricted to
    /// names `wants` accepts (so large unrelated arguments are never cloned).
    pub fn take_tool_progress(&mut self, wants: &dyn Fn(&str) -> bool) -> Vec<ToolArgsProgress> {
        let mut out = Vec::new();
        for (index, acc) in &self.tool_acc {
            let reported = self.reported_tool_args.get(index).copied();
            if acc.name.is_empty() || !wants(&acc.name) || reported == Some(acc.args.len()) {
                continue;
            }
            self.reported_tool_args.insert(*index, acc.args.len());
            out.push(ToolArgsProgress {
                index: *index,
                call_id: acc.id.clone(),
                name: acc.name.clone(),
                arguments: acc.args.clone(),
            });
        }
        out
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
