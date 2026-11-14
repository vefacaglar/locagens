//! Anthropic Messages request body.

use super::*;

/// `max_tokens` when the catalog has none: Claude 4.6+ models get room for
/// thinking plus a large tool call, older ones keep the classic default.
const DEFAULT_MAX_TOKENS: i64 = 8192;
const DEFAULT_MAX_TOKENS_ADAPTIVE: i64 = 32_000;

pub(super) fn anthropic_body(endpoint: &Endpoint, system: &str, messages: &[LlmMessage], tools: &[Value]) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), json!(endpoint.model));
    body.insert("stream".into(), json!(true));
    if !system.trim().is_empty() {
        // Cache breakpoint 1: the system prompt.
        body.insert("system".into(), json!([{ "type": "text", "text": system, "cache_control": ephemeral() }]));
    }

    let mut chat: Vec<Value> = Vec::new();
    for message in messages {
        match message.role.as_str() {
            "assistant" => chat.push(json!({ "role": "assistant", "content": assistant_blocks(message) })),
            "tool" => push_tool_result(&mut chat, message),
            role => {
                chat.push(json!({ "role": role, "content": message_content(message) }));
            }
        }
    }
    // Cache breakpoint 2: the conversation so far, so each round of a tool
    // loop reads the previous rounds from the cache.
    mark_last_block_cacheable(&mut chat);
    body.insert("messages".into(), json!(chat));

    if !tools.is_empty() {
        let mut converted: Vec<Value> = tools
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
        // Cache breakpoint 3: the tool list.
        if let Some(Value::Object(last)) = converted.last_mut() {
            last.insert("cache_control".into(), ephemeral());
        }
        body.insert("tools".into(), json!(converted));
    }

    apply_thinking(&mut body, endpoint, messages);
    Value::Object(body)
}

/// A message's `content`: the plain string, or base64 `image` blocks + a text
/// block when the message carries images.
fn message_content(message: &LlmMessage) -> Value {
    if message.images.is_empty() {
        return json!(message.content);
    }
    let mut blocks: Vec<Value> = message
        .images
        .iter()
        .map(|image| {
            json!({
                "type": "image",
                "source": { "type": "base64", "media_type": image.mime, "data": image.data },
            })
        })
        .collect();
    if !message.content.is_empty() {
        blocks.push(json!({ "type": "text", "text": message.content }));
    }
    json!(blocks)
}

fn ephemeral() -> Value {
    json!({ "type": "ephemeral" })
}

/// Thinking blocks first (replayed unchanged, signature included, as the
/// Messages API requires in a tool loop), then text, then `tool_use`.
fn assistant_blocks(message: &LlmMessage) -> Vec<Value> {
    let mut blocks: Vec<Value> = message.thinking_blocks.clone().unwrap_or_default();
    if !message.content.is_empty() {
        blocks.push(json!({ "type": "text", "text": message.content }));
    }
    for call in message.tool_calls.iter().flatten() {
        let input = serde_json::from_str::<Value>(call.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}"))
            .unwrap_or_else(|_| json!({}));
        blocks.push(json!({
            "type": "tool_use",
            "id": call.get("id").cloned().unwrap_or(Value::Null),
            "name": call.pointer("/function/name").cloned().unwrap_or(Value::Null),
            "input": input,
        }));
    }
    blocks
}

/// Tool results live in the following user message; consecutive tool
/// messages merge into one.
fn push_tool_result(chat: &mut Vec<Value>, message: &LlmMessage) {
    let tool_use_id = message.tool_call_id.clone().unwrap_or_default();
    let tool_result = match extract_tool_image(&message.content) {
        // Native multimodal tool_result: a text block plus a base64 image.
        Some((compact_json, _path, mime, b64)) => json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": [
                { "type": "text", "text": compact_json },
                { "type": "image", "source": { "type": "base64", "media_type": mime, "data": b64 } },
            ],
        }),
        None => json!({ "type": "tool_result", "tool_use_id": tool_use_id, "content": message.content }),
    };
    if let Some(Value::Array(blocks)) = chat
        .last_mut()
        .filter(|last| last.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(|last| last.get_mut("content"))
    {
        blocks.push(tool_result);
        return;
    }
    chat.push(json!({ "role": "user", "content": [tool_result] }));
}

