use super::*;

/// QA-025: a multibyte character split across two network chunks must
/// survive (it used to become U+FFFD per chunk).
#[test]
fn sse_lines_keep_multibyte_text_split_across_chunks() {
    let line = "data: {\"t\":\"çğü → 日本 🙂\"}\r\n".as_bytes();
    let mut buffer = SseLineBuffer::default();
    // Split inside the 3-byte `→` and inside the 4-byte emoji.
    let arrow = line.windows(3).position(|w| w == "→".as_bytes()).unwrap();
    let emoji = line.windows(4).position(|w| w == "🙂".as_bytes()).unwrap();
    buffer.push(&line[..arrow + 1]);
    assert!(buffer.next_line().is_none());
    buffer.push(&line[arrow + 1..emoji + 2]);
    assert!(buffer.next_line().is_none());
    buffer.push(&line[emoji + 2..]);
    assert_eq!(buffer.next_line().as_deref(), Some("data: {\"t\":\"çğü → 日本 🙂\"}"));
    assert!(buffer.next_line().is_none());
}

#[tokio::test]
async fn stalled_stream_fails_after_the_idle_timeout_and_stops_on_cancel() {
    let mut silent = futures::stream::pending::<std::result::Result<Vec<u8>, String>>();
    let err = next_stream_chunk_within(&mut silent, &|| false, Duration::from_millis(300))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Stream stalled"), "{err}");

    // Cancellation is noticed while no chunk arrives at all.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = stop.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let started = std::time::Instant::now();
    let should_stop = move || stop.load(std::sync::atomic::Ordering::SeqCst);
    let err = next_stream_chunk_within(&mut silent, &should_stop, Duration::from_secs(60))
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "cancelled");
    assert!(started.elapsed() < Duration::from_secs(5));

    // A chunk that keeps arriving resets nothing but is returned as is.
    let mut one = futures::stream::iter(vec![Ok::<_, String>(b"x".to_vec())]);
    let chunk = next_stream_chunk_within(&mut one, &|| false, Duration::from_secs(1)).await.unwrap();
    assert_eq!(chunk, Some(b"x".to_vec()));
    assert_eq!(next_stream_chunk_within(&mut one, &|| false, Duration::from_secs(1)).await.unwrap(), None);
}

#[test]
fn tool_progress_reports_growing_arguments_of_wanted_tools_only() {
    let wants = |name: &str| name == "apply_patch";
    let mut acc = StreamAcc::default();
    acc.feed_openai(&json!({ "choices": [{ "delta": { "tool_calls": [
        { "index": 0, "id": "c1", "function": { "name": "apply_patch", "arguments": "{\"input\":\"*** Be" } },
        { "index": 1, "id": "c2", "function": { "name": "read_file", "arguments": "{\"path\":\"a\"}" } }
    ] } }] }));
    let first = acc.take_tool_progress(&wants);
    assert_eq!(first.len(), 1, "read_file is filtered out");
    assert_eq!((first[0].index, first[0].call_id.as_str()), (0, "c1"));
    assert_eq!(first[0].arguments, "{\"input\":\"*** Be");

    // No growth, nothing reported.
    assert!(acc.take_tool_progress(&wants).is_empty());

    acc.feed_openai(&json!({ "choices": [{ "delta": { "tool_calls": [
        { "index": 0, "function": { "arguments": "gin Patch\\n" } }
    ] } }] }));
    let second = acc.take_tool_progress(&wants);
    assert_eq!(second.len(), 1);
    assert!(second[0].arguments.ends_with("gin Patch\\n"));
}

#[test]
fn tool_progress_works_for_responses_and_anthropic_streams() {
    let wants = |name: &str| name == "apply_patch";
    let mut responses = StreamAcc::default();
    responses.feed_responses(&json!({
        "type": "response.output_item.added", "output_index": 0,
        "item": { "type": "function_call", "call_id": "r1", "name": "apply_patch", "arguments": "" }
    }));
    responses.feed_responses(&json!({ "type": "response.function_call_arguments.delta", "output_index": 0, "delta": "{\"in" }));
    let progress = responses.take_tool_progress(&wants);
    assert_eq!(progress.last().map(|p| p.arguments.as_str()), Some("{\"in"));

    let mut anthropic = StreamAcc::default();
    anthropic.feed_anthropic(&json!({
        "type": "content_block_start", "index": 0,
        "content_block": { "type": "tool_use", "id": "t1", "name": "apply_patch", "input": {} }
    }));
    anthropic.feed_anthropic(&json!({
        "type": "content_block_delta", "index": 0,
        "delta": { "type": "input_json_delta", "partial_json": "{\"input\":\"x" }
    }));
    let progress = anthropic.take_tool_progress(&wants);
    assert_eq!(progress.last().map(|p| p.arguments.as_str()), Some("{\"input\":\"x"));
}

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
