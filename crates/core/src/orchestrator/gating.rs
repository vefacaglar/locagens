//! Permission gating of workspace tools: exec policy, standing grants and
//! interactive prompts.

use super::*;

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
        let tool_call = json!({
            "id": call_id,
            "function": {
                "name": name,
                "arguments": serde_json::to_string(args).unwrap_or_else(|_| "{}".into()),
            }
        });
        let mut preview = tools::build_preview(&self.project_path, name, args);
        if let (Some(path), Some(Value::Object(fields))) = (protected.as_ref(), preview.as_mut()) {
            fields.insert("protectedPath".into(), json!(path));
        }
        match self.permission_hook(call_id, name, args).await {
            hook_calls::PermissionVerdict::Allow => return self.execute_with_turn_diff(name, args).await,
            hook_calls::PermissionVerdict::Deny(reason) => return Ok(format!("Denied by PermissionRequest hook: {reason}")),
            hook_calls::PermissionVerdict::Ask => {}
        }
        // One shared prompt path: queues behind other sub-agents' and network
        // prompts, and its slot/status are restored even if this future is
        // aborted (`close_agent` mid-prompt).
        self.notification_hook("permission_prompt", &format!("Locagens needs permission to run {name}")).await;
        let prompt = PermissionPrompt {
            tool_call,
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

        // Standing grants are persisted here only (QA-031), never by the facade.
        // Unknown answers deny (QA-006); protected / dangerous calls are
        // approved once, never as a standing or run-wide grant.
        let mut decision = PermissionDecision::parse_or_deny(&decision);
        if must_prompt && decision.allows() {
            decision = PermissionDecision::AllowOnce;
        }
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