/// Adds `cache_control` to the last content block of the last message
/// (a string content becomes one text block). Thinking blocks and empty
/// text cannot carry it and are left alone.
fn mark_last_block_cacheable(chat: &mut [Value]) {
    let Some(content) = chat.last_mut().and_then(|last| last.get_mut("content")) else { return };
    if let Some(text) = content.as_str().filter(|text| !text.is_empty()) {
        *content = json!([{ "type": "text", "text": text }]);
    }
    let Some(Value::Object(block)) = content.as_array_mut().and_then(|blocks| blocks.last_mut()) else { return };
    let kind = block.get("type").and_then(Value::as_str).unwrap_or_default();
    let empty_text = kind == "text" && block.get("text").and_then(Value::as_str).is_none_or(str::is_empty);
    if !matches!(kind, "thinking" | "redacted_thinking") && !empty_text {
        block.insert("cache_control".into(), ephemeral());
    }
}

/// `thinking` / `output_config` / `max_tokens` for the effort. Claude 4.6+
/// (and Fable / Mythos) take adaptive thinking with an effort level and
/// reject `budget_tokens`; older and unknown models keep the budget form.
fn apply_thinking(body: &mut Map<String, Value>, endpoint: &Endpoint, messages: &[LlmMessage]) {
    let adaptive = uses_adaptive_thinking(&endpoint.model);
    let default_max = if adaptive { DEFAULT_MAX_TOKENS_ADAPTIVE } else { DEFAULT_MAX_TOKENS };
    let mut max_tokens = endpoint.max_tokens.filter(|t| *t > 0).unwrap_or(default_max);
    let effort = endpoint
        .reasoning_effort
        .as_deref()
        .map(str::trim)
        .filter(|effort| !effort.is_empty() && !matches!(*effort, "off" | "none"));
    if let Some(effort) = effort {
        if adaptive {
            body.insert("thinking".into(), json!({ "type": "adaptive" }));
            body.insert("output_config".into(), json!({ "effort": adaptive_effort(effort) }));
        } else if !open_tool_turn_lacks_thinking(messages) {
            let budget = match effort {
                "low" | "minimal" => 2048,
                "high" | "xhigh" | "max" => 8192,
                _ => 4096,
            };
            max_tokens = max_tokens.max(budget + 1024);
            body.insert("thinking".into(), json!({ "type": "enabled", "budget_tokens": budget }));
        }
    }
    body.insert("max_tokens".into(), json!(max_tokens));
}

/// The budget form requires the last assistant message of an open tool loop
/// to start with a thinking block; history from a turn without thinking
/// (another model, effort just switched on) has none, so that request runs
/// without thinking instead of failing with HTTP 400.
fn open_tool_turn_lacks_thinking(messages: &[LlmMessage]) -> bool {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .is_some_and(|message| message.tool_calls.is_some() && message.thinking_blocks.is_none())
}

fn adaptive_effort(effort: &str) -> &str {
    match effort {
        "minimal" => "low",
        "low" | "medium" | "high" | "xhigh" | "max" => effort,
        _ => "medium",
    }
}

/// Whether a model id is Claude 4.6 or later, or a Fable / Mythos model
/// (`claude-opus-4-6`, `us.anthropic.claude-sonnet-5-5-v1:0`,
/// `anthropic/claude-fable-5-2`). Dated snapshots such as
/// `claude-sonnet-4-20250514` and the `claude-3-7-sonnet` naming are older.
pub(super) fn uses_adaptive_thinking(model: &str) -> bool {
    let id = model.to_lowercase();
    let Some((_, rest)) = id.split_once("claude-") else { return false };
    let parts: Vec<&str> = rest.split(['-', '.', '@']).collect();
    let family = parts.first().copied().unwrap_or_default();
    if matches!(family, "fable" | "mythos") {
        return true;
    }
    if !matches!(family, "opus" | "sonnet" | "haiku") {
        return false;
    }
    let number = |index: usize| -> u32 {
        parts.get(index).filter(|part| part.len() <= 2).and_then(|part| part.parse().ok()).unwrap_or(0)
    };
    let (major, minor) = (number(1), number(2));
    major > 4 || (major == 4 && minor >= 6)
}
