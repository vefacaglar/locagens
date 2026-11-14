//! One agent's model <-> tool rounds: streaming a round, running its tool
//! calls, folding queued messages, and context compaction.

use super::*;
use super::drive::{cost_of, endpoint_http, resolve_endpoint};

/// Tells the model why a call did not run when its arguments were cut off.
const TRUNCATED_ARGUMENTS: &str = "The tool call's arguments were cut off at the model's output token limit, so the \
call did not run. Retry with a smaller call (for example, write a large file in several smaller edits).";

/// Appended to a final answer that hit the output-token limit.
const TRUNCATED_ANSWER_NOTE: &str = "\n\n[The answer was cut off at the model's output token limit.]";

impl AgentLoop {
    /// Runs the loop until the model produces a final answer.
    pub(super) async fn run(&mut self, max_rounds: usize) -> Result<String, AgentError> {
        let mut stop_attempts = 0u32;
        for round in 1..=max_rounds {
            self.check_cancel()?;
            // Codex-style auto compaction: when the estimated input tokens
            // exceed the threshold, summarize older history before the call.
            self.maybe_auto_compact().await?;
            if self.agent_role.is_none() {
                self.handle.emit(json!({ "type": "round_started", "round": round }));
            }
            let (mut message, outcome) = self.stream_round().await?;
            self.record_usage(outcome.usage);

            if outcome.tool_calls.is_empty() {
                if outcome.truncated {
                    message.content.push_str(TRUNCATED_ANSWER_NOTE);
                    self.save_message(&message);
                }
                // The main agent's Stop hook may ask it to continue.
                if self.agent_role.is_none() && self.stop_hook_continues(&message, &mut stop_attempts).await {
                    continue;
                }
                // A message queued while the answer streamed starts the next
                // round instead of being lost (FR-010).
                let queued = self.take_queued().await;
                if !queued.is_empty() {
                    self.history.push(to_llm_message(&message));
                    self.add_queued(queued);
                    continue;
                }
                return Ok(message.content);
            }
            self.run_tool_calls(&message, outcome.tool_calls, outcome.truncated).await?;
            let queued = self.take_queued().await;
            self.add_queued(queued);
        }
        Err(AgentError::Failed(format!(
            "Stopped after {max_rounds} rounds without a final answer. Send a message to let the agent continue."
        )))
    }

    /// Streams one model call into a persisted assistant message. On cancel
    /// or failure the text streamed so far is persisted before returning.
    async fn stream_round(&mut self) -> Result<(RunMessage, llm::StreamOutcome), AgentError> {
        let endpoint = resolve_endpoint(
            &self.state.pi,
            &self.provider_id,
            &self.model,
            self.reasoning_effort.as_deref(),
            &self.run_id,
        )
        .await
        .map_err(AgentError::Failed)?;
        // Every request starts from a provider-valid history (FR-001..003).
        history::sanitize_in_place(&mut self.history);

        // Assistant placeholder, streamed live.
        let mut message = self.new_message("assistant", "");
        self.persist_and_emit(&message);
        let message_id = message.id.clone();

        let mut content = String::new();
        let mut reasoning = String::new();
        let mut last_emit = Instant::now() - Duration::from_secs(1);
        let mut last_persist = Instant::now() - Duration::from_secs(1);
        // Live `apply_patch` progress while the call's arguments stream.
        let mut patch_streams: HashMap<usize, patch_progress::PatchArgsStream> = HashMap::new();
        let mut last_patch_emit = Instant::now() - Duration::from_secs(1);

        let http = endpoint_http(&self.state);
        let stream = {
            let mut on_delta = |content_delta: &str, reasoning_delta: &str| {
                content.push_str(content_delta);
                reasoning.push_str(reasoning_delta);
                message.content = content.clone();
                if !reasoning.is_empty() {
                    message.reasoning_content = Some(reasoning.clone());
                }
                if last_emit.elapsed() >= STREAM_EMIT_INTERVAL {
                    self.handle.emit(json!({
                        "type": "message_updated",
                        "message": serde_json::to_value(&message).unwrap_or(Value::Null),
                    }));
                    last_emit = Instant::now();
                }
                if last_persist.elapsed() >= STREAM_PERSIST_INTERVAL {
                    self.state.db.stream_message(&self.run_id, &message_id, &content, Some(&reasoning));
                    last_persist = Instant::now();
                }
            };
            let mut on_tool_args = |progress: &llm::ToolArgsProgress| {
                let patch = patch_streams.entry(progress.index).or_default();
                let changed = patch.update(&progress.arguments);
                if changed && (patch.ended() || last_patch_emit.elapsed() >= STREAM_EMIT_INTERVAL) {
                    last_patch_emit = Instant::now();
                    self.handle.emit(patch.event(&self.run_id, &message_id, &progress.call_id, self.agent_name.as_deref()));
                }
            };
            let should_stop = || self.stop_requested();
            let call = llm::stream_chat_observed(
                &http,
                &endpoint,
                &self.system,
                &self.history,
                &self.tools,
                &mut on_delta,
                &mut on_tool_args,
                &should_stop,
            );
            // Race the whole call (request, retries, stream) against
            // cancellation so a stalled provider cannot pin a cancelled loop.
            tokio::select! {
                stream = call => Some(stream),
                _ = self.stop_signal() => None,
            }
        };

        let outcome = match stream {
            Some(Ok(outcome)) if !self.stop_requested() => outcome,
            ended => {
                // Keep every streamed character in the rollout (FR-011).
                message.content = content;
                if !reasoning.is_empty() {
                    message.reasoning_content = Some(reasoning);
                }
                self.save_message(&message);
                return Err(match ended {
                    Some(Err(err)) if err.to_string() != "cancelled" && !self.stop_requested() => {
                        AgentError::Failed(err.to_string())
                    }
                    _ => AgentError::Cancelled,
                });
            }
        };
        message.content = outcome.content.clone();
        if !outcome.reasoning.is_empty() {
            message.reasoning_content = Some(outcome.reasoning.clone());
        }
        if !outcome.tool_calls.is_empty() {
            message.raw_response = Some(serde_json::to_string(&outcome.tool_calls).unwrap_or_default());
        }
        if !outcome.thinking_blocks.is_empty() {
            message.thinking_blocks = Some(Value::Array(outcome.thinking_blocks.clone()));
        }
        self.save_message(&message);
        Ok((message, outcome))
    }

