//! FR-012 / FR-013 / FR-014 / FR-015 regressions: stream completeness,
//! Anthropic thinking replay, cache breakpoints, non-streaming usage.

use super::*;

fn anthropic_endpoint(model: &str, effort: Option<&str>) -> Endpoint {
    Endpoint {
        base_url: "https://api.anthropic.com/v1".into(),
        api: "anthropic".into(),
        model: model.into(),
        reasoning_effort: effort.map(str::to_string),
        ..Default::default()
    }
}

fn tool_turn(thinking: Option<Vec<Value>>) -> Vec<LlmMessage> {
    vec![
        LlmMessage { role: "user".into(), content: "read it".into(), ..Default::default() },
        LlmMessage {
            role: "assistant".into(),
            tool_calls: Some(vec![json!({ "id": "t1", "function": { "name": "read_file", "arguments": "{}" } })]),
            thinking_blocks: thinking,
            ..Default::default()
        },
        LlmMessage { role: "tool".into(), content: "data".into(), tool_call_id: Some("t1".into()), ..Default::default() },
    ]
}

/// FR-013: thinking blocks are captured with their signature and replayed
/// first in the assistant message of the tool loop.
#[test]
fn anthropic_thinking_blocks_are_captured_and_replayed_with_signatures() {
    let mut acc = StreamAcc::default();
    acc.feed_anthropic(&json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "" } }));
    acc.feed_anthropic(&json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "plan" } }));
    acc.feed_anthropic(&json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "signature_delta", "signature": "sig-1" } }));
    acc.feed_anthropic(&json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "redacted_thinking", "data": "opaque" } }));
    acc.feed_anthropic(&json!({ "type": "content_block_start", "index": 2, "content_block": { "type": "tool_use", "id": "t1", "name": "read_file" } }));
    let blocks = acc.thinking_blocks();
    assert_eq!(
        blocks,
        vec![
            json!({ "type": "thinking", "thinking": "plan", "signature": "sig-1" }),
            json!({ "type": "redacted_thinking", "data": "opaque" }),
        ]
    );

    let body = anthropic_body(&anthropic_endpoint("claude-opus-4-1", Some("high")), "", &tool_turn(Some(blocks)), &[]);
    let content = &body["messages"][1]["content"];
    assert_eq!(content[0]["signature"], "sig-1");
    assert_eq!(content[1]["type"], "redacted_thinking");
    assert_eq!(content[2]["type"], "tool_use");
    assert_eq!(body["thinking"]["type"], "enabled");
}

/// FR-013: Claude 4.6+ gets adaptive thinking with an effort (budget_tokens
/// is rejected there); the budget form is skipped for a tool turn that has
/// no thinking block to replay.
#[test]
fn anthropic_thinking_config_follows_the_model() {
    for model in ["claude-opus-4-6", "claude-sonnet-5-5", "us.anthropic.claude-opus-5-5-v1:0", "anthropic/claude-fable-5-2"] {
        assert!(uses_adaptive_thinking(model), "{model}");
    }
    for model in ["claude-sonnet-4-20250514", "claude-opus-4-1", "claude-haiku-4-5", "claude-3-7-sonnet-latest", "MiniMax-M2"] {
        assert!(!uses_adaptive_thinking(model), "{model}");
    }
    let adaptive = anthropic_body(&anthropic_endpoint("claude-opus-5-5", Some("minimal")), "", &tool_turn(None), &[]);
    assert_eq!(adaptive["thinking"], json!({ "type": "adaptive" }));
    assert_eq!(adaptive["output_config"]["effort"], "low");
    assert_eq!(adaptive["max_tokens"], 32_000);

    let legacy = anthropic_body(&anthropic_endpoint("claude-opus-4-1", Some("high")), "", &tool_turn(None), &[]);
    assert!(legacy.get("thinking").is_none(), "no thinking block to replay: thinking stays off");
    let off = anthropic_body(&anthropic_endpoint("claude-opus-5-5", Some("off")), "", &tool_turn(None), &[]);
    assert!(off.get("thinking").is_none());
}

/// FR-014: system prompt, tools and the last message carry cache breakpoints.
#[test]
fn anthropic_request_marks_cache_breakpoints() {
    let tools = vec![
        json!({ "type": "function", "function": { "name": "a", "parameters": { "type": "object" } } }),
        json!({ "type": "function", "function": { "name": "b", "parameters": { "type": "object" } } }),
    ];
    let body = anthropic_body(&anthropic_endpoint("claude-opus-5-5", None), "sys", &tool_turn(None), &tools);
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    assert!(body["tools"][0].get("cache_control").is_none());
    assert_eq!(body["tools"][1]["cache_control"]["type"], "ephemeral");
    let last = body["messages"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["content"][0]["type"], "tool_result");
    assert_eq!(last["content"][0]["cache_control"]["type"], "ephemeral");
    let user = vec![LlmMessage { role: "user".into(), content: "hi".into(), ..Default::default() }];
    let plain = anthropic_body(&anthropic_endpoint("claude-opus-5-5", None), "", &user, &[]);
    assert_eq!(plain["messages"][0]["content"][0]["text"], "hi");
    assert_eq!(plain["messages"][0]["content"][0]["cache_control"]["type"], "ephemeral");
}

