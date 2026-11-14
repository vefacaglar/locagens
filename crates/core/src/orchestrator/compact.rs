//! Codex-compatible context compaction.
//!
//! Ported from `codex-rs/core/src/compact.rs`:
//!
//! - When the estimated input token count exceeds the threshold (from
//!   `config.toml`, else the model's context window, else
//!   [`AUTO_COMPACT_TOKEN_THRESHOLD`]), or compaction is explicitly triggered
//!   via `/compact` or the `compact_context` tool, the older portion of the
//!   conversation is summarized with [`SUMMARIZATION_PROMPT`] and replaced in
//!   the history by a `[Compacted context summary]` block, while the last
//!   [`DEFAULT_KEEP_LAST_MESSAGES`] messages (never splitting a tool call from
//!   its results) stay intact.
//! - All of the older history is summarized: it is cut into chunks of at most
//!   [`COMPACT_USER_MESSAGE_MAX_TOKENS`], and each chunk extends the summary of
//!   the chunks before it. Only single oversized messages are shortened, with
//!   an explicit marker.
//! - The main agent persists a compaction marker (a `system` message) so
//!   `seed_history` replays the compacted history on the next turn.
//!
//! The pipeline is split into pure, testable steps plus one async driver
//! (`AgentLoop::run_compaction`).

use super::llm::LlmMessage;
use crate::db::RunMessage;
use serde_json::{json, Value};

/// Codex's summarization prompt (verbatim intent from `codex-rs` prompts).
pub const SUMMARIZATION_PROMPT: &str = r#"You are acting as an automatic context compaction assistant for a coding agent. The conversation so far is about to exceed the model's context window, so it must be replaced with a compact summary.

Produce a structured summary of the conversation that a successor agent could continue from seamlessly. Include, in this order:
1. The user's primary goals and explicit constraints.
2. A short chronological list of actions already taken (tool calls made, files created or modified, commands run) with their outcomes.
3. Current state: relevant file paths, branch names, errors hit and their fixes, pending plans or in-progress steps.
4. Any important user preferences, decisions or corrections from earlier in the conversation.

Be factual and dense. Do not add commentary, and do not attempt to continue the task — only summarize it. Write in plain markdown, at most a few hundred words."#;

/// Prefix prepended to the summary when it replaces older history.
pub const SUMMARY_PREFIX: &str = "[Compacted context summary]";

/// Auto-compaction trigger when neither `config.toml` nor the provider
/// catalog knows the model's context window.
pub const AUTO_COMPACT_TOKEN_THRESHOLD: usize = 90_000;

/// Share of the context window the history may fill before it compacts.
const WINDOW_FILL_RATIO: f64 = 0.9;

/// Maximum tokens of serialized transcript per summarizer call.
pub const COMPACT_USER_MESSAGE_MAX_TOKENS: usize = 20_000;

/// Longest rendering of one message in the transcript; longer messages
/// (huge tool outputs) keep their head and tail around an omission marker.
const MESSAGE_RENDER_MAX_CHARS: usize = 16_000;

/// Messages kept intact at the tail of the history during compaction.
pub const DEFAULT_KEEP_LAST_MESSAGES: usize = 8;

/// `rawResponse` key of a persisted compaction marker.
const MARKER_KEY: &str = "compaction";

/// Rough token estimate (~4 chars per token) — the same heuristic Codex uses
/// when exact tokenizer counts are unavailable.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// Flat token cost counted for one attached image (providers bill images by
/// pixels, not by the length of their base64 text).
const IMAGE_TOKEN_ESTIMATE: usize = 1_600;

/// Estimated tokens of a message's text; a `view_image` result counts its
/// JSON without the base64 bytes plus [`IMAGE_TOKEN_ESTIMATE`] (FR-027).
fn estimate_content_tokens(message: &LlmMessage) -> usize {
    match (message.role == "tool").then(|| super::llm::tool_image_text(&message.content)).flatten() {
        Some(text) => estimate_tokens(&text) + IMAGE_TOKEN_ESTIMATE,
        None => estimate_tokens(&message.content),
    }
}

/// Estimated token count of a full conversation history.
pub fn estimate_history_tokens(history: &[LlmMessage]) -> usize {
    history
        .iter()
        .map(|message| {
            estimate_content_tokens(message)
                + message
                    .tool_calls
                    .as_ref()
                    .map(|calls| calls.iter().map(|call| estimate_tokens(&call.to_string())).sum::<usize>())
                    .unwrap_or(0)
        })
        .sum()
}

