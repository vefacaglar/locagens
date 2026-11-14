//! Messages and plans of a run.

use super::*;

impl Db {
    // ------------------------------------------------------------------
    // Messages
    // ------------------------------------------------------------------

    pub fn append_message(&self, message: RunMessage) {
        let mut guard = self.write();
        let state = &mut *guard;
        if state.deleted_runs.contains(&message.run_id) {
            return;
        }
        state
            .messages
            .entry(message.run_id.clone())
            .or_default()
            .push(message.clone());
        append_event(
            &self.root,
            state,
            &message.run_id,
            &message.created_at,
            "message",
            message_payload(&message),
        );
        let ephemeral = state.runs.get(&message.run_id).is_some_and(|run| run.ephemeral);
        if message.role == "user" && message.agent_role.is_none() && !ephemeral {
            append_history(&self.root, &message.run_id, &message.content, unix_secs(&message.created_at));
        }
    }

    pub fn list_messages(&self, run_id: &str) -> Vec<RunMessage> {
        self.read()
            .messages
            .get(run_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Updates a message in the in-memory index and appends a
    /// `message_update` line carrying the replacement message.
    pub fn update_message(
        &self,
        run_id: &str,
        message_id: &str,
        content: &str,
        reasoning_content: Option<&str>,
    ) -> Option<RunMessage> {
        self.update_message_full(run_id, message_id, content, reasoning_content, None)
    }

    /// Like `update_message`, but can also replace the serialized
    /// `rawResponse` blob (used for tool-call payloads on assistant messages).
    pub fn update_message_full(
        &self,
        run_id: &str,
        message_id: &str,
        content: &str,
        reasoning_content: Option<&str>,
        raw_response: Option<&str>,
    ) -> Option<RunMessage> {
        self.apply_message_update(
            run_id,
            message_id,
            |message| set_text(message, content, reasoning_content, raw_response),
            true,
        )
    }

    /// Persists the final state of a generated message: content, reasoning,
    /// `rawResponse` and thinking blocks. Also used when a round is cancelled
    /// or fails, so the rollout keeps every streamed character (FR-011).
    pub fn finish_message(&self, message: &RunMessage) -> Option<RunMessage> {
        self.apply_message_update(
            &message.run_id,
            &message.id,
            |stored| {
                set_text(stored, &message.content, message.reasoning_content.as_deref(), message.raw_response.as_deref());
                if message.thinking_blocks.is_some() {
                    stored.thinking_blocks = message.thinking_blocks.clone();
                }
            },
            true,
        )
    }

    /// A streaming update of a message that is still being generated: the
    /// in-memory copy is always current, but the rollout only gets a
    /// `message_update` checkpoint every [`STREAM_CHECKPOINT_INTERVAL`]. The
    /// final `finish_message` always persists, so a 100 KB answer costs a
    /// handful of lines instead of hundreds (QA-035). A crash loses at most the
    /// last few seconds of streamed text of a run that is failed on restart anyway.
    pub fn stream_message(
        &self,
        run_id: &str,
        message_id: &str,
        content: &str,
        reasoning_content: Option<&str>,
    ) -> Option<RunMessage> {
        self.apply_message_update(run_id, message_id, |message| set_text(message, content, reasoning_content, None), false)
    }

    pub(super) fn apply_message_update(
        &self,
        run_id: &str,
        message_id: &str,
        update: impl FnOnce(&mut RunMessage),
        force_persist: bool,
    ) -> Option<RunMessage> {
        let mut guard = self.write();
        let state = &mut *guard;
        let message = state
            .messages
            .get_mut(run_id)?
            .iter_mut()
            .find(|message| message.id == message_id)?;
        update(message);
        let updated = message.clone();
        let now = Instant::now();
        let due = match state.stream_checkpoints.get(message_id) {
            Some(last) => now.duration_since(*last) >= STREAM_CHECKPOINT_INTERVAL,
            None => {
                // The first streamed chunk only starts the clock: the
                // placeholder message is already on disk.
                state.stream_checkpoints.insert(message_id.to_string(), now);
                false
            }
        };
        if !force_persist && !due {
            return Some(updated);
        }
        if force_persist {
            state.stream_checkpoints.remove(message_id);
        } else {
            state.stream_checkpoints.insert(message_id.to_string(), now);
        }
        append_event(
            &self.root,
            state,
            run_id,
            &updated.created_at,
            "message_update",
            message_payload(&updated),
        );
        Some(updated)
    }

    // ------------------------------------------------------------------
    // Plans
    // ------------------------------------------------------------------

    pub fn get_plan(&self, run_id: &str) -> Option<Plan> {
        self.read().plans.get(run_id).cloned()
    }

    pub fn set_plan(&self, plan: Plan) {
        let mut guard = self.write();
        let state = &mut *guard;
        if state.deleted_runs.contains(&plan.run_id) {
            return;
        }
        state.plans.insert(plan.run_id.clone(), plan.clone());
        append_event(&self.root, state, &plan.run_id, &plan.created_at, "plan", plan_payload(&plan));
    }
}

/// Sets the text fields of a message; `None` keeps reasoning / `rawResponse`.
fn set_text(message: &mut RunMessage, content: &str, reasoning_content: Option<&str>, raw_response: Option<&str>) {
    message.content = content.to_string();
    if let Some(reasoning) = reasoning_content {
        message.reasoning_content = Some(reasoning.to_string());
    }
    if let Some(raw) = raw_response {
        message.raw_response = Some(raw.to_string());
    }
}

/// Replaces `items[id]` or pushes when absent.
pub(super) fn upsert_message(messages: &mut Vec<RunMessage>, message: RunMessage) {
    match messages.iter().rposition(|existing| existing.id == message.id) {
        Some(index) => messages[index] = message,
        None => messages.push(message),
    }
}