/// FR-012: stop reasons are kept so a cut-off answer is recognized.
#[test]
fn stop_reasons_mark_truncated_output() {
    let mut openai = StreamAcc::default();
    openai.feed_openai(&json!({ "choices": [{ "delta": {}, "finish_reason": "length" }] }));
    assert!(openai.finished && openai.truncated());
    let mut anthropic = StreamAcc::default();
    anthropic.feed_anthropic(&json!({ "type": "message_delta", "delta": { "stop_reason": "max_tokens" }, "usage": { "output_tokens": 3 } }));
    assert!(anthropic.truncated() && !anthropic.finished);
    let mut responses = StreamAcc::default();
    responses.feed_responses(&json!({ "type": "response.incomplete", "response": { "incomplete_details": { "reason": "max_output_tokens" } } }));
    assert!(responses.finished && responses.truncated());
}

/// FR-070: a 60 s Retry-After is honoured (capped at 90 s).
#[test]
fn retry_after_is_honoured_up_to_the_cap() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("retry-after", "60".parse().unwrap());
    assert_eq!(retry_after(&headers), Some(Duration::from_secs(60)));
    headers.insert("retry-after", "3600".parse().unwrap());
    assert_eq!(retry_after(&headers), Some(MAX_RETRY_AFTER));
    headers.insert("retry-after-ms", "1500".parse().unwrap());
    assert_eq!(retry_after(&headers), Some(Duration::from_millis(1500)));
    assert_eq!(retry_after(&reqwest::header::HeaderMap::new()), None);
}

/// Serves one canned HTTP response and returns its base URL.
async fn serve_once(content_type: &'static str, body: String) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}/v1", listener.local_addr().unwrap().port());
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = vec![0u8; 65536];
        let _ = stream.read(&mut buffer).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
    });
    base
}

async fn call(endpoint: &Endpoint) -> Result<StreamOutcome> {
    let messages = vec![LlmMessage { role: "user".into(), content: "hi".into(), ..Default::default() }];
    stream_chat(&reqwest::Client::new(), endpoint, "", &messages, &[], &mut |_, _| {}, &|| false).await
}

/// FR-012: a stream that closes without a terminal event, or that reports
/// an Anthropic `error` event, is an error, not a finished answer.
#[tokio::test]
async fn incomplete_or_failed_streams_are_errors() {
    let cut = "data: {\"choices\":[{\"delta\":{\"content\":\"Hal\"}}]}\n\n".to_string();
    let base = serve_once("text/event-stream", cut).await;
    let endpoint = Endpoint { base_url: base, api: "openai-compatible".into(), model: "m".into(), ..Default::default() };
    let err = call(&endpoint).await.unwrap_err();
    assert!(err.to_string().contains("ended before the response was complete"), "{err}");

    let overloaded =
        "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n".to_string();
    let base = serve_once("text/event-stream", overloaded).await;
    let endpoint = Endpoint { base_url: base, api: "anthropic".into(), model: "m".into(), ..Default::default() };
    let err = call(&endpoint).await.unwrap_err();
    assert!(err.to_string().contains("Overloaded"), "{err}");
}

/// FR-015: a non-streaming OpenAI body keeps its output token count.
#[tokio::test]
async fn non_streaming_openai_body_keeps_output_tokens() {
    let body = json!({
        "choices": [{ "message": { "role": "assistant", "content": "OK" }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 4 }
    });
    let base = serve_once("application/json", body.to_string()).await;
    let endpoint = Endpoint { base_url: base, api: "openai-compatible".into(), model: "m".into(), ..Default::default() };
    let outcome = call(&endpoint).await.unwrap();
    let usage = outcome.usage.unwrap();
    assert_eq!((usage.input_tokens, usage.output_tokens), (10, 4));
    assert_eq!(outcome.content, "OK");
}

/// OpenCode's Messages endpoint sends zero usage in `message_start` and the
/// real prompt counts in `message_delta`; Anthropic sends them at the start.
#[test]
fn anthropic_prompt_usage_is_taken_from_either_event() {
    let mut delta_only = StreamAcc::default();
    delta_only.feed_anthropic(&json!({ "type": "message_start", "message": { "usage": { "input_tokens": 0, "output_tokens": 0 } } }));
    delta_only.feed_anthropic(&json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" },
        "usage": { "input_tokens": 1, "output_tokens": 22, "cache_read_input_tokens": 203 } }));
    assert_eq!((delta_only.usage.input_tokens, delta_only.usage.cache_read_tokens, delta_only.usage.output_tokens), (204, 203, 22));

    let mut start_only = StreamAcc::default();
    start_only.feed_anthropic(&json!({ "type": "message_start", "message": { "usage": {
        "input_tokens": 10, "cache_read_input_tokens": 90, "cache_creation_input_tokens": 5 } } }));
    start_only.feed_anthropic(&json!({ "type": "message_delta", "usage": { "output_tokens": 7 } }));
    assert_eq!((start_only.usage.input_tokens, start_only.usage.cache_read_tokens, start_only.usage.cache_write_tokens), (105, 90, 5));
    assert_eq!(start_only.usage.output_tokens, 7);
}