/// The auto-compaction threshold: `config.toml`
/// (`model_auto_compact_token_limit` / `model_context_window`) wins, then 90 %
/// of the provider catalog's context window minus the fixed request overhead
/// (system prompt and tool specs), then [`AUTO_COMPACT_TOKEN_THRESHOLD`].
pub fn auto_compact_threshold(context_window: Option<i64>, overhead_tokens: usize) -> usize {
    if let Some(limit) = crate::config_runtime::current().auto_compact_limit() {
        return limit;
    }
    threshold_for_window(context_window, overhead_tokens)
}

/// The catalog part of [`auto_compact_threshold`] (pure, for tests).
pub fn threshold_for_window(context_window: Option<i64>, overhead_tokens: usize) -> usize {
    match context_window.filter(|window| *window > 0) {
        Some(window) => {
            let window = window as usize;
            let fill = (window as f64 * WINDOW_FILL_RATIO) as usize;
            // Never so low that every round compacts.
            fill.saturating_sub(overhead_tokens).max(window / 4)
        }
        None => AUTO_COMPACT_TOKEN_THRESHOLD,
    }
}

/// Whether `history` exceeds the threshold.
pub fn should_auto_compact_at(history: &[LlmMessage], threshold: usize) -> bool {
    estimate_history_tokens(history) > threshold
}

/// Splits `history` into `(older, keep)` where `keep` is the intact tail of
/// about `keep_last` messages. The tail never starts with a tool result: the
/// call that produced it moves into the tail with it.
pub fn split_history(history: &[LlmMessage], keep_last: usize) -> (&[LlmMessage], &[LlmMessage]) {
    let wanted = history.len() - keep_last.min(history.len());
    history.split_at(super::history::group_safe_split(history, wanted))
}

