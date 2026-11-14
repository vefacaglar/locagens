//! Keeps a model history provider-valid.
//!
//! OpenAI and Anthropic reject a request when an assistant message is empty,
//! when a tool call has no result, or when a tool result has no call. A
//! failed or cancelled round, an interrupted tool loop or a compaction cut can
//! each leave such a shape behind; [`sanitize`] repairs all of them in one
//! pass so every request (and every reseeded `continue_run`) is valid.

use super::llm::LlmMessage;
use serde_json::Value;

/// Result given to a tool call that never produced one (cancelled, failed
/// or interrupted before it ran).
pub const INTERRUPTED_TOOL_RESULT: &str = "Tool call was interrupted before it returned a result.";

/// Returns `history` with:
/// - assistant messages that carry no text, no tool calls and nothing else
///   dropped (a round that failed before any delta arrived);
/// - a synthetic [`INTERRUPTED_TOOL_RESULT`] for every tool call without a
///   result, placed right after that call's results;
/// - tool results whose call is not in the preceding assistant message
///   (orphans, duplicates) dropped.
pub fn sanitize(history: Vec<LlmMessage>) -> Vec<LlmMessage> {
    let mut out: Vec<LlmMessage> = Vec::with_capacity(history.len());
    // Call ids of the open tool group still waiting for a result.
    let mut pending: Vec<String> = Vec::new();
    for mut message in history {
        if message.role == "tool" {
            let id = message.tool_call_id.clone().unwrap_or_default();
            if let Some(position) = pending.iter().position(|pending_id| *pending_id == id) {
                pending.remove(position);
                out.push(message);
            }
            continue;
        }
        close_tool_group(&mut out, &mut pending);
        if message.role == "assistant" {
            if message.tool_calls.as_ref().is_some_and(Vec::is_empty) {
                message.tool_calls = None;
            }
            if is_empty_assistant(&message) {
                continue;
            }
            pending = call_ids(&message);
        }
        out.push(message);
    }
    close_tool_group(&mut out, &mut pending);
    out
}

/// In-place [`sanitize`].
pub fn sanitize_in_place(history: &mut Vec<LlmMessage>) {
    let taken = std::mem::take(history);
    *history = sanitize(taken);
}

/// Answers every still-pending call of the open tool group.
fn close_tool_group(out: &mut Vec<LlmMessage>, pending: &mut Vec<String>) {
    for id in pending.drain(..) {
        out.push(LlmMessage {
            role: "tool".into(),
            content: INTERRUPTED_TOOL_RESULT.into(),
            tool_call_id: Some(id),
            ..Default::default()
        });
    }
}

fn is_empty_assistant(message: &LlmMessage) -> bool {
    message.content.trim().is_empty() && message.tool_calls.is_none()
}

fn call_ids(message: &LlmMessage) -> Vec<String> {
    message
        .tool_calls
        .iter()
        .flatten()
        .map(|call| call.get("id").and_then(Value::as_str).unwrap_or_default().to_string())
        .collect()
}

/// Index where a tail kept verbatim may start: `wanted` moved back past
/// `tool` results, so the tail never begins with a result whose call was
/// cut away (the call's assistant message moves into the tail instead).
pub fn group_safe_split(history: &[LlmMessage], wanted: usize) -> usize {
    let mut split = wanted.min(history.len());
    while split > 0 && split < history.len() && history[split].role == "tool" {
        split -= 1;
    }
    split
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(role: &str, content: &str) -> LlmMessage {
        LlmMessage { role: role.into(), content: content.into(), ..Default::default() }
    }

    fn calls(ids: &[&str]) -> LlmMessage {
        let calls = ids
            .iter()
            .map(|id| json!({ "id": id, "function": { "name": "read_file", "arguments": "{}" } }))
            .collect();
        LlmMessage { role: "assistant".into(), tool_calls: Some(calls), ..Default::default() }
    }

    fn result(id: &str) -> LlmMessage {
        LlmMessage { role: "tool".into(), content: format!("result {id}"), tool_call_id: Some(id.into()), ..Default::default() }
    }

    fn shape(history: &[LlmMessage]) -> Vec<String> {
        history
            .iter()
            .map(|m| match m.role.as_str() {
                "tool" => format!("tool:{}", m.tool_call_id.as_deref().unwrap_or_default()),
                role => format!("{role}:{}", m.content),
            })
            .collect()
    }

    /// FR-001: the placeholder of a failed round is dropped.
    #[test]
    fn empty_assistant_messages_are_dropped() {
        let history = vec![msg("user", "first"), msg("assistant", ""), msg("assistant", "  "), msg("user", "second")];
        assert_eq!(shape(&sanitize(history)), ["user:first", "user:second"]);
    }

    /// FR-002: unanswered calls get a result right after the answered ones.
    #[test]
    fn unanswered_calls_get_an_interrupted_result() {
        let history = vec![msg("user", "go"), calls(&["a", "b", "c"]), result("a"), msg("user", "next")];
        let fixed = sanitize(history);
        assert_eq!(shape(&fixed), ["user:go", "assistant:", "tool:a", "tool:b", "tool:c", "user:next"]);
        assert_eq!(fixed[3].content, INTERRUPTED_TOOL_RESULT);
        // A trailing open group is closed too.
        let trailing = sanitize(vec![msg("user", "go"), calls(&["x"])]);
        assert_eq!(shape(&trailing), ["user:go", "assistant:", "tool:x"]);
    }

    /// FR-003: results without their call are dropped.
    #[test]
    fn orphan_and_duplicate_results_are_dropped() {
        let history = vec![result("gone"), msg("user", "hi"), calls(&["a"]), result("a"), result("a"), result("zz")];
        assert_eq!(shape(&sanitize(history)), ["user:hi", "assistant:", "tool:a"]);
    }

    #[test]
    fn valid_history_is_unchanged() {
        let history = vec![msg("user", "go"), calls(&["a", "b"]), result("b"), result("a"), msg("assistant", "done")];
        assert_eq!(shape(&sanitize(history.clone())), shape(&history));
    }

    #[test]
    fn split_never_starts_the_tail_with_a_tool_result() {
        let history = vec![msg("user", "u"), calls(&["a", "b"]), result("a"), result("b"), msg("assistant", "x")];
        assert_eq!(group_safe_split(&history, 3), 1);
        assert_eq!(group_safe_split(&history, 2), 1);
        assert_eq!(group_safe_split(&history, 4), 4);
        assert_eq!(group_safe_split(&history, 9), 5);
    }
}
