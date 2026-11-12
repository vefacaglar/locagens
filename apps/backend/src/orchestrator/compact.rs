//! Codex-compatible context compaction.
//!
//! Ported from `codex-rs/core/src/compact.rs`:
//!
//! - When the estimated input token count exceeds [`AUTO_COMPACT_TOKEN_THRESHOLD`]
//!   (or compaction is explicitly triggered via `/compact` or the
//!   `compact_context` tool), the older portion of the conversation is
//!   summarized with [`SUMMARIZATION_PROMPT`] and replaced in the in-memory
//!   history by a `[Compacted context summary]` block, while the last
//!   [`DEFAULT_KEEP_LAST_MESSAGES`] messages stay intact.
//!
//! The pipeline is split into pure, testable steps plus one async driver:
//!
//! 1. [`should_auto_compact`] — token estimate gate.
//! 2. [`build_summarization_messages`] — builds the summarizer request
//!    (Codex bounds the serialized transcript at
//!    [`COMPACT_USER_MESSAGE_MAX_TOKENS`]).
//! 3. [`apply_compaction`] — folds the summary back into the history.

use super::llm::LlmMessage;
use serde_json::Value;

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

/// Auto-compaction trigger: estimated input tokens above this compact.
pub const AUTO_COMPACT_TOKEN_THRESHOLD: usize = 90_000;

/// Maximum tokens of serialized transcript sent to the summarizer.
pub const COMPACT_USER_MESSAGE_MAX_TOKENS: usize = 20_000;

/// Messages kept intact at the tail of the history during compaction.
pub const DEFAULT_KEEP_LAST_MESSAGES: usize = 8;

/// Rough token estimate (~4 chars per token) — the same heuristic Codex uses
/// when exact tokenizer counts are unavailable.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// Estimated token count of a full conversation history.
pub fn estimate_history_tokens(history: &[LlmMessage]) -> usize {
    history
        .iter()
        .map(|message| {
            estimate_tokens(&message.content)
                + message
                    .tool_calls
                    .as_ref()
                    .map(|calls| calls.iter().map(|call| estimate_tokens(&call.to_string())).sum::<usize>())
                    .unwrap_or(0)
        })
        .sum()
}

/// Whether the history should be compacted right now.
pub fn should_auto_compact(history: &[LlmMessage]) -> bool {
    estimate_history_tokens(history) > AUTO_COMPACT_TOKEN_THRESHOLD
}

/// Splits `history` into `(older, keep)` where `keep` is the intact tail.
pub fn split_history(history: &[LlmMessage], keep_last: usize) -> (&[LlmMessage], &[LlmMessage]) {
    let keep = keep_last.min(history.len());
    let split_at = history.len() - keep;
    (&history[..split_at], &history[split_at..])
}

/// Builds the summarizer request as `(system_prompt, transcript)`. Returns
/// `None` when there is nothing to summarize.
pub fn summarization_request(history: &[LlmMessage], keep_last: usize) -> Option<(String, String)> {
    let (older, _) = split_history(history, keep_last);
    if older.is_empty() {
        return None;
    }
    let mut transcript = String::new();
    let mut budget = COMPACT_USER_MESSAGE_MAX_TOKENS.saturating_mul(4); // chars
    for message in older {
        let mut rendered = format!("## {}\n{}\n", message.role, message.content);
        if let Some(calls) = &message.tool_calls {
            rendered.push_str(&format!(
                "tool_calls: {}\n",
                serde_json::to_string(calls).unwrap_or_default()
            ));
        }
        if let Some(id) = &message.tool_call_id {
            rendered.push_str(&format!("tool_call_id: {id}\n"));
        }
        if rendered.len() > budget {
            rendered.truncate(budget);
            rendered.push_str("\n... (transcript truncated)\n");
            transcript.push_str(&rendered);
            break;
        }
        budget = budget.saturating_sub(rendered.len());
        transcript.push_str(&rendered);
        if budget == 0 {
            transcript.push_str("\n... (transcript truncated)\n");
            break;
        }
    }
    Some((SUMMARIZATION_PROMPT.to_string(), transcript))
}

/// Builds the full message list for the summarizer call (system + user).
pub fn build_summarization_messages(history: &[LlmMessage], keep_last: usize) -> Option<Vec<LlmMessage>> {
    let (system, transcript) = summarization_request(history, keep_last)?;
    Some(vec![
        LlmMessage {
            role: "system".into(),
            content: system,
            ..Default::default()
        },
        LlmMessage {
            role: "user".into(),
            content: transcript,
            ..Default::default()
        },
    ])
}

/// Folds a finished summary back into the history: the older portion is
/// replaced with one user message carrying the `[Compacted context summary]`
/// block, and the tail is preserved verbatim.
pub fn apply_compaction(history: &[LlmMessage], summary: &str, keep_last: usize) -> Vec<LlmMessage> {
    let (_, keep) = split_history(history, keep_last);
    let mut compacted = Vec::with_capacity(keep.len() + 1);
    compacted.push(LlmMessage {
        role: "user".into(),
        content: format!("{SUMMARY_PREFIX}\n\n{summary}"),
        ..Default::default()
    });
    compacted.extend(keep.iter().cloned());
    compacted
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
                serde_json::json!({
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
                content: "x".repeat(*chars),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn token_estimates_and_auto_compact_gate() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        // 8 messages of 50k chars ≈ 100k tokens → compact.
        assert!(should_auto_compact(&history(&[50_000; 8])));
        assert!(!should_auto_compact(&history(&[1_000; 8])));
    }

    #[test]
    fn summarization_messages_bounded_and_tail_preserved() {
        let hist = history(&[3_000; 100]); // 300k chars of older content
        let messages = build_summarization_messages(&hist, 8).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[0].content, SUMMARIZATION_PROMPT);
        // Bounded at ~20k tokens of transcript (80k chars) + truncation marker.
        assert!(messages[1].content.len() <= 80_000 + 64);
        assert!(messages[1].content.contains("## user"));
        assert!(messages[1].content.contains("(transcript truncated)"));

        // Nothing to summarize when history fits entirely in the tail.
        let short = history(&[10; 4]);
        assert!(build_summarization_messages(&short, 8).is_none());
    }

    #[test]
    fn compaction_replaces_older_history_with_summary_block() {
        let hist = history(&[100; 20]);
        let compacted = apply_compaction(&hist, "Summary of earlier work.", 8);
        assert_eq!(compacted.len(), 9);
        assert!(compacted[0].content.starts_with(SUMMARY_PREFIX));
        assert!(compacted[0].content.contains("Summary of earlier work."));
        assert_eq!(compacted[0].role, "user");
        // The intact tail survives verbatim.
        assert_eq!(compacted[1].content, "x".repeat(100));
        assert_eq!(compacted[8].content, "x".repeat(100));
        assert_eq!(count_compactions(&compacted), 1);
        // A second compaction replaces the previous summary block with the
        // newest one (older history is always dropped wholesale).
        let twice = apply_compaction(&compacted, "Second summary.", 8);
        assert_eq!(twice.len(), 9);
        assert_eq!(count_compactions(&twice), 1);
        assert!(twice[0].content.contains("Second summary."));
        assert!(!twice[0].content.contains("Summary of earlier work."));
    }
}