/// The older history rendered as transcript chunks of at most
/// [`COMPACT_USER_MESSAGE_MAX_TOKENS`] each, oldest first. `None` when there
/// is nothing to summarize.
pub fn summarization_chunks(history: &[LlmMessage], keep_last: usize) -> Option<Vec<String>> {
    let (older, _) = split_history(history, keep_last);
    if older.is_empty() {
        return None;
    }
    let budget = COMPACT_USER_MESSAGE_MAX_TOKENS.saturating_mul(4); // chars
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    for message in older {
        let rendered = render_message(message);
        if !current.is_empty() && current.len() + rendered.len() > budget {
            chunks.push(std::mem::take(&mut current));
        }
        current.push_str(&rendered);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    Some(chunks)
}

/// One message as transcript text; an oversized one is cut in the middle
/// with a visible marker.
fn render_message(message: &LlmMessage) -> String {
    let mut body = super::llm::tool_image_text(&message.content).unwrap_or_else(|| message.content.clone());
    if let Some(calls) = &message.tool_calls {
        body.push_str(&format!("\ntool_calls: {}", serde_json::to_string(calls).unwrap_or_default()));
    }
    if let Some(id) = &message.tool_call_id {
        body.push_str(&format!("\ntool_call_id: {id}"));
    }
    format!("## {}\n{}\n", message.role, shorten_middle(&body, MESSAGE_RENDER_MAX_CHARS))
}

/// `text` cut to about `max_bytes`: head and tail kept around a marker that
/// says how much was left out.
fn shorten_middle(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let head = locagens_protocol::text::prefix_at_char_boundary(text, max_bytes / 2);
    let tail = locagens_protocol::text::suffix_at_char_boundary(text, max_bytes / 2);
    let omitted = text.len() - head.len() - tail.len();
    format!("{head}\n[... {omitted} bytes omitted from this message ...]\n{tail}")
}

/// The user message of one summarizer call: the transcript chunk, plus the
/// summary of the chunks before it.
pub fn chunk_prompt(previous_summary: Option<&str>, chunk: &str, index: usize, total: usize) -> String {
    match previous_summary {
        None if total <= 1 => chunk.to_string(),
        None => format!("Transcript part 1 of {total}:\n\n{chunk}"),
        Some(summary) => format!(
            "Summary of the earlier parts of the conversation:\n\n{summary}\n\n\
             Extend that summary with transcript part {} of {total} below, and return one \
             combined summary of everything.\n\n{chunk}",
            index + 1
        ),
    }
}

/// Folds a finished summary back into the history: the older portion is
/// replaced with one user message carrying the `[Compacted context summary]`
/// block, and the tail is preserved verbatim.
pub fn apply_compaction(history: &[LlmMessage], summary: &str, keep_last: usize) -> Vec<LlmMessage> {
    let (_, keep) = split_history(history, keep_last);
    let mut compacted = Vec::with_capacity(keep.len() + 1);
    compacted.push(LlmMessage {
        role: "user".into(),
        content: summary_block(summary),
        ..Default::default()
    });
    compacted.extend(keep.iter().cloned());
    compacted
}

/// The text of the summary block (also the content of a compaction marker).
pub fn summary_block(summary: &str) -> String {
    format!("{SUMMARY_PREFIX}\n\n{summary}")
}

/// Persisted id of the first message a compaction keeps (the tail start),
/// skipping tail messages that were never persisted.
pub fn first_kept_id(history: &[LlmMessage], keep_last: usize) -> Option<String> {
    let (_, keep) = split_history(history, keep_last);
    keep.iter().find_map(|message| message.source_id.clone())
}

/// `rawResponse` of a compaction marker that kept `first_kept` onwards.
pub fn marker_raw(first_kept: Option<&str>) -> String {
    json!({ MARKER_KEY: { "firstKeptMessageId": first_kept } }).to_string()
}

/// The latest compaction marker among persisted messages:
/// `(index, first kept message id)`.
pub fn latest_marker(messages: &[RunMessage]) -> Option<(usize, Option<String>)> {
    messages.iter().enumerate().rev().find_map(|(index, message)| {
        if message.role != "system" || message.agent_role.is_some() {
            return None;
        }
        let raw: Value = serde_json::from_str(message.raw_response.as_deref()?).ok()?;
        let marker = raw.get(MARKER_KEY)?;
        let first_kept = marker.get("firstKeptMessageId").and_then(Value::as_str).map(str::to_string);
        Some((index, first_kept))
    })
}

/// Number of compactions reflected in a history (count of summary blocks).
pub fn count_compactions(history: &[LlmMessage]) -> usize {
    history
        .iter()
        .filter(|message| message.content.starts_with(SUMMARY_PREFIX))
        .count()
}

/// Serializes a history to a compact JSON value (used by the inspector).
pub fn history_to_json(history: &[LlmMessage]) -> Value {
    Value::Array(
        history
            .iter()
            .map(|message| {
                json!({
                    "role": message.role,
                    "content": message.content,
                    "tool_call_id": message.tool_call_id,
                    "tool_calls": message.tool_calls,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(lengths: &[usize]) -> Vec<LlmMessage> {
        lengths
            .iter()
            .enumerate()
            .map(|(index, chars)| LlmMessage {
                role: if index % 2 == 0 { "user" } else { "assistant" }.into(),
                content: format!("m{index}:{}", "x".repeat(*chars)),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn token_estimates_and_auto_compact_gate() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert!(should_auto_compact_at(&history(&[50_000; 8]), AUTO_COMPACT_TOKEN_THRESHOLD));
        assert!(!should_auto_compact_at(&history(&[1_000; 8]), AUTO_COMPACT_TOKEN_THRESHOLD));
        assert!(should_auto_compact_at(&history(&[1_000; 8]), 1_000));
    }

    /// FR-005: the threshold follows the model's context window.
    #[test]
    fn threshold_follows_the_context_window() {
        assert_eq!(threshold_for_window(None, 5_000), AUTO_COMPACT_TOKEN_THRESHOLD);
        assert_eq!(threshold_for_window(Some(0), 5_000), AUTO_COMPACT_TOKEN_THRESHOLD);
        assert_eq!(threshold_for_window(Some(32_000), 4_000), 28_800 - 4_000);
        assert_eq!(threshold_for_window(Some(1_000_000), 10_000), 890_000);
        // A system prompt bigger than the window still leaves a floor.
        assert_eq!(threshold_for_window(Some(8_000), 50_000), 2_000);
    }

    /// FR-004: every older message reaches the summarizer, in order.
    #[test]
    fn all_older_history_is_chunked_for_summarization() {
        let hist = history(&[3_000; 200]); // ~600k chars before the tail
        let chunks = summarization_chunks(&hist, 8).unwrap();
        assert!(chunks.len() > 5, "{}", chunks.len());
        let budget = COMPACT_USER_MESSAGE_MAX_TOKENS * 4;
        assert!(chunks.iter().all(|chunk| chunk.len() <= budget));
        let joined = chunks.concat();
        for index in 0..192 {
            assert!(joined.contains(&format!("m{index}:")), "message {index} missing");
        }
        assert!(!joined.contains("m192:"), "the tail is not summarized");
        assert!(summarization_chunks(&history(&[10; 4]), 8).is_none());
    }

    /// QA-021: oversized messages are shortened at char boundaries with a marker.
    #[test]
    fn oversized_messages_are_cut_in_the_middle_with_a_marker() {
        let content = format!("{}{}", "→".repeat(30_000), "end");
        let mut hist = vec![LlmMessage { role: "tool".into(), content, ..Default::default() }];
        hist.extend(history(&[10; 8]));
        let chunks = summarization_chunks(&hist, 8).unwrap();
        assert!(chunks[0].contains("bytes omitted from this message"));
        assert!(chunks[0].contains("end"));
        assert!(chunks[0].len() < MESSAGE_RENDER_MAX_CHARS + 200);
    }

    #[test]
    fn chunk_prompts_carry_the_running_summary() {
        assert_eq!(chunk_prompt(None, "T", 0, 1), "T");
        assert!(chunk_prompt(None, "T", 0, 3).starts_with("Transcript part 1 of 3"));
        let next = chunk_prompt(Some("S1"), "T2", 1, 3);
        assert!(next.contains("S1") && next.contains("T2") && next.contains("part 2 of 3"));
    }

    #[test]
    fn compaction_replaces_older_history_with_summary_block() {
        let hist = history(&[100; 20]);
        let compacted = apply_compaction(&hist, "Summary of earlier work.", 8);
        assert_eq!(compacted.len(), 9);
        assert!(compacted[0].content.starts_with(SUMMARY_PREFIX));
        assert!(compacted[0].content.contains("Summary of earlier work."));
        assert_eq!(compacted[0].role, "user");
        assert_eq!(compacted[1].content, hist[12].content);
        assert_eq!(compacted[8].content, hist[19].content);
        assert_eq!(count_compactions(&compacted), 1);
        let twice = apply_compaction(&compacted, "Second summary.", 8);
        assert_eq!(twice.len(), 9);
        assert_eq!(count_compactions(&twice), 1);
        assert!(twice[0].content.contains("Second summary."));
    }

    /// FR-003: the kept tail never starts with an orphan tool result.
    #[test]
    fn compaction_keeps_a_tool_call_with_its_results() {
        let mut hist = history(&[10; 10]);
        hist.push(LlmMessage {
            role: "assistant".into(),
            tool_calls: Some(vec![json!({ "id": "a" }), json!({ "id": "b" })]),
            ..Default::default()
        });
        for id in ["a", "b"] {
            hist.push(LlmMessage { role: "tool".into(), content: "r".into(), tool_call_id: Some(id.into()), ..Default::default() });
        }
        hist.extend(history(&[10; 6]));
        // len 19, keep 8 -> naive split at 11 (the first tool result).
        let compacted = apply_compaction(&hist, "S", 8);
        assert_eq!(compacted[1].role, "assistant");
        assert!(compacted[1].tool_calls.is_some());
        assert_eq!(compacted.len(), 1 + 9);
    }

    #[test]
    fn markers_round_trip_through_persisted_messages() {
        let mut hist = history(&[10; 12]);
        for (index, message) in hist.iter_mut().enumerate() {
            message.source_id = Some(format!("id{index}"));
        }
        assert_eq!(first_kept_id(&hist, 8).as_deref(), Some("id4"));
        let marker = RunMessage {
            id: "mk".into(),
            role: "system".into(),
            content: summary_block("S"),
            raw_response: Some(marker_raw(Some("id4"))),
            ..Default::default()
        };
        let plain = RunMessage { role: "system".into(), raw_response: Some("{}".into()), ..Default::default() };
        let messages = vec![plain.clone(), marker, plain];
        assert_eq!(latest_marker(&messages), Some((1, Some("id4".to_string()))));
        assert_eq!(latest_marker(&messages[2..]), None);
    }
}
