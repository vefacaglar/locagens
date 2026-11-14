//! Agent-loop plumbing: cancellation, messages, hooks and tool dispatch
//! (the round loop itself is in `agent_round.rs`).

use super::*;
use super::interactive::normalize_request_user_input;

impl AgentLoop {
    pub(super) fn interrupted(&self) -> bool {
        self.extras.interrupt.as_ref().is_some_and(|flag| flag.load(Ordering::SeqCst))
    }

    /// True when the whole run was cancelled or this agent was interrupted.
    pub(super) fn stop_requested(&self) -> bool {
        self.handle.is_cancelled() || self.interrupted()
    }

    /// Resolves once the run is cancelled or this agent is interrupted (for
    /// `tokio::select!` around long awaits).
    pub(super) async fn stop_signal(&self) {
        match self.extras.interrupt.as_ref() {
            Some(flag) => tokio::select! {
                _ = self.handle.cancelled() => {}
                _ = agents::interrupt_raised(flag) => {}
            },
            None => self.handle.cancelled().await,
        }
    }

    pub(super) fn check_cancel(&self) -> Result<(), AgentError> {
        if self.stop_requested() {
            Err(AgentError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// `get_context_remaining`: window from the provider catalog, usage from
    /// the auto-compactor's own estimate.
    pub(super) fn context_remaining(&self) -> String {
        let budget =
            context_remaining::ContextBudget::measure(&self.history, self.context_window(), self.auto_compact_threshold());
        serde_json::to_string_pretty(&budget.to_json()).unwrap_or_default()
    }

    pub(super) fn new_message(&self, role: &str, content: &str) -> RunMessage {
        RunMessage {
            id: format!("msg-{}", uuid::Uuid::new_v4()),
            run_id: self.run_id.clone(),
            role: role.to_string(),
            agent_role: self.agent_role.clone(),
            agent_name: self.agent_name.clone(),
            provider_id: if role == "assistant" { Some(self.provider_id.clone()) } else { None },
            provider_display_name: if role == "assistant" { Some(self.provider_display_name.clone()) } else { None },
            model: if role == "assistant" { Some(self.model.clone()) } else { None },
            content: content.to_string(),
            created_at: crate::util::now_iso(),
            ..Default::default()
        }
    }

    pub(super) fn persist_and_emit(&self, message: &RunMessage) {
        self.state.db.append_message(message.clone());
        self.handle.emit(json!({
            "type": "message_created",
            "message": serde_json::to_value(message).unwrap_or(Value::Null),
        }));
    }

    // ------------------------------------------------------------------
    // Lifecycle hooks (PreToolUse / PostToolUse / Stop)
    // ------------------------------------------------------------------

    pub(super) fn hook_payload(&self, event: &str, extra: Value) -> Value {
        let mut payload = json!({
            "session_id": self.run_id,
            "cwd": self.project_path.display().to_string(),
            "hook_event_name": event,
        });
        if let (Some(obj), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                obj.insert(key.clone(), value.clone());
            }
        }
        payload
    }

    /// Runs `PreToolUse` hooks; a blocked outcome suppresses the tool call.
    pub(super) async fn run_pre_tool_hook(&self, call_id: &str, name: &str, args: &Value) -> crate::services::hooks::HookOutcome {
        let payload = self.hook_payload(
            "PreToolUse",
            json!({
                "tool_name": name,
                "tool_input": args,
                "tool_use_id": call_id,
            }),
        );
        crate::services::hooks::run_hook_event("PreToolUse", &self.project_path, &payload).await
    }

    /// Runs `PostToolUse` hooks and appends any additional context to the
    /// tool response.
    pub(super) async fn run_post_tool_hook(&self, call_id: &str, name: &str, args: &Value, mut result: String) -> String {
        let payload = self.hook_payload(
            "PostToolUse",
            json!({
                "tool_name": name,
                "tool_input": args,
                "tool_response": result,
                "tool_use_id": call_id,
            }),
        );
        let outcome = crate::services::hooks::run_hook_event("PostToolUse", &self.project_path, &payload).await;
        if let Some(context) = outcome.context_joined() {
            result.push_str(&format!("\n[hook context]: {context}"));
        }
        if outcome.blocked {
            result.push_str(&format!("\n[hook feedback]: {}", outcome.reason.unwrap_or_else(|| "blocked".into())));
        }
        result
    }

    /// Whether `name` was offered to this agent: an advertised tool spec, a
    /// deferred MCP spec, or `tool_search` while specs are deferred.
    pub(super) fn tool_is_offered(&self, name: &str) -> bool {
        let named = |tool: &Value| tool.pointer("/function/name").and_then(Value::as_str) == Some(name);
        self.tools.iter().any(named)
            || self.extras.deferred_tools.iter().any(named)
            || (name == tool_search::TOOL_SEARCH_NAME && !self.extras.deferred_tools.is_empty())
    }

    /// Executes one tool call, routing interactive/delegation tools to the
    /// orchestrator and workspace tools through permission gating.
    pub(super) async fn dispatch_tool(&mut self, call_id: &str, name: &str, args: &Value) -> Result<String, AgentError> {
        // Models do call tools they were never offered: only the advertised
        // set (plus deferred MCP specs behind `tool_search`) may run.
        if !self.tool_is_offered(name) {
            return Err(AgentError::Failed(format!(
                "Unknown tool \"{name}\": it is not available to this agent."
            )));
        }
        let readonly_mode = if self.extras.readonly { "plan" } else { self.mode.as_str() };
        if tools::is_forbidden_in_readonly_mode(readonly_mode, name) {
            let error = if self.extras.readonly {
                format!("Tool '{name}' is not allowed for a read-only agent.")
            } else {
                format!("Tool '{name}' is not allowed in '{}' mode.", self.mode)
            };
            return Ok(serde_json::json!({ "ok": false, "error": error }).to_string());
        }
        match name {
            "set_chat_title" => {
                let title = args.get("title").and_then(Value::as_str).unwrap_or_default().trim().to_string();
                if title.is_empty() {
                    return Err(AgentError::Failed("set_chat_title: title is empty.".into()));
                }
                self.state.db.patch_run(&self.run_id, |run| run.title = title.clone());
                self.handle.emit(json!({ "type": "run_title_changed", "runId": self.run_id, "title": title }));
                Ok("Chat title updated.".to_string())
            }
            "update_plan" => self.update_plan(args).await,
            "remember" => {
                let scope = args.get("scope").and_then(Value::as_str).unwrap_or("project");
                let category = args.get("category").and_then(Value::as_str).unwrap_or("project");
                let content = args.get("content").and_then(Value::as_str).unwrap_or_default().trim().to_string();
                if content.is_empty() {
                    return Err(AgentError::Failed("remember: content is empty.".into()));
                }
                if let Some(denied) = self.gate_remember(call_id, scope, category, &content).await? {
                    return Ok(denied);
                }
                self.state
                    .db
                    .create_memory(scope, &self.project_path.display().to_string(), category, &content)
                    .map_err(|err| AgentError::Failed(err.to_string()))?;
                Ok(format!("Remembered ({scope}/{category})."))
            }
            "load_skill" => {
                let name = args.get("name").and_then(Value::as_str).unwrap_or_default();
                let skill = crate::services::skills::list(Some(&self.project_path.display().to_string()))
                    .into_iter()
                    .find(|skill| skill.name == name);
                match skill {
                    Some(skill) => Ok(skill.body.unwrap_or_default()),
                    None => Err(AgentError::Failed(format!("Skill \"{name}\" not found."))),
                }
            }
            "ask_user_question" => self.ask_user_question(args).await,
            "request_user_input" => {
                // Codex schema normalization: every question gets a header and
                // default options so the existing QuestionCard flow can render it.
                let normalized = normalize_request_user_input(args);
                self.ask_user_question(&normalized).await
            }
            "compact_context" => self.compact_context_tool().await,
            "spawn_agents" if self.depth_left > 0 => self.spawn_agents_tool(args).await,
            name if agents::is_agent_tool(name) && self.depth_left > 0 => self.dispatch_agent_tool(name, args).await,
            tool_search::TOOL_SEARCH_NAME => {
                tool_search::apply_search(&mut self.tools, &self.extras.deferred_tools, args).map_err(AgentError::Failed)
            }
            "get_context_remaining" => Ok(self.context_remaining()),
            "host_executable" => host_executable::execute(args).map_err(AgentError::Failed),
            "view_image" => tools::execute(&self.project_path, "view_image", args)
                .await
                .map_err(AgentError::Failed),
            "list_mcp_resources" => {
                let server = str_arg(args, "server").or_else(|| str_arg(args, "server_filter"));
                let cursor = str_arg(args, "cursor");
                crate::services::mcp::list_resources(
                    Some(&self.project_path.display().to_string()),
                    server,
                    cursor,
                )
                .await
                .map_err(AgentError::Failed)
            }
            "read_mcp_resource" => {
                let server = str_arg(args, "server").unwrap_or_default();
                let uri = str_arg(args, "uri").unwrap_or_default();
                if server.trim().is_empty() || uri.trim().is_empty() {
                    return Err(AgentError::Failed("read_mcp_resource: server and uri are required.".into()));
                }
                crate::services::mcp::read_resource(
                    Some(&self.project_path.display().to_string()),
                    server,
                    uri,
                )
                .await
                .map_err(AgentError::Failed)
            }
            name if name.starts_with("mcp__") => {
                // Qualified MCP tools are blocked in read-only modes; they
                // route through the gated executor elsewhere (permission is
                // required in ask_permissions mode via needs_permission).
                if self.mode == "chat" || self.mode == "plan" {
                    Ok(json!({
                        "ok": false,
                        "error": format!("MCP tools are not available in '{}' mode.", self.mode),
                    })
                    .to_string())
                } else {
                    self.gated_execute(call_id, name, args).await
                }
            }
            "exec_command" | "apply_patch" => self.gated_execute(call_id, name, args).await,
            "write_stdin" => self.write_stdin_tool(call_id, args).await,
            name if WORKSPACE_TOOLS.contains(&name) => self.gated_execute(call_id, name, args).await,
            other => Err(AgentError::Failed(format!("Unknown tool \"{other}\"."))),
        }
    }

    /// Executes one gated tool after permission decisions have been made.
    pub(super) async fn execute_tool(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "exec_command" => self.exec_session(args).await,
            "run_command" => {
                let (network, _approval_window) = self.command_network_policy(args);
                tools::run_command_with_network(&self.project_path, &self.mode, args, Some(network)).await
            }
            "apply_patch" => self.apply_patch_tool(args).await,
            name if name.starts_with("mcp__") => {
                let (server_key, tool_name) = crate::services::mcp::parse_qualified_mcp_tool(name)
                    .ok_or_else(|| format!("Invalid qualified MCP tool name: {name}"))?;
                crate::services::mcp::call_mcp_tool(
                    Some(&self.project_path.display().to_string()),
                    &server_key,
                    &tool_name,
                    args,
                )
                .await
            }
            _ => tools::execute_with_mode(&self.project_path, &self.mode, name, args).await,
        }
    }

    /// `apply_patch` tool: snapshots turn-diff baselines, applies the patch
    /// and returns Codex's summary plus the unified diff.
    pub(super) async fn apply_patch_tool(&self, args: &Value) -> Result<String, String> {
        let patch_text = args
            .get("input")
            .and_then(Value::as_str)
            .or_else(|| args.get("patch").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        if patch_text.trim().is_empty() {
            return Err("apply_patch: patch text is empty.".to_string());
        }
        let parsed = apply_patch::parse_patch(&patch_text).map_err(|err| err.to_string())?;
        let affected = apply_patch::affected_paths(&self.project_path, &parsed)?;
        // Snapshot baselines before anything mutates the disk.
        for relative in affected.all() {
            if let Ok(path) = tools::safe_path(&self.project_path, &relative) {
                self.turn_diff.snapshot(&path);
            }
        }
        let (summary, diff, _affected) = apply_patch::apply_parsed(&self.project_path, &parsed)?;
        self.emit_turn_diff();
        let mut response = summary;
        if !diff.is_empty() {
            response.push_str("\n\nUnified diff preview:\n");
            response.push_str(&diff.chars().take(12_000).collect::<String>());
        }
        Ok(response)
    }

    /// Emits `turn_diff_updated` with the aggregate diff for this turn.
    pub(super) fn emit_turn_diff(&self) {
        let diff = self.turn_diff.unified_diff(&self.project_path);
        if diff.is_empty() {
            return;
        }
        self.handle.emit(json!({
            "type": "turn_diff_updated",
            "diff": diff,
            "filesChanged": self.turn_diff.changed_files(&self.project_path),
        }));
    }

    /// Snapshots turn-diff baselines for a mutating tool call.
    pub(super) fn snapshot_turn_diff(&self, name: &str, args: &Value) {
        if !tools::MUTATION_TOOLS.contains(&name) || name == "apply_patch" {
            return;
        }
        let mut paths: Vec<String> = Vec::new();
        if let Some(path) = args.get("path").and_then(Value::as_str) {
            paths.push(path.to_string());
        }
        if let Some(from) = args.get("from").and_then(Value::as_str) {
            paths.push(from.to_string());
        }
        if let Some(to) = args.get("to").and_then(Value::as_str) {
            paths.push(to.to_string());
        }
        for relative in paths {
            if let Ok(path) = tools::safe_path(&self.project_path, &relative) {
                self.turn_diff.snapshot(&path);
            }
        }
    }

    /// Manual context compaction (`compact_context` tool).
    pub(super) async fn compact_context_tool(&mut self) -> Result<String, AgentError> {
        let before = self.history.len();
        self.run_compaction("manual").await?;
        Ok(format!(
            "Context compacted: {before} messages → {} messages. Older conversation was replaced by a summary block.",
            self.history.len()
        ))
    }
}
