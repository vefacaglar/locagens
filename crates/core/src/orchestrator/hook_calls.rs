//! Lifecycle-hook call sites for the agent loop, kept out of `mod.rs` so the
//! drive loop only carries one-line calls into this file. Event semantics
//! live in `services::hooks`.

use super::{AgentError, AgentLoop};
use crate::services::hooks;
use serde_json::{json, Value};

/// Stop hooks may ask the agent to keep going at most this many times per
/// final answer (guards against hooks that always block).
const MAX_STOP_CONTINUATIONS: u32 = 3;

/// A `PermissionRequest` hook's verdict.
pub(super) enum PermissionVerdict {
    Allow,
    Deny(String),
    /// No hook answered: ask the user.
    Ask,
}

impl AgentLoop {
    fn fires_hooks(&self) -> bool {
        !self.project_path.as_os_str().is_empty()
    }

    /// `UserPromptSubmit`: appends hook context to the system prompt and
    /// returns the block reason when a hook rejected the prompt.
    pub(super) async fn user_prompt_hook(&mut self, prompt: &str) -> Option<String> {
        if !self.fires_hooks() {
            return None;
        }
        let outcome = hooks::fire(
            "UserPromptSubmit",
            &self.project_path,
            &self.run_id,
            json!({ "prompt": prompt, "model": self.model, "permission_mode": self.mode }),
        )
        .await;
        if let Some(context) = outcome.context_joined() {
            self.system.push_str("\n\n## Hook context\n");
            self.system.push_str(&context);
        }
        outcome
            .blocked
            .then(|| outcome.reason.unwrap_or_else(|| "blocked".to_string()))
    }

    /// `Stop` (+ the `notify` command). Returns `true` when a hook blocked
    /// the stop: its reason was fed back as a user message and the loop
    /// should run another round.
    pub(super) async fn stop_hook_continues(&mut self, final_text: &str, attempts: &mut u32) -> bool {
        if self.fires_hooks() {
            let outcome = hooks::fire(
                "Stop",
                &self.project_path,
                &self.run_id,
                json!({
                    "stop_hook_active": *attempts > 0,
                    "last_assistant_message": final_text,
                    "model": self.model,
                    "permission_mode": self.mode,
                }),
            )
            .await;
            if outcome.blocked && *attempts < MAX_STOP_CONTINUATIONS {
                *attempts += 1;
                let reason = outcome.reason.unwrap_or_else(|| "A Stop hook asked you to continue.".to_string());
                let feedback = format!("Stop hook feedback: {reason}");
                self.history.push(super::llm::LlmMessage {
                    role: "assistant".into(),
                    content: final_text.to_string(),
                    ..Default::default()
                });
                let message = self.new_message("user", &feedback);
                self.persist_and_emit(&message);
                self.history.push(super::llm::LlmMessage {
                    role: "user".into(),
                    content: feedback,
                    ..Default::default()
                });
                return true;
            }
        }
        crate::config_runtime::notify_turn_complete(
            &self.run_id,
            &self.project_path.display().to_string(),
            final_text,
        );
        false
    }

    /// `PermissionRequest`: lets a hook answer before the user is asked.
    pub(super) async fn permission_hook(&self, call_id: &str, name: &str, args: &Value) -> PermissionVerdict {
        if !self.fires_hooks() {
            return PermissionVerdict::Ask;
        }
        let outcome = hooks::fire(
            "PermissionRequest",
            &self.project_path,
            &self.run_id,
            json!({
                "tool_name": name,
                "tool_input": args,
                "tool_use_id": call_id,
                "permission_mode": self.mode,
            }),
        )
        .await;
        match outcome.permission_allow {
            Some(true) => PermissionVerdict::Allow,
            Some(false) => PermissionVerdict::Deny(outcome.reason.unwrap_or_else(|| "denied".to_string())),
            None if outcome.blocked => {
                PermissionVerdict::Deny(outcome.reason.unwrap_or_else(|| "denied".to_string()))
            }
            None => PermissionVerdict::Ask,
        }
    }

    /// `Notification`: the run is waiting on the user
    /// (`permission_prompt` / `elicitation_dialog`).
    pub(super) async fn notification_hook(&self, notification_type: &str, message: &str) {
        if self.fires_hooks() {
            hooks::fire(
                "Notification",
                &self.project_path,
                &self.run_id,
                json!({ "notification_type": notification_type, "message": message }),
            )
            .await;
        }
    }

    /// `PreCompact`; `true` means a hook vetoed the compaction.
    pub(super) async fn pre_compact_blocks(&self, trigger: &str) -> bool {
        self.fires_hooks()
            && hooks::fire("PreCompact", &self.project_path, &self.run_id, json!({ "trigger": trigger }))
                .await
                .blocked
    }

    /// `PostCompact`.
    pub(super) async fn post_compact_hook(&self, trigger: &str) {
        if self.fires_hooks() {
            hooks::fire("PostCompact", &self.project_path, &self.run_id, json!({ "trigger": trigger })).await;
        }
    }

    /// `SubagentStart`.
    pub(super) async fn subagent_start_hook(&self) {
        if self.fires_hooks() {
            hooks::fire(
                "SubagentStart",
                &self.project_path,
                &self.run_id,
                json!({ "agent_type": self.agent_role, "agent_name": self.agent_name }),
            )
            .await;
        }
    }

    /// `SubagentStop` with the sub-agent's final output.
    pub(super) async fn subagent_stop_hook(&self, result: &Result<String, AgentError>) {
        if self.fires_hooks() {
            let last = result.as_ref().map(String::as_str).unwrap_or_default();
            hooks::fire(
                "SubagentStop",
                &self.project_path,
                &self.run_id,
                json!({
                    "agent_type": self.agent_role,
                    "agent_name": self.agent_name,
                    "last_assistant_message": last,
                }),
            )
            .await;
        }
    }
}

/// `SessionEnd` when a run's drive task finishes.
pub(super) async fn session_end_hook(project_path: &str, run_id: &str, result: &Result<String, AgentError>) {
    if project_path.is_empty() {
        return;
    }
    let reason = match result {
        Ok(_) => "done",
        Err(AgentError::Cancelled) => "cancelled",
        Err(AgentError::Failed(_)) => "failed",
    };
    hooks::fire("SessionEnd", std::path::Path::new(project_path), run_id, json!({ "reason": reason })).await;
}
