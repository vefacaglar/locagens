//! `get_context_remaining` tool (Codex: `get_context_remaining`).
//!
//! Codex reports `tokens_left` = the smaller of "tokens until the model's
//! context window is full" and "tokens until auto-compaction triggers",
//! both measured from the active context size. Locagens measures the active
//! context with the estimate the auto-compactor itself uses
//! (`compact::estimate_history_tokens`) and compares it against the model's
//! `contextWindow` from the provider catalog and `AUTO_COMPACT_TOKEN_THRESHOLD`.

use super::compact::{estimate_history_tokens, AUTO_COMPACT_TOKEN_THRESHOLD};
use super::llm::LlmMessage;
use serde_json::{json, Value};

pub fn tool_def() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "get_context_remaining",
            "description": "Get the remaining tokens in the current context window (estimated). `tokens_left` is how much room is left before the window fills or the conversation is auto-compacted, whichever comes first.",
            "parameters": { "type": "object", "properties": {}, "required": [] }
        }
    })
}

/// Budget numbers for one conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextBudget {
    pub estimated_tokens: i64,
    pub context_window: Option<i64>,
    pub auto_compact_threshold: i64,
}

impl ContextBudget {
    pub fn measure(history: &[LlmMessage], context_window: Option<i64>) -> Self {
        Self {
            estimated_tokens: estimate_history_tokens(history) as i64,
            context_window: context_window.filter(|window| *window > 0),
            auto_compact_threshold: AUTO_COMPACT_TOKEN_THRESHOLD as i64,
        }
    }

    /// Tokens until the window is full, when the window is known.
    pub fn window_remaining(&self) -> Option<i64> {
        self.context_window.map(|window| (window - self.estimated_tokens).max(0))
    }

    pub fn until_auto_compact(&self) -> i64 {
        (self.auto_compact_threshold - self.estimated_tokens).max(0)
    }

    /// Codex's `tokens_left`: the tighter of window and auto-compact limits.
    pub fn tokens_left(&self) -> i64 {
        match self.window_remaining() {
            Some(window) => window.min(self.until_auto_compact()),
            None => self.until_auto_compact(),
        }
    }

    /// Share of the context window still free (0-100), when known.
    pub fn percent_remaining(&self) -> Option<f64> {
        let window = self.context_window?;
        let free = self.window_remaining()? as f64;
        Some(((free * 1000.0 / window as f64).round()) / 10.0)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "tokens_left": self.tokens_left(),
            "context_window": self.context_window,
            "estimated_tokens_used": self.estimated_tokens,
            "percent_remaining": self.percent_remaining(),
            "tokens_until_auto_compact": self.until_auto_compact(),
            "estimate": "~4 characters per token, same as the auto-compactor",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history_of(chars: usize) -> Vec<LlmMessage> {
        vec![LlmMessage { role: "user".into(), content: "x".repeat(chars), ..Default::default() }]
    }

    #[test]
    fn uses_the_compactors_estimate() {
        let budget = ContextBudget::measure(&history_of(4000), Some(200_000));
        assert_eq!(budget.estimated_tokens, 1000);
        assert_eq!(budget.window_remaining(), Some(199_000));
        assert_eq!(budget.until_auto_compact(), 89_000);
    }

    #[test]
    fn tokens_left_is_the_tighter_limit() {
        // Big window: auto-compaction triggers first.
        let big = ContextBudget::measure(&history_of(4000), Some(1_000_000));
        assert_eq!(big.tokens_left(), 89_000);
        // Small window: the window fills first.
        let small = ContextBudget::measure(&history_of(4000), Some(8_000));
        assert_eq!(small.tokens_left(), 7_000);
    }

    #[test]
    fn unknown_window_falls_back_to_the_compaction_threshold() {
        let budget = ContextBudget::measure(&history_of(400), None);
        assert_eq!(budget.window_remaining(), None);
        assert_eq!(budget.percent_remaining(), None);
        assert_eq!(budget.tokens_left(), 89_900);
        assert!(budget.to_json()["context_window"].is_null());
    }

    #[test]
    fn overfull_history_clamps_to_zero_and_percent_is_rounded() {
        let over = ContextBudget::measure(&history_of(4_000_000), Some(10_000));
        assert_eq!(over.tokens_left(), 0);
        assert_eq!(over.percent_remaining(), Some(0.0));
        let quarter = ContextBudget::measure(&history_of(30_000), Some(10_000 + 7_500));
        // 7500 used of 17500 -> 10000 free = 57.1%
        assert_eq!(quarter.percent_remaining(), Some(57.1));
    }

    #[test]
    fn nonpositive_window_is_treated_as_unknown() {
        assert_eq!(ContextBudget::measure(&[], Some(0)).context_window, None);
        assert_eq!(ContextBudget::measure(&[], Some(-5)).context_window, None);
    }
}