    /// Persists a generated message's final state and tells the frontends.
    pub(super) fn save_message(&self, message: &RunMessage) {
        self.state.db.finish_message(message);
        self.handle.emit(json!({
            "type": "message_updated",
            "message": serde_json::to_value(message).unwrap_or(Value::Null),
        }));
    }

    /// Usage accounting (main and sub-agent roles are both logged).
    fn record_usage(&self, usage: Option<llm::Usage>) {
        let Some(usage) = usage else { return };
        let info = self.state.pi.model_info(&self.provider_id, &self.model);
        let role = self.agent_role.as_deref().unwrap_or(crate::db::MAIN_AGENT_ROLE);
        self.state.db.append_usage(UsageLogEntry {
            id: 0,
            run_id: self.run_id.clone(),
            agent_role: Some(role.to_string()),
            provider_id: self.provider_id.clone(),
            model: self.model.clone(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            // `input_tokens` includes cached tokens for every protocol, so
            // this stays within 0-100 % (FR-078).
            cache_hit_rate: if usage.input_tokens > 0 {
                usage.cache_read_tokens as f64 * 100.0 / usage.input_tokens as f64
            } else {
                0.0
            },
            cost: cost_of(&usage, info.as_ref()),
            created_at: crate::util::now_iso(),
            duration_ms: None,
        });
        // Lets frontends refresh usage / context fill while the run is
        // still going (not only when it ends).
        self.handle.emit(json!({ "type": "usage_recorded", "runId": self.run_id, "agentRole": role }));
    }

    /// Runs the tool calls of one assistant message and records each result.
    async fn run_tool_calls(&mut self, message: &RunMessage, tool_calls: Vec<Value>, truncated: bool) -> Result<(), AgentError> {
        let assistant_at = self.history.len();
        self.history.push(to_llm_message(message));
        let mut rewritten = false;
        for (index, call) in tool_calls.iter().enumerate() {
            self.check_cancel()?;
            let call_id = call.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
            let name = call.pointer("/function/name").and_then(Value::as_str).unwrap_or_default().to_string();
            let arguments = call.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}");
            let result = match parse_tool_arguments(arguments, truncated) {
                Err(error) => json!({ "ok": false, "error": error }).to_string(),
                Ok(args) => {
                    let (result, executed) = self.run_one_tool(&call_id, &name, args).await?;
                    if let Some(executed) = executed {
                        rewritten |= self.rewrite_call_arguments(assistant_at, index, &executed);
                    }
                    result
                }
            };
            self.record_tool_result(&call_id, result);
        }
        if rewritten {
            // The stored call shows the input that actually ran (FR-020).
            let mut updated = message.clone();
            updated.raw_response = self.history[assistant_at].tool_calls.as_ref().map(|calls| Value::Array(calls.clone()).to_string());
            self.state.db.finish_message(&updated);
        }
        Ok(())
    }

