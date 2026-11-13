//! OpenAI-compatible chat completions request body.

use super::*;

pub(super) fn openai_body(endpoint: &Endpoint, system: &str, messages: &[LlmMessage], tools: &[Value]) -> Value {
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


/// Wraps a non-streaming OpenAI response so `feed_openai` can normalize it.
pub(super) fn non_streaming_openai(body: &Value) -> Value {
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
