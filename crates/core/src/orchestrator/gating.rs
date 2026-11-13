//! Permission gating of workspace tools: exec policy, standing grants and
//! interactive prompts.

use super::*;
use locagens_execpolicy::stdin_input;

/// Pre-prompt verdict for one line typed into a shell / REPL session.
enum TypedLineGate {
    /// Known-safe, allow-listed or covered by a grant: send it.
    Send,
    /// Forbidden by an execpolicy rule.
    Blocked(String),
    /// Needs a prompt; `once_only` for dangerous lines (no standing grant).
    Prompt { once_only: bool },
}

impl AgentLoop {
    /// Loads the merged Starlark `.rules` exec policy for this run: every
    /// agent-directory `rules` folder plus, for a trusted project folder
    /// only, `<project>/.locagens/rules` (an untrusted repo could otherwise
    /// ship `allow` rules for any command).
    pub(super) fn load_exec_policy(&self) -> locagens_execpolicy::rules::Policy {
        let mut dirs: Vec<PathBuf> = crate::paths::agent_dir_chain()
            .into_iter()
            .map(|dir| dir.join("rules"))
            .collect();
        if crate::services::project_trust::is_trusted(&self.project_path) {
            dirs.push(self.project_path.join(".locagens").join("rules"));
        }
        locagens_execpolicy::rules::Policy::load_from_dirs(&dirs)
    }

    // -- Permission gating -------------------------------------------------

    /// Network allowlist + live domain approval for a sandboxed command.
    pub(super) fn command_network_policy(
        &self,
        args: &Value,
    ) -> (locagens_execpolicy::network_proxy::NetworkPolicy, network_access::ApprovalWindow) {
        network_access::command_network_policy(&self.state, &self.handle, &self.project_path, args)
    }

    pub(super) fn standing_grant(&self, name: &str, args: &Value) -> bool {
        let project_path = self.project_path.display().to_string();
        let command = extract_command_str(args);
        let domains = sorted_domains(args);
        self.state
            .db
            .list_permissions()
            .iter()
            .any(|rule| standing_grant_matches(rule, name, command, &domains, &project_path))
    }

    pub(super) async fn gated_execute(&self, call_id: &str, name: &str, args: &Value) -> Result<String, AgentError> {
        // Codex command safety + Starlark `.rules` execpolicy: under
        // accept_edits (on-request + workspace-write) allow-listed commands
        // run without a prompt; forbidden rules block outright. Requested
        // network domains are only granted through a prompt (or a matching
        // standing grant), never by the known-safe bypass.
        let is_command = name == "run_command" || name == "exec_command";
        let mut dangerous = false;
        if is_command {
            let cmd_str = command_safety::command_from_args(args);
            dangerous = command_safety::is_dangerous_command(&cmd_str);
            let policy = self.load_exec_policy();
            let requests_network = !sorted_domains(args).is_empty();
            match command_safety::command_bypasses_prompt_with_policy(&self.mode, &cmd_str, Some(&policy)) {
                Ok(true) if !requests_network => {
                    return self.execute_tool(name, args).await.map_err(AgentError::Failed)
                }
                Ok(_) => {}
                Err(reason) => {
                    return Ok(json!({
                        "ok": false,
                        "error": format!("Command blocked by execpolicy rule: {reason}")
                    })
                    .to_string());
                }
            }
        }
        // Outside Full Access, writes to protected paths (`.git`, `.locagens`,
        // … — what the command sandbox also protects) and dangerous commands
        // always prompt: no standing grant or "Allow for Run" covers them,
        // and the answer approves this one call only.
        let protected = if self.mode == "auto" {
            None
        } else {
            tools::protected_mutation_target(&self.project_path, name, args)
        };
        let must_prompt = protected.is_some() || (dangerous && self.mode != "auto");
        if !must_prompt
            && (!tools::needs_permission(&self.mode, name)
                || self.handle.tool_allowed_for_run(name)
                || self.standing_grant(name, args))
        {
            return self.execute_with_turn_diff(name, args).await;
        }

        // Pause for an interactive decision: status → awaiting_permission,
        // permission_requested event, then wait for `api::runs::permission_decision`.
        let mut preview = tools::build_preview(&self.project_path, name, args);
        if let (Some(path), Some(Value::Object(fields))) = (protected.as_ref(), preview.as_mut()) {
            fields.insert("protectedPath".into(), json!(path));
        }
        let decision = match self.ask_tool_permission(call_id, name, args, preview).await? {
            Ok(decision) => decision,
            Err(reason) => return Ok(format!("Denied by PermissionRequest hook: {reason}")),
        };

        // Standing grants are persisted here only (QA-031), never by the facade.
        // Unknown answers deny (QA-006); protected / dangerous calls are
        // approved once, never as a standing or run-wide grant.
        let decision = if must_prompt && decision.allows() { PermissionDecision::AllowOnce } else { decision };
        match decision {
            PermissionDecision::Deny => Ok("The user denied permission for this tool call.".to_string()),
            PermissionDecision::AllowRun => {
                self.handle.allow_tool_for_run(name);
                self.execute_with_turn_diff(name, args).await
            }
            PermissionDecision::AllowProject | PermissionDecision::AllowAlways => {
                // Standing grants live in `permissions.json` under the
                // Locagens home and match the exact command + domains
                // (`standing_grant_matches`); nothing is written into the
                // project folder.
                self.state.db.upsert_permission(
                    decision.standing_scope().unwrap_or("project"),
                    &self.project_path.display().to_string(),
                    name,
                    extract_command_str(args),
                    sorted_domains(args),
                    decision.as_str(),
                );
                self.execute_with_turn_diff(name, args).await
            }
            PermissionDecision::AllowOnce => self.execute_with_turn_diff(name, args).await,
        }
    }