    /// PreToolUse hook, dispatch and PostToolUse hook of one call. Returns
    /// the result and, when a hook rewrote the input, the input that ran.
    async fn run_one_tool(&mut self, call_id: &str, name: &str, args: Value) -> Result<(String, Option<Value>), AgentError> {
        let pre = self.run_pre_tool_hook(call_id, name, &args).await;
        if pre.blocked {
            let reason = pre.reason.unwrap_or_else(|| "blocked".to_string());
            let result = json!({ "ok": false, "error": format!("Blocked by PreToolUse hook: {reason}") }).to_string();
            return Ok((self.run_post_tool_hook(call_id, name, &args, result).await, None));
        }
        let rewritten = pre.updated_input.filter(|input| *input != args);
        let executed = rewritten.clone().unwrap_or(args);
        self.handle.note_tool_paths(&self.project_path, name, &executed);
        let result = match self.dispatch_tool(call_id, name, &executed).await {
            Ok(result) => result,
            Err(AgentError::Cancelled) => return Err(AgentError::Cancelled),
            Err(AgentError::Failed(err)) => format!("Tool error: {err}"),
        };
        // PostToolUse sees the input that actually ran.
        let result = self.run_post_tool_hook(call_id, name, &executed, result).await;
        Ok((result, rewritten))
    }

    /// Replaces the arguments of call `index` of the assistant message at
    /// `assistant_at` in the history.
    fn rewrite_call_arguments(&mut self, assistant_at: usize, index: usize, executed: &Value) -> bool {
        let call = self.history[assistant_at].tool_calls.as_mut().and_then(|calls| calls.get_mut(index));
        match call.and_then(|call| call.get_mut("function")).and_then(Value::as_object_mut) {
            Some(function) => {
                function.insert("arguments".into(), json!(executed.to_string()));
                true
            }
            None => false,
        }
    }

    /// Persists a tool result and appends it to the history. A `view_image`
    /// payload is stored without its bytes; the history keeps them once for
    /// the requests of this turn (FR-027).
    fn record_tool_result(&mut self, call_id: &str, result: String) {
        let stored = llm::tool_image_text(&result).map(|compact| image_reference(&compact));
        let content = if stored.is_some() { without_data_url(&result) } else { result };
        let mut tool_message = self.new_message("tool", stored.as_deref().unwrap_or(&content));
        tool_message.raw_response = Some(call_id.to_string());
        self.persist_and_emit(&tool_message);
        self.history.push(llm::LlmMessage {
            role: "tool".into(),
            content,
            tool_call_id: Some(call_id.to_string()),
            source_id: Some(tool_message.id.clone()),
            ..Default::default()
        });
    }

    /// Queued mid-run user messages. Only the main agent reads the run's
    /// inbox; a sub-agent reads its own `send_message` queue (FR-010).
    async fn take_queued(&mut self) -> Vec<String> {
        let mut queued: Vec<String> = Vec::new();
        if self.agent_role.is_none() {
            queued.extend(self.handle.inbox.lock().await.drain(..));
        }
        if let Some(agent_inbox) = self.agent_inbox.as_mut() {
            while let Ok(command) = agent_inbox.try_recv() {
                queued.push(command.into_text());
            }
        }
        queued
    }

    /// Persists queued messages as user messages and adds them to the history.
    fn add_queued(&mut self, queued: Vec<String>) {
        for content in queued {
            let user_message = self.new_message("user", &content);
            self.persist_and_emit(&user_message);
            self.history.push(to_llm_message(&user_message));
        }
    }

    // ------------------------------------------------------------------
    // Compaction
    // ------------------------------------------------------------------

    /// The model's context window from the provider catalog.
    pub(super) fn context_window(&self) -> Option<i64> {
        self.state.pi.model_info(&self.provider_id, &self.model).and_then(|info| info.context_window)
    }

    /// Auto-compaction threshold for this agent's model and request overhead.
    pub(super) fn auto_compact_threshold(&self) -> usize {
        let overhead = compact::estimate_tokens(&self.system)
            + self.tools.iter().map(|tool| compact::estimate_tokens(&tool.to_string())).sum::<usize>();
        compact::auto_compact_threshold(self.context_window(), overhead)
    }

    /// Codex-style compaction: summarize older history with the
    /// SUMMARIZATION_PROMPT and replace it with a summary block. Sub-agents
    /// compact too (FR-005).
    pub(super) async fn maybe_auto_compact(&mut self) -> Result<(), AgentError> {
        if !compact::should_auto_compact_at(&self.history, self.auto_compact_threshold()) {
            return Ok(());
        }
        self.run_compaction("auto").await
    }

