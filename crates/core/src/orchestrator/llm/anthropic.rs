//! Anthropic Messages request body.

use super::*;

pub(super) fn anthropic_body(endpoint: &Endpoint, system: &str, messages: &[LlmMessage], tools: &[Value]) -> Value {
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
                let merge = matches!(chat.last_mut(), Some(Value::Object(last))
                    if last.get("role").and_then(Value::as_str) == Some("user")
                        && last.get("content").is_some_and(Value::is_array));
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


/// Wraps a non-streaming Anthropic content block for replay through
/// `feed_anthropic`'s `content_block_start` handler.
pub(super) fn normalize_anthropic_block(block: &Value) -> Value {
    if block.get("type").and_then(Value::as_str) == Some("tool_use") {
        block.clone()
    } else {
        json!({ "type": "text" })
    }
}