    /// The one prompt path of a gated call: the PermissionRequest hook first,
    /// then the shared permission queue (it queues behind other sub-agents'
    /// and network prompts, and its slot/status are restored even if this
    /// future is aborted, e.g. `close_agent` mid-prompt). `Ok(Err(reason))`
    /// is a hook denial; a hook approval counts as "allow once".
    pub(super) async fn ask_tool_permission(
        &self,
        call_id: &str,
        name: &str,
        args: &Value,
        preview: Option<Value>,
    ) -> Result<Result<PermissionDecision, String>, AgentError> {
        match self.permission_hook(call_id, name, args).await {
            hook_calls::PermissionVerdict::Allow => return Ok(Ok(PermissionDecision::AllowOnce)),
            hook_calls::PermissionVerdict::Deny(reason) => return Ok(Err(reason)),
            hook_calls::PermissionVerdict::Ask => {}
        }
        self.notification_hook("permission_prompt", &format!("Locagens needs permission to run {name}")).await;
        let prompt = PermissionPrompt {
            tool_call: json!({
                "id": call_id,
                "function": {
                    "name": name,
                    "arguments": serde_json::to_string(args).unwrap_or_else(|_| "{}".into()),
                }
            }),
            preview,
            agent_role: self.agent_role.clone(),
            agent_name: self.agent_name.clone(),
            armed: None,
        };
        let Some(decision) = self.handle.ask_permission(&self.state.db, prompt).await else {
            return Err(AgentError::Cancelled);
        };
        if self.stop_requested() {
            return Err(AgentError::Cancelled);
        }
        Ok(Ok(PermissionDecision::parse_or_deny(&decision)))
    }

    /// The pre-prompt verdict for one line typed into a shell / REPL
    /// session: the same classification as an `exec_command` of that line
    /// (execpolicy rules, known-safe commands, dangerous commands, standing
    /// grants and "Allow for Run" of `exec_command`).
    fn typed_line_gate(&self, policy: &locagens_execpolicy::rules::Policy, line: &str) -> TypedLineGate {
        match command_safety::command_bypasses_prompt_with_policy(&self.mode, line, Some(policy)) {
            Err(reason) => return TypedLineGate::Blocked(reason),
            Ok(true) => return TypedLineGate::Send,
            Ok(false) => {}
        }
        if command_safety::is_dangerous_command(line) {
            return TypedLineGate::Prompt { once_only: true };
        }
        let as_command = json!({ "cmd": line });
        if self.handle.tool_allowed_for_run("exec_command") || self.standing_grant("exec_command", &as_command) {
            return TypedLineGate::Send;
        }
        TypedLineGate::Prompt { once_only: false }
    }

