//! OpenAI `/v1/responses` (Codex WireAPI) request body and stream.

use super::*;

/// Extracts a base64 image attached by the `view_image` tool from a tool
/// response: `(compact_json, path, mime_type, data_base64)` where
/// `compact_json` is the response with `data_base64`/`data_url` stripped
/// (the raw base64 blob is delivered as a real image part instead of text).
pub(super) fn extract_tool_image(content: &str) -> Option<(String, String, String, String)> {
    if !content.contains("\"data_base64\"") {
        return None;
    }
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

/// A tool result without its attached image bytes: the compact JSON when
/// the result carries a `view_image` payload, else `None` (FR-027).
pub fn tool_image_text(content: &str) -> Option<String> {
    extract_tool_image(content).map(|(compact_json, ..)| compact_json)
}

/// Builds a Codex OpenAI `/v1/responses` (Responses WireAPI) request body:
/// system prompt becomes `instructions`, the conversation becomes `input`
/// items (`message`, `function_call`, `function_call_output`) and tools are
/// flattened to the `{ type: "function", name, description, parameters }`
/// shape the Responses API expects.
pub(super) fn build_responses_body(
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
            .map(|tool| {
                // Accept both the chat-completions nested form and the
                // already-flat Responses form.
                let function = tool.get("function").unwrap_or(tool);
                json!({
                    "type": "function",
                    "name": function.get("name").cloned().unwrap_or(Value::Null),
                    "description": function.get("description").cloned().unwrap_or(Value::Null),
                    "parameters": function.get("parameters").cloned().unwrap_or(json!({ "type": "object", "properties": {} })),
                })
            })
            .collect();
        body.insert("tools".into(), json!(converted));
    }

    if let Some(effort) = endpoint.reasoning_effort.as_deref().filter(|e| !e.is_empty()) {
        if endpoint.reasoning_supported {
            let mut reasoning = json!({ "effort": effort });
            // `model_reasoning_summary` (config.toml): auto|concise|detailed|none.
            let config = crate::config_runtime::current();
            if let Some(summary) = config.model_reasoning_summary.as_deref().filter(|s| !s.is_empty() && *s != "none") {
                reasoning["summary"] = json!(summary);
            }
            body.insert("reasoning".into(), reasoning);
        }
    }
    // `model_verbosity` (config.toml): low|medium|high.
    if let Some(verbosity) = crate::config_runtime::current().model_verbosity.as_deref().filter(|v| !v.is_empty()) {
        body.insert("text".into(), json!({ "verbosity": verbosity }));
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
pub(super) fn parse_responses_sse_line(line: &str) -> Option<Value> {
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
    on_tool_args: &mut (dyn FnMut(&ToolArgsProgress) + Send),
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
        return Err(provider_error(status, &body));
    }

    consume_response(response, endpoint, on_delta, on_tool_args, should_stop).await
}