    pub(super) async fn run_compaction(&mut self, trigger: &str) -> Result<(), AgentError> {
        if self.pre_compact_blocks(trigger).await {
            return Ok(());
        }
        let keep = compact::DEFAULT_KEEP_LAST_MESSAGES;
        let Some(chunks) = compact::summarization_chunks(&self.history, keep) else {
            return Ok(());
        };
        let summary = self.summarize_chunks(&chunks).await?;
        let estimated_tokens = compact::estimate_history_tokens(&self.history);
        let messages_before = self.history.len();
        let first_kept = compact::first_kept_id(&self.history, keep);
        self.history = compact::apply_compaction(&self.history, &summary, keep);
        if self.agent_role.is_none() {
            self.persist_compaction_marker(&summary, first_kept.as_deref());
        }
        self.handle.emit(json!({
            "type": "context_compacted",
            "estimatedTokensBefore": estimated_tokens,
            "messagesBefore": messages_before,
            "messagesAfter": self.history.len(),
            "summary": summary,
            "agentName": self.agent_name,
        }));
        self.post_compact_hook(trigger).await;
        Ok(())
    }

    /// Records the compaction in the rollout so `seed_history` replays the
    /// compacted history on the next turn instead of summarizing again.
    fn persist_compaction_marker(&self, summary: &str, first_kept: Option<&str>) {
        let mut marker = self.new_message("system", &compact::summary_block(summary));
        marker.raw_response = Some(compact::marker_raw(first_kept));
        self.state.db.append_message(marker);
    }

    /// Summarizes the transcript chunks in order, each call extending the
    /// summary of the chunks before it, so nothing is dropped (FR-004).
    async fn summarize_chunks(&self, chunks: &[String]) -> Result<String, AgentError> {
        let endpoint = resolve_endpoint(
            &self.state.pi,
            &self.provider_id,
            &self.model,
            self.reasoning_effort.as_deref(),
            &self.run_id,
        )
        .await
        .map_err(AgentError::Failed)?;
        let mut summary: Option<String> = None;
        for (index, chunk) in chunks.iter().enumerate() {
            let prompt = compact::chunk_prompt(summary.as_deref(), chunk, index, chunks.len());
            summary = Some(self.summarize_once(&endpoint, prompt).await?);
        }
        Ok(summary.unwrap_or_default())
    }

    async fn summarize_once(&self, endpoint: &llm::Endpoint, prompt: String) -> Result<String, AgentError> {
        let messages = vec![llm::LlmMessage { role: "user".into(), content: prompt, ..Default::default() }];
        let http = endpoint_http(&self.state);
        let mut ignore_deltas = |_content_delta: &str, _reasoning_delta: &str| {};
        let should_stop = || self.stop_requested();
        let call = llm::stream_chat(&http, endpoint, compact::SUMMARIZATION_PROMPT, &messages, &[], &mut ignore_deltas, &should_stop);
        let outcome = tokio::select! {
            outcome = call => outcome,
            _ = self.stop_signal() => return Err(AgentError::Cancelled),
        };
        let outcome = outcome.map_err(|err| {
            if err.to_string() == "cancelled" || self.stop_requested() {
                AgentError::Cancelled
            } else {
                AgentError::Failed(format!("compact_context: {err}"))
            }
        })?;
        if outcome.content.trim().is_empty() {
            return Err(AgentError::Failed("compact_context: the summarizer returned no summary.".into()));
        }
        Ok(outcome.content)
    }
}

/// Parsed tool arguments, or the error the model gets instead of running
/// the call with `{}` (FR-012, FR-020).
fn parse_tool_arguments(raw: &str, truncated: bool) -> Result<Value, String> {
    match serde_json::from_str::<Value>(raw) {
        Ok(args) if args.is_object() => Ok(args),
        _ if truncated => Err(TRUNCATED_ARGUMENTS.to_string()),
        Ok(_) => Err("The tool call's arguments must be a JSON object; the call did not run.".to_string()),
        Err(err) => Err(format!("The tool call's arguments are not valid JSON ({err}); the call did not run.")),
    }
}

/// The stored form of a `view_image` result: its JSON without the bytes.
fn image_reference(compact_json: &str) -> String {
    let mut value: Value = serde_json::from_str(compact_json).unwrap_or_else(|_| json!({}));
    if let Some(object) = value.as_object_mut() {
        object.insert("image_omitted".into(), json!("The image bytes are not stored; call view_image again to see it."));
    }
    value.to_string()
}

/// A `view_image` result without its duplicate `data_url` field.
fn without_data_url(result: &str) -> String {
    match serde_json::from_str::<Value>(result) {
        Ok(Value::Object(mut object)) => {
            object.remove("data_url");
            Value::Object(object).to_string()
        }
        _ => result.to_string(),
    }
}
