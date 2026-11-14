//! Reading earlier prompts back from `history.jsonl` (prompt recall in `lcgn`).

use super::*;

impl Db {
    /// The last `limit` prompts typed in `session_ids` (oldest first), from
    /// `history.jsonl`; consecutive repeats are collapsed.
    pub fn prompt_history(&self, session_ids: &std::collections::HashSet<String>, limit: usize) -> Vec<String> {
        let Ok(raw) = std::fs::read_to_string(self.root().join(HISTORY_FILE)) else {
            return Vec::new();
        };
        let mut prompts: Vec<String> = Vec::new();
        for line in raw.lines() {
            let Ok(entry) = serde_json::from_str::<Value>(line) else { continue };
            let wanted = entry
                .get("session_id")
                .and_then(Value::as_str)
                .is_some_and(|id| session_ids.contains(id));
            let text = entry.get("text").and_then(Value::as_str).unwrap_or("").trim();
            if wanted && !text.is_empty() && prompts.last().map(String::as_str) != Some(text) {
                prompts.push(text.to_string());
            }
        }
        let skip = prompts.len().saturating_sub(limit);
        prompts.split_off(skip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_last_prompts_of_the_given_sessions_only() {
        let dir = std::env::temp_dir().join(format!("lcgn-history-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&dir).unwrap();
        for (session, text) in [("a", "one"), ("b", "other project"), ("a", "two"), ("a", "two"), ("a", "three")] {
            rollout::append_history(&dir, session, text, 1);
        }
        let ids: std::collections::HashSet<String> = ["a".to_string()].into();
        assert_eq!(db.prompt_history(&ids, 100), ["one", "two", "three"]);
        assert_eq!(db.prompt_history(&ids, 2), ["two", "three"]);
        assert!(db.prompt_history(&Default::default(), 10).is_empty());
    }
}