    /// QA-012: what of `chars` may be written to exec session `session_id`
    /// now. Outside Full Access, input to a session that reads commands
    /// (see `locagens_execpolicy::stdin_input`) is split into submitted
    /// lines and each line is gated like a command; one prompt covers every
    /// line of the write that needs approval. Text after the last line break
    /// is held back until a later write ends the line. Input to any other
    /// program, and interrupt / end-of-input keys, pass unchanged.
    ///
    /// Returns the text to write, or `Err(response)` when nothing is sent.
    pub(super) async fn gate_stdin_input(
        &self,
        call_id: &str,
        session_id: &str,
        chars: &str,
    ) -> Result<Result<String, String>, AgentError> {
        let orchestrator = &self.state.orchestrator;
        if self.mode == "auto" || !orchestrator.exec_session_reads_commands(&self.run_id, session_id) {
            return Ok(Ok(chars.to_string()));
        }
        if stdin_input::is_signal_input(chars) {
            orchestrator.set_held_input(&self.run_id, session_id, String::new());
            return Ok(Ok(chars.to_string()));
        }
        // The held text is dropped unless this write is allowed.
        let input = stdin_input::split_submitted_input(&orchestrator.take_held_input(&self.run_id, session_id), chars);
        let policy = self.load_exec_policy();
        let mut prompted: Vec<String> = Vec::new();
        let mut once_only = false;
        for line in input.lines.iter().map(|line| line.trim()).filter(|line| !line.is_empty()) {
            match self.typed_line_gate(&policy, line) {
                TypedLineGate::Send => {}
                TypedLineGate::Blocked(reason) => {
                    return Ok(Err(json!({
                        "ok": false,
                        "error": format!("Input blocked by execpolicy rule: {reason}. Nothing was sent.")
                    })
                    .to_string()))
                }
                TypedLineGate::Prompt { once_only: dangerous } => {
                    once_only |= dangerous;
                    prompted.push(line.to_string());
                }
            }
        }
        if !prompted.is_empty() {
            let commands = prompted.join("\n");
            let args = json!({ "session_id": session_id, "chars": commands });
            let preview = json!({
                "tool": "write_stdin",
                "action": "command",
                "path": "",
                "absolutePath": self.project_path.display().to_string(),
                "oldContent": Value::Null,
                "newContent": Value::Null,
                "command": commands,
                "sessionId": session_id,
                "networkDomains": [],
            });
            let decision = match self.ask_tool_permission(call_id, "write_stdin", &args, Some(preview)).await? {
                Ok(decision) => decision,
                Err(reason) => return Ok(Err(format!("Denied by PermissionRequest hook: {reason}. Nothing was sent."))),
            };
            match decision {
                PermissionDecision::Deny => {
                    return Ok(Err("The user denied permission for this input; nothing was sent.".to_string()))
                }
                // Dangerous lines are approved for this write only.
                _ if once_only => {}
                PermissionDecision::AllowOnce => {}
                PermissionDecision::AllowRun => self.handle.allow_tool_for_run("exec_command"),
                PermissionDecision::AllowProject | PermissionDecision::AllowAlways => {
                    for line in &prompted {
                        self.state.db.upsert_permission(
                            decision.standing_scope().unwrap_or("project"),
                            &self.project_path.display().to_string(),
                            "exec_command",
                            line,
                            Vec::new(),
                            decision.as_str(),
                        );
                    }
                }
            }
        }
        orchestrator.set_held_input(&self.run_id, session_id, input.held);
        Ok(Ok(input.to_send))
    }

    /// Executes a gated tool, snapshotting turn-diff baselines for mutating
    /// tools first and emitting `turn_diff_updated` afterwards.
    pub(super) async fn execute_with_turn_diff(&self, name: &str, args: &Value) -> Result<String, AgentError> {
        self.snapshot_turn_diff(name, args);
        let result = self.execute_tool(name, args).await;
        if result.is_ok() {
            self.emit_turn_diff();
        }
        result.map_err(AgentError::Failed)
    }
}

/// The command string of a `run_command`/`exec_command` call: `command`
/// first, Codex's `cmd` as fallback.
pub(super) fn extract_command_str(args: &Value) -> &str {
    args.get("command")
        .or_else(|| args.get("cmd"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

pub(super) fn sorted_domains(args: &Value) -> Vec<String> {
    args.get("network_domains")
        .or_else(|| args.get("networkDomains"))
        .and_then(Value::as_array)
        .map(|items| {
            let mut domains: Vec<String> = items.iter().filter_map(Value::as_str).map(str::to_string).collect();
            domains.sort();
            domains
        })
        .unwrap_or_default()
}

/// Whether a persisted permission rule covers this exact tool call.
pub(super) fn standing_grant_matches(
    rule: &crate::db::PermissionRule,
    tool: &str,
    command: &str,
    domains: &[String],
    project_path: &str,
) -> bool {
    if !rule.status.starts_with("allow") || rule.tool != tool {
        return false;
    }
    let scope_ok = rule.scope == "global" || (rule.scope == "project" && rule.project_path == project_path);
    if !scope_ok {
        return false;
    }
    if tool == "run_command" || tool == "exec_command" {
        // An empty rule command used to be saved for exec_command (its args
        // use `cmd`); never let it act as a wildcard.
        if rule.command.is_empty() || command.is_empty() || rule.command != command {
            return false;
        }
        if !rule.network_domains.is_empty() {
            let mut rule_domains = rule.network_domains.clone();
            rule_domains.sort();
            if rule_domains != domains {
                return false;
            }
        }
    }
    true
}
